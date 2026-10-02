"""Mechanical checks on a facade's pages, briefs, summaries, and sample text.

usage:
  python tools/cicerone/scripts/checks.py CRATE [<page>]
  python tools/cicerone/scripts/checks.py CRATE --brief <page>
  python tools/cicerone/scripts/checks.py CRATE --summaries
  python tools/cicerone/scripts/checks.py CRATE --text <file>

A page check covers banned strings and phrases, page shape, tour shape, the
length limits, links, and Reference coverage against the plan. The first
output line is `CHECKS pass` or `CHECKS fail <count>`; up to 15 failures
follow, and all of them go to target/cicerone-CRATE/check-<stem>.md. Each
banned-phrase failure names its replacement.

--brief checks a brief for banned strings, banned phrases, and the sections
the plan requires. --summaries reports the rustdoc summary lines, which come
from doc comments in the defining crates, that fail the phrase checks; they
are findings, never failures. --text applies the phrase and length checks to
any Markdown file.

gates.py imports page_failures and split_logs.
"""
import json
import re
import sys
from pathlib import Path

sys.dont_write_bytecode = True
import common  # noqa: E402

SOURCE = "(source)"
HEADING = re.compile(r"^(#{1,6})\s+(.+?)\s*$")
LIST_ITEM = re.compile(r"^\s*(?:[-*+]|\d+[.)])\s+")
TOKEN = re.compile(r"\[`([^`\]]+)`\](?:\(([^)\s]+)\))?|\[([^\]`][^\]]*)\]\(([^)\s]+)\)|`([^`\n]+)`")
PATH_LIKE = re.compile(r"^[A-Za-z_][A-Za-z0-9_]*(::[A-Za-z_][A-Za-z0-9_]*)+(\(\))?$")
CALL_LIKE = re.compile(r"^(?:([a-z_][a-z0-9_]*)\.)?([a-z_][a-z0-9_]*)\(\)$")
SPLIT = re.compile(r"(?<=[.!?])[\"')\]]*\s+(?=[A-Z0-9\"'(\[])")
WORD = re.compile(r"[A-Za-z0-9][\w'-]*")
LUA_GLOBALS = {"ui", "jump", "call", "fanout", "list_from_section", "tostring", "pcall", "print", "require"}
LUA_TABLES = {"messages", "models", "tools", "store", "tasks", "sys", "var", "argv", "compactors", "input", "string", "table", "math"}

LIB_OPEN, LIB_LAST_TOUR, LIB_CLOSE = "Before you start", "The complete program", ("Reference", "Where to go next")
MODULE_OPEN, MODULE_CLOSE = "Where this fits", ("Reference",)
LIMITS = {"first_words": 20, "sentence": 40, "mean": 22, "para_sentences": 5, "para_words": 100,
          "tour_words": float("inf"), "ref_words": float("inf"), "ref_bullets": float("inf"),
          "bullet_words": float("inf"), "example_lines": 45,
          "fits_lines": 5, "fence_within": 6}

# Each banned phrase, as a case-insensitive regex, with the replacement a writer uses instead.
PHRASES = [
    (r"not built yet", "cut the sentence and describe what exists"),
    (r"not yet", "cut the sentence and describe what exists"),
    (r"today", "cut the word and state the behavior"),
    (r"currently", "cut the word and state the behavior"),
    (r"in this version", "cut the phrase and state the behavior"),
    (r"for now", "cut the phrase"),
    (r"in the future", "cut the sentence"),
    (r"reserved for future", "cut the sentence"),
    (r"deferred", "cut the sentence"),
    (r"is unknown", "cut the sentence, or state what is known"),
    (r"in-repo", "cut the reference to other programs in the repository"),
    (r"the engine", "the run, or this crate"),
    (r"facades?", "this crate"),
    (r"the family", "this crate"),
    (r"shims?", "name what the code does"),
    (r"containers?", "name the thing itself"),
    (r"utili[sz]e[sd]?", "use"),
    (r"leverag(?:e|es|ed|ing)", "use"),
    (r"note that", "cut the phrase"),
    (r"simply", "cut the word"),
    (r"obviously", "cut the word"),
    (r"of course", "cut the phrase"),
    (r"e\.g\.", "for example"),
    (r"i\.e\.", "that is"),
    (r"etc\.", "name the rest, or cut it"),
]
REFERENCE_PHRASES = [
    (r"takes `&self`", "say what the call does for the caller"),
    (r"takes `&mut self`", "state the consequence, such as one caller at a time"),
    (r"cannot fail", "cut it; the signature shows it"),
    (r"has no arguments", "cut it; the signature shows it"),
    (r"takes no arguments", "cut it; the signature shows it"),
    (r"returns nothing", "cut it; the signature shows it"),
]
SHOWN = {r"facades?": "facade", r"shims?": "shim", r"containers?": "container", r"utili[sz]e[sd]?": "utilize",
         r"leverag(?:e|es|ed|ing)": "leverage", r"e\.g\.": "e.g.", r"i\.e\.": "i.e.", r"etc\.": "etc."}
