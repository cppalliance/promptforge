"""Coverage, bare-symbol, and banned-string checks for a facade's pages.

usage: python tools/scripts/check_docs.py CRATE [<page>]
Without <page>, every checklist entry in target/dokuman-CRATE/checklist-head.txt
needs an intra-doc link on some page. With <page>, every entry in that page's
checklist-<stem>.txt needs one on the page itself, and only that page's bare
and banned hits count. A bare hit is a prose code span that looks like an
unlinked Rust symbol; a banned hit is a string no page may contain. Details
go to target/dokuman-CRATE/check-<stem>.md, or check-all.md.

gates.py also imports split_logs, which turns rustdoc, doctest, and surface
check logs into failures by page.
"""
import re
import sys
from pathlib import Path

sys.dont_write_bytecode = True
import facade  # noqa: E402

CODE_SPAN = re.compile(r"`([^`\n]+)`")
LUA_GLOBALS = {"user_input", "ui", "jump", "call", "fanout", "list_from_section", "tostring", "pcall", "print", "require"}
LUA_TABLES = {"messages", "models", "tools", "store", "tasks", "sys", "var", "argv", "compactors", "string", "table", "math"}
SOURCE = "(source)"


def coverage_missing(pages, entries, ident):
    linked = facade.suffixes(set().union(*(facade.link_targets(t, ident) for t in pages.values())) if pages else set())
    return [e for e in entries if e[1] not in linked and e[1].split("::", 1)[-1] not in linked]


def bare_hits(pages, names, methods):
    """Return (page, line number, span) for each prose code span that looks like an unlinked Rust symbol."""
    path_like = re.compile(r"^[A-Za-z_][A-Za-z0-9_]*(::[A-Za-z_][A-Za-z0-9_]*)+(\(\))?$")
    call_like = re.compile(r"^(?:([a-z_][a-z0-9_]*)\.)?([a-z_][a-z0-9_]*)\(\)$")
    hits = []
    for name, text in pages.items():
        linked = {m.start() for m in re.finditer(r"\[`", text)}
        for m in CODE_SPAN.finditer(text):
            if m.start() - 1 in linked:
                continue
            span, call = m.group(1), call_like.match(m.group(1))
            rust_call = call and call.group(2) in methods and call.group(2) not in LUA_GLOBALS and call.group(1) not in LUA_TABLES
            if span in names or path_like.match(span) or rust_call:
                hits.append((name, text.count("\n", 0, m.start()) + 1, span))
    return hits


def banned_hits(raw_pages, banned):
    hits = []
    for name, text in raw_pages.items():
        for n, line in enumerate(text.splitlines(), 1):
            for word in banned:
                if word in line:
                    hits.append((name, n, word.strip() or repr(word)))
    return hits


def coverage(f, checklist, page=None):
    """Return (counts, detail lines, failures by page) for every page, or for one page."""
    pages, raw = facade.load_pages(f), facade.load_pages(f, raw=True)
    entries, names, methods = facade.load_checklist(checklist, f.ident)
    head = f.scratch / "checklist-head.txt"
    if page and head.exists():
        _, names, methods = facade.load_checklist(head, f.ident)
    missing = coverage_missing({page: pages.get(page, "")} if page else pages, entries, f.ident)
    bare = [h for h in bare_hits(pages, names, methods) if not page or h[0] == page]
    banned = [h for h in banned_hits(raw, f.banned()) if not page or h[0] == page]
    mods = facade.module_map(f.lib_rs_text())
    per_page = {}
    for module, _, line in missing:
        per_page.setdefault(page or mods.get(module) or facade.page_for(module), []).append(f"unlinked: {line}")
    for name, n, span in bare:
        per_page.setdefault(name, []).append(f"bare line {n}: `{span}` needs an intra-doc link")
    for name, n, word in banned:
        per_page.setdefault(name, []).append(f"banned line {n}: `{word}`")
    lines = [f"{name}: {item}" for name, items in sorted(per_page.items()) for item in items]
    counts = {"missing": len(missing), "entries": len(entries), "bare": len(bare), "banned": len(banned)}
    return counts, lines, per_page


def split_logs(f, logs, kinds=("rustdoc", "doctest", "surface")):
    """Return failures by page from rustdoc, doctest, and surface-check logs.

    A surface finding on an item rather than on the crate or one of its
    modules comes from a doc comment in an internal crate, not from a page,
    and is filed under SOURCE."""
    mods = facade.module_map(f.lib_rs_text())
    crate_dir = re.escape(f.crate)
    rustdoc = re.compile(r"^\s*--> crates[\\/]" + crate_dir + r"[\\/]src[\\/](\w+\.md):(\d+)")
    doctest = re.compile(r"crates[\\/]" + crate_dir + r"[\\/]src[\\/](\w+)\.(md|rs) - (\w*) ?\(line (\d+)\)")
    label = re.compile(r"^" + re.escape(f.ident) + r"((?:::\w+)*)$")
    named = re.compile(r"\b" + re.escape(f.ident) + r"\b")
    per_page = {}
    for log in logs:
        if not Path(log).exists():
            continue
        text = facade.read_log(log)
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
                target = mods[segments[0]] or facade.page_for(segments[0])
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
                    page, where = mods.get(module) or facade.page_for(module), f"doctest near lib.rs line {number}"
                per_page.setdefault(page, []).append(f"{where}:\n" + block.strip()[:1500])
    return per_page


def page_name(argument):
    name = Path(argument).name
    return name if name.endswith(".md") else name + ".md"


if __name__ == "__main__":
    f, rest = facade.args("check_docs.py CRATE [<page>]", 0, 1)
    page = page_name(rest[0]) if rest else None
    checklist = f.scratch / (f"checklist-{page[:-3]}.txt" if page else "checklist-head.txt")
    if not checklist.exists():
        facade.stop(f"no {checklist.name}; run survey.py first")
    counts, lines, _ = coverage(f, checklist, page)
    details = f.scratch / f"check-{page[:-3] if page else 'all'}.md"
    facade.write(details, "# check_docs details\n\n" + "\n".join(lines) + "\n")
    print(f"COVERAGE missing={counts['missing']} of {counts['entries']}")
    print(f"BARE hits={counts['bare']}")
    print(f"BANNED hits={counts['banned']}")
    for line in lines[:15]:
        print("  " + line)
    if len(lines) > 15:
        print(f"  ... {len(lines) - 15} more in {details}")