COMPILED = [(re.compile(r"(?<![\w-])" + p + r"(?![\w-])", re.I), SHOWN.get(p, p), r) for p, r in PHRASES]
COMPILED_REF = [(re.compile(p, re.I), p, r) for p, r in REFERENCE_PHRASES]


def records(text):
    """(line number, line, fenced) for every line; fence delimiters count as fenced."""
    out, fence = [], None
    for n, line in enumerate(text.split("\n"), 1):
        m = common.FENCE.match(line)
        if fence is None:
            if m:
                fence = m.group(1)
            out.append((n, line, fence is not None))
            continue
        out.append((n, line, True))
        stripped = line.strip()
        if m and stripped.startswith(fence) and set(stripped) == {fence[0]}:
            fence = None
    return out


def headings(recs):
    """(index, level, title, line number) for every heading outside a fence."""
    out = []
    for i, (n, line, fenced) in enumerate(recs):
        m = None if fenced else HEADING.match(line)
        if m:
            out.append((i, len(m.group(1)), m.group(2), n))
    return out


def span(recs, heads, index, level):
    """The record range after heading `index` up to the next heading at `level` or above."""
    start = heads[index][0] + 1
    end = next((h[0] for h in heads[index + 1:] if h[1] <= level), len(recs))
    return start, end


def blocks(recs):
    """Prose blocks outside fences, headings, and HTML comments: dicts of kind, first line, and lines."""
    found, current, comment = [], [], False

    def close():
        if current:
            first = current[0][1]
            kind = "table" if first.lstrip().startswith("|") else "list" if LIST_ITEM.match(first) else "para"
            found.append({"kind": kind, "line": current[0][0], "lines": list(current)})
            current.clear()

    for n, line, fenced in recs:
        stripped = line.strip()
        if comment:
            comment = "-->" not in stripped
            continue
        if fenced or not stripped or HEADING.match(line):
            close()
            continue
        if stripped.startswith("<!--"):
            close()
            comment = "-->" not in stripped
            continue
        current.append((n, line))
    close()
    return found


def items_of(block):
    """(line number, text) for each item of a list block."""
    out = []
    for n, line in block["lines"]:
        m = LIST_ITEM.match(line)
        if m:
            out.append([n, line[m.end():].strip()])
        elif out:
            out[-1][1] += " " + line.strip()
    return [tuple(item) for item in out]


def text_of(block):
    return " ".join(line.strip() for _, line in block["lines"])


def plain(text):
    text = TOKEN.sub(lambda m: " CODE " if m.group(5) else " LINK ", text)
    return re.sub(r"[*_]{1,2}", "", text)


def sentences(text):
    return [s for s in SPLIT.split(plain(text).strip()) if WORD.search(s)]


def words(text):
    return len(WORD.findall(plain(text)))


def units(recs):
    """(line number, text) for every paragraph and list item, the units that hold sentences."""
    out = []
    for block in blocks(recs):
        if block["kind"] == "para":
            out.append((block["line"], text_of(block)))
        elif block["kind"] == "list":
            out.extend(items_of(block))
    return out


def phrase_hits(recs, compiled, where="", keep_code=False):
    fails = []
    for n, text in units(recs):
        bare = text if keep_code else re.sub(r"`[^`\n]+`", " ", text)
        for pattern, shown, replacement in compiled:
            if pattern.search(bare):
                fails.append(f"line {n}: banned phrase `{shown}`{where}; instead: {replacement}")
    return fails


def raw_bans(f, recs):
    fails = []
    for n, line, fenced in recs:
        for word in f.banned_raw():
            if word == " -- " and fenced:
                continue
            if word in line:
                fails.append(f"line {n}: banned string `{word.strip() or repr(word)}`")
        if "<!-- note:" in line:
            fails.append(f"line {n}: leftover skeleton note; remove every `<!-- note:` comment")
    return fails


def sibling_fails(f, recs):
    """Other crates of this repository, which a page never names because its reader may not have them. A facade
    crate this crate depends on is the exception: its reader already uses it."""
    others = sorted(set(common.crate_dirs()) - {f.ident} - common.public_dependencies(f), key=len, reverse=True)
    if not others:
        return []
    names = "|".join(re.escape(o) for o in others + [o.replace("_", "-") for o in others])
    mention = re.compile(r"(?<![\w:])(?:" + names + r")::|docs\.rs/(?:" + names + r")\b|`?\b(?:" + names + r")\b`?\s+crates?\b")
    fails = []
    for n, line, fenced in recs:
        if not fenced:
            for m in mention.finditer(line):
                fails.append(f"line {n}: `{m.group(0).replace('`', '')}` names another crate of this repository; "
                             "describe what this crate asks of the reader's own code instead")
    return fails


ITEM_ANCHORS = ("variant.", "method.", "structfield.", "tymethod.", "associatedtype.", "associatedconstant.", "impl-")


def slug(heading):
    """The id rustdoc gives a heading: its words in lowercase joined by dashes, with ticks and punctuation dropped."""
    heading = re.sub(r"\[([^\]]*)\]\([^)]*\)", r"\1", heading).replace("`", "")
    out = []
    for c in heading.strip():
        if c.isalnum() or c in "-_":
            out.append(c.lower() if c.isascii() else c)
        elif c.isspace() and c.isascii():
            out.append("-")
    return "".join(out)


def heading_ids(recs):
    """Every heading id on a page, with a repeated heading numbered the way rustdoc numbers it."""
    seen, ids = {}, set()
    for _, line, fenced in recs:
        m = None if fenced else re.match(r"^#{1,6} (.+?)\s*$", line)
        if m:
            s = slug(m.group(1))
            ids.add(f"{s}-{seen[s]}" if s in seen else s)
            seen[s] = seen.get(s, 0) + 1
    return ids


def anchor_fails(f, page, recs):
    """Links to a heading on a page of this crate whose anchor names no heading there; rustdoc never checks them."""
    ids, fails = {common.stem(page): heading_ids(recs)}, []
    for n, line, fenced in recs:
        if fenced:
            continue
        for m in re.finditer(r"\]\(([^)\s#]*)#([^)\s]+)\)", line):
            dest, frag = m.group(1), m.group(2)
            if frag.startswith(ITEM_ANCHORS) or "://" in dest:
                continue
            target = common.stem(page) if dest in ("", "self") else "lib" if dest == "crate" else dest.split("::")[-1]
            path = f.src / f"{target}.md"
            if target not in ids and not path.is_file():
                continue
            if target not in ids:
                ids[target] = heading_ids(records(path.read_text(encoding="utf-8")))
            if frag not in ids[target]:
                fails.append(f"line {n}: `{dest}#{frag}` names no heading on {target}.md; link one of its headings")
    return fails


def length_fails(recs):
    fails, lengths = [], []
    for n, text in units(recs):
        for s in sentences(text):
            count = words(s)
            lengths.append(count)
            if count > LIMITS["sentence"]:
                fails.append(f"line {n}: a {count}-word sentence; split it to at most {LIMITS['sentence']} words: \"{s[:70]}...\"")
    for block in blocks(recs):
        if block["kind"] == "para":
            text = text_of(block)
            count, many = words(text), len(sentences(text))
            if many > LIMITS["para_sentences"] or count > LIMITS["para_words"]:
                fails.append(f"line {block['line']}: a paragraph of {many} sentences and {count} words; "
                             f"keep paragraphs to {LIMITS['para_sentences']} sentences and {LIMITS['para_words']} words")
    if len(lengths) >= 5 and sum(lengths) / len(lengths) > LIMITS["mean"]:
        fails.append(f"mean sentence length is {sum(lengths) / len(lengths):.1f} words; bring it to {LIMITS['mean']} or less")
    return fails


def text_failures(path):
    recs = records(Path(path).read_text(encoding="utf-8"))
    return phrase_hits(recs, COMPILED) + length_fails(recs)


def shape_fails(page, recs, heads):
    fails = []
    h1 = [(k, h) for k, h in enumerate(heads) if h[1] == 1]
    titles = [h[2] for _, h in h1]
    if page == "lib.md":
        opening, closing, low, high = LIB_OPEN, list(LIB_CLOSE), 3, 6
    else:
        opening, closing, low, high = MODULE_OPEN, list(MODULE_CLOSE), 1, 3
    if not titles or titles[0] != opening:
        fails.append(f"the first level-1 heading must be `# {opening}`")
    if titles[-len(closing):] != closing:
        fails.append("the page must end with " + ", then ".join(f"`# {c}`" for c in closing))
    fixed = {opening, *closing}
    tours = [(k, h) for k, h in h1 if h[2] not in fixed]
    if not low <= len(tours) <= high:
        fails.append(f"{len(tours)} tours; this page needs {low} to {high}")
    if page == "lib.md" and (not tours or tours[-1][1][2] != LIB_LAST_TOUR):
        fails.append(f"the last tour must be `# {LIB_LAST_TOUR}`")
    for k, h in tours:
        start, end = span(recs, heads, k, 1)
        seen, opened = 0, False
        for n, line, fenced in recs[start:end]:
            if fenced:
                opened = True
                break
            if line.strip():
                seen += 1
        if not opened or seen >= LIMITS["fence_within"]:
            fails.append(f"line {h[3]}: tour `{h[2]}` must reach its example within its first {LIMITS['fence_within']} non-empty lines")
        prose = [b for b in blocks(recs[start:end]) if b["kind"] in ("para", "list")]
        if not prose or not any(m.group(1) or m.group(3) for m in TOKEN.finditer(text_of(prose[-1]))):
            fails.append(f"line {h[3]}: tour `{h[2]}` must end with a link to the next tour or page")
        count = sum(words(text_of(b)) for b in prose)
        if count > LIMITS["tour_words"]:
            fails.append(f"line {h[3]}: tour `{h[2]}` has {count} words of prose; cut it to {LIMITS['tour_words']}")
    for k, h in h1:
        if h[2] == MODULE_OPEN and page != "lib.md":
            start, end = span(recs, heads, k, 1)
            count = sum(1 for _, line, _ in recs[start:end] if line.strip())
            if count > LIMITS["fits_lines"]:
                fails.append(f"line {h[3]}: `# {MODULE_OPEN}` has {count} lines; keep it to {LIMITS['fits_lines']}")
    first = next(iter(blocks(recs[:heads[0][0]] if heads else recs)), None)
    if first is None or first["kind"] != "para":
        fails.append("the page must open with a one-sentence summary paragraph")
    else:
        text = text_of(first)
        if len(sentences(text)) != 1 or words(text) > LIMITS["first_words"]:
            fails.append(f"line {first['line']}: the opening summary must be one sentence of at most {LIMITS['first_words']} words")
    return fails


def reference_fails(recs, heads):
    """Failures in the Reference section, and each entry's title mapped to its text."""
    fails, texts = [], {}
    ref = next((k for k, h in enumerate(heads) if h[1] == 1 and h[2] == "Reference"), None)
    if ref is None:
        return fails, texts
    start, end = span(recs, heads, ref, 1)
    entries = [(k, h) for k, h in enumerate(heads) if h[1] == 2 and start <= h[0] < end]
    for k, h in entries:
        s, e = span(recs, heads, k, 2)
        texts[h[2]] = "\n".join(line for _, line, _ in recs[s:e])
        found = blocks(recs[s:e])
        paras = [b for b in found if b["kind"] == "para"]
        if len(paras) != 1 or words(text_of(paras[0])) > LIMITS["ref_words"]:
            fails.append(f"line {h[3]}: Reference entry `{h[2]}` needs exactly one paragraph of at most {LIMITS['ref_words']} words")
        bullets = [item for b in found if b["kind"] == "list" for item in items_of(b)]
        if len(bullets) > LIMITS["ref_bullets"]:
            fails.append(f"line {h[3]}: Reference entry `{h[2]}` has {len(bullets)} member bullets; keep {LIMITS['ref_bullets']}")
        for n, text in bullets:
            if words(text) > LIMITS["bullet_words"]:
                fails.append(f"line {n}: a member bullet of {words(text)} words; keep it to {LIMITS['bullet_words']}")
        fails += phrase_hits(recs[s:e], COMPILED_REF, " in Reference", keep_code=True)
    return fails, texts


def variants_of(items, key):
    """The variant names of an enum item in the inventory, in declaration order; empty for any other item."""
    item = items.get(key)
    if not item or item["kind"] != "enum":
        return []
    return [member.split(" ", 1)[1].split("::")[-1] for member in item["members"] if member.startswith("variant ")]


def missing_variants(text, variants):
    """The variants that text never names in a code span, such as `Parse`, `RunErrorKind::Parse`, or `Chat { request }`."""
    return [v for v in variants if not re.search(r"`(?:[\w:]*::)?" + re.escape(v) + r"(?:[\s({][^`]*)?`", text)]


def section(body, head):
    """The text under the level-2 heading `## <head>` in body, matched as a whole line, or None when it is absent."""
    m = re.search(r"^## " + re.escape(head) + r"[ \t]*$", body, re.M)
    if m is None:
        return None
    rest = body[m.end():]
    end = re.search(r"^## ", rest, re.M)
    return rest[:end.start()] if end else rest


def example_fails(recs, heads):
    fails, marker, visible, rust, first, tour = [], None, 0, True, 0, ""
    h1 = {h[0]: h[2] for h in heads if h[1] == 1}
    for i, (n, line, fenced) in enumerate(recs):
        if i in h1:
            tour = h1[i]
        if not fenced:
            continue
        m = common.FENCE.match(line)
        stripped = line.strip()
        if marker is None and m:
            info = stripped[len(m.group(1)):].strip().lower()
            marker, visible, first = m.group(1), 0, n
            rust = info in ("", "rust") or "rust" in info.split(",")
            continue
        if m and stripped.startswith(marker) and set(stripped) == {marker[0]}:
            marker = None
            if visible > LIMITS["example_lines"] and tour != LIB_LAST_TOUR:
                fails.append(f"line {first}: an example with {visible} visible lines; hide setup shown earlier or cut it to {LIMITS['example_lines']}")
            continue
        if not (rust and (stripped == "#" or stripped.startswith("# "))):
            visible += 1
    return fails


def link_fails(f, recs, heads, names, methods):
    fails = []
    bounds = [h[0] for h in heads] + [len(recs)]
    starts = [0] + [h[0] + 1 for h in heads]
    for start, end in zip(starts, bounds):
        linked, seen = set(), set()
        for n, text in [(b["line"], text_of(b)) for b in blocks(recs[start:end])]:
            for m in TOKEN.finditer(text):
                if m.group(1) or m.group(3):
                    destination = (m.group(2) or m.group(1)) if m.group(1) else m.group(4)
                    target = common.norm(destination, f.ident)
                    anchor = destination.split("#", 1)[1] if "#" in destination and "#variant." not in destination else ""
                    key = f"{target}#{anchor}" if anchor else target
                    if key and key in seen:
                        fails.append(f"line {n}: `{key}` is linked twice in one section; keep one link, on its first "
                                     "mention, or on the closing link when the second one ends a tour")
                    if key:
                        seen.add(key)
                    if target:
                        linked |= common.suffixes({target})
                    continue
                code = m.group(5)
                call = CALL_LIKE.match(code)
                rust_call = call and call.group(2) in methods and call.group(2) not in LUA_GLOBALS and call.group(1) not in LUA_TABLES
                if (code in names or PATH_LIKE.match(code) or rust_call) and code.rstrip("()") not in linked:
                    fails.append(f"line {n}: `{code}` is a Rust item's first mention in this section; make it an intra-doc link")
                    linked.add(code.rstrip("()"))
    return fails


def inventory(f):
    path = f.scratch / "inventory.txt"
    if not path.exists():
        return {}, set(), set()
    _, names, methods = common.load_checklist(path, f.ident)
    return common.inventory_items(path, f.ident), names, methods


def page_failures(f, page, plan=None, items=None):
    """Every failure on one page, as `line N: message` strings."""
    path = f.src / page
    if not path.exists():
        return [f"{page} does not exist"]
    recs = records(path.read_text(encoding="utf-8"))
    heads = headings(recs)
    known, names, methods = inventory(f)
    items = items or known
    fails = raw_bans(f, recs) + sibling_fails(f, recs) + phrase_hits(recs, COMPILED) + shape_fails(page, recs, heads)
    ref_fails, entries = reference_fails(recs, heads)
    fails += ref_fails + length_fails(recs) + example_fails(recs, heads) + link_fails(f, recs, heads, names, methods)
    fails += anchor_fails(f, page, recs)
    plan = plan if plan is not None else common.load_plan(f)
    if plan and page in plan["pages"]:
        owned = {key.split("::")[-1]: key for key in plan["pages"][page]["owns"]}
        fails += [f"no Reference entry `## {name}` for {key}" for name, key in owned.items() if name not in entries]
        fails += [f"Reference entry `## {name}` is for an item this page does not own" for name in entries if name not in owned]
        for name, key in owned.items():
            gone = missing_variants(entries.get(name, ""), variants_of(items, key)) if name in entries else []
            if gone:
                fails.append(f"Reference entry `## {name}` never names variant{'s' if len(gone) > 1 else ''} "
                             + ", ".join(f"`{v}`" for v in gone) + "; name every variant, in a table row when there are 4 or more")
    return fails


def part_failures(f, path, entry, keys):
    """Failures in one curator file's <brief-part>: the tours part when keys is None, else the part for those item keys.
    Line numbers count from the top of the file."""
    if not path.exists():
        return [f"{path.name} does not exist"]
    text = path.read_text(encoding="utf-8")
    found = common.blocks(text)
    if "brief-part" not in found or "findings-part" not in found:
        return ["the file needs a <brief-part> block and a <findings-part> block, each tag alone on its line"]
    body = found["brief-part"][-1]
    lines = text.split("\n")
    offset = lines.index("<brief-part>") + 1
    while offset < len(lines) and not lines[offset].strip():
        offset += 1
    recs = [(n + offset, line, fenced) for n, line, fenced in records(body)]
    fails = [x for x in raw_bans(f, recs) if "skeleton note" not in x] + phrase_hits(recs, COMPILED)
    titles = {h[2] for h in headings(recs) if h[1] == 2}
    if keys is None:
        if "Page" not in titles:
            fails.append("missing `## Page`")
        for tour in entry["tours"]:
            head = f"Tour: {tour['name']}"
            text = section(body, head)
            if text is None:
                fails.append(f"missing `## {head}`")
                continue
            for field in ("Situation", "Concept", "Claims", "Misconception", "Takeaway", "Next", "Example", "Diagram"):
                if not re.search(r"^- " + field + r":", text, re.M):
                    fails.append(f"`## {head}` lacks `- {field}:`")
    else:
        items, _, _ = inventory(f)
        for key in keys:
            name = key.split("::")[-1]
            text = section(body, f"Item: {name}")
            if text is None:
                fails.append(f"missing `## Item: {name}` for {key}")
                continue
            gone = missing_variants(text, variants_of(items, key))
            if gone:
                fails.append(f"`## Item: {name}` lacks a `Variants:` line for " + ", ".join(f"`{v}`" for v in gone))
    return fails


def brief_failures(f, page, plan=None):
    """Failures in every curator file for a page, each prefixed with the file it is in."""
    plan = plan if plan is not None else common.load_plan(f)
    entry = plan["pages"].get(page) if plan else None
    batches = f.scratch / "batches.json"
    if entry is None or not batches.exists():
        return [f"no plan entry or batches.json for {page}; run survey.py first"]
    parts = [(f.scratch / f"tours-{common.stem(page)}.md", None)]
    parts += [(f.scratch / f"ref-{b['id']}.md", b["items"]) for b in json.loads(batches.read_text(encoding="utf-8"))
              if b["page"] == page and b["items"]]
    return [f"{path.name}: {x}" for path, keys in parts for x in part_failures(f, path, entry, keys)]


def summaries_report(f, required=True):
    path = f.scratch / "summaries.txt"
    if not path.exists():
        if required:
            common.stop("no summaries.txt; run survey.py first")
        return []
    lines = []
    for row in path.read_text(encoding="utf-8").splitlines():
        item, _, text = row.partition("\t")
        bare = re.sub(r"`[^`\n]+`", " ", text)
        hits = [shown for pattern, shown, _ in COMPILED if pattern.search(bare)]
        hits += [w for w in f.banned_raw() if w.strip() and w in text]
        if hits:
            lines.append(f"- {item}: {', '.join(hits)}: \"{text[:160]}\"")
    common.write(f.scratch / "summaries-report.md", "# Summary lines that fail the phrase checks\n\n"
                 "These come from doc comments in the defining crates, so no page edit fixes them.\n\n" + "\n".join(lines) + "\n")
    return lines


def split_logs(f, logs, kinds=("rustdoc", "doctest", "surface")):
    """Return failures by page from rustdoc, doctest, and surface-check logs.

    A surface finding on an item rather than on the crate or one of its
    modules comes from a doc comment in an internal crate, not from a page,
    and is filed under SOURCE."""
    mods = common.module_map(f.lib_rs_text())
    crate_dir = re.escape(f.crate)
    rustdoc = re.compile(r"^\s*--> crates[\\/]" + crate_dir + r"[\\/]src[\\/](\w+\.md):(\d+)")
    doctest = re.compile(r"crates[\\/]" + crate_dir + r"[\\/]src[\\/](\w+)\.(md|rs) - (\w*) ?\(line (\d+)\)")
    label = re.compile(r"^" + re.escape(f.ident) + r"((?:::\w+)*)$")
    named = re.compile(r"\b" + re.escape(f.ident) + r"\b")
    per_page = {}
    for log in logs:
        if not Path(log).exists():
            continue
        text = common.read_log(log)
        lines = text.splitlines()
        for i, line in enumerate(lines):
            m = rustdoc.match(line)
            if m and "rustdoc" in kinds:
                per_page.setdefault(m.group(1), []).append(f"rustdoc line {m.group(2)}: {lines[i - 1].strip()}")
                continue
            head, found, _ = line.partition(": mentions ")
            if not (found and "surface" in kinds and named.search(head)):
                continue
            owner = label.match(head.strip())
            segments = [s for s in owner.group(1).split("::") if s] if owner else None
            if segments == []:
                target = "lib.md"
            elif segments and len(segments) == 1 and segments[0] in mods:
                target = mods[segments[0]] or common.page_for(segments[0])
            else:
                target = SOURCE
            per_page.setdefault(target, []).append(f"surface check: {line.strip()[:400]}")
        if "doctest" in kinds:
            for block in re.split(r"^---- ", text, flags=re.M)[1:]:
                m = doctest.match(block)
                if not m:
                    continue
                stem, extension, module, number = m.groups()
                if extension == "md":
                    page, where = f"{stem}.md", f"doctest at line {number}"
                else:
                    module = module or "root"
                    page, where = mods.get(module) or common.page_for(module), f"doctest near lib.rs line {number}"
                per_page.setdefault(page, []).append(f"{where}:\n" + block.strip()[:1500])
    return per_page


def report(f, label, fails):
    details = f.scratch / f"check-{label}.md"
    common.write(details, f"# checks for {label}\n\n" + "".join(f"- {x}\n" for x in fails))
    print(f"CHECKS {'pass' if not fails else 'fail ' + str(len(fails))}")
    for line in fails[:15]:
        print("  " + line[:220])
    if len(fails) > 15:
        print(f"  ... {len(fails) - 15} more in {details.relative_to(common.REPO).as_posix()}")


def main():
    f, rest = common.args("checks.py CRATE [<page> | --brief <page> | --summaries | --text <file>]", 0, 2)
    if rest[:1] == ["--summaries"]:
        lines = summaries_report(f)
        print(f"SUMMARIES findings={len(lines)} in {(f.scratch / 'summaries-report.md').relative_to(common.REPO).as_posix()}")
        return
    if rest[:1] == ["--text"] and len(rest) == 2:
        report(f, "text-" + Path(rest[1]).stem, text_failures(rest[1]))
        return
    if rest[:1] == ["--brief"] and len(rest) == 2:
        page = common.page_name(rest[1])
        report(f, "brief-" + common.stem(page), brief_failures(f, page))
        return
    plan = common.load_plan(f)
    items, _, _ = inventory(f)
    pages = [common.page_name(rest[0])] if rest else sorted(p.name for p in f.src.glob("*.md"))
    fails = [f"{page} {x}" if not rest else x for page in pages for x in page_failures(f, page, plan, items)]
    report(f, common.stem(pages[0]) if rest else "all", fails)


if __name__ == "__main__":
    main()
