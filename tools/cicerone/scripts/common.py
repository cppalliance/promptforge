"""Shared paths and parsing for the Cicerone scripts.

A facade is a crate at crates/<crate>/ whose lib.rs holds only single-item
`pub use` re-exports grouped into `pub mod` blocks, each block documented by
an `include_str!` page beside lib.rs. Every script in this directory runs
from the repository root, takes the facade crate name first, keeps its
scratch files in target/cicerone-<crate>/, and prints at most 20 lines. A
line starting with STOP ends the run.

Each script sets sys.dont_write_bytecode before importing its siblings, so
no __pycache__ directory appears in the repository.
"""
import json
import os
import re
import subprocess
import sys
from functools import lru_cache
from pathlib import Path

TOOL = "cicerone"
REPO = Path.cwd()
FENCE = re.compile(r"^\s*(`{3,}|~{3,})")
LINK_BARE = re.compile(r"\[`([^`\]]+)`\](?![(\[])")
LINK_TARGET = re.compile(r"\]\(([^)\s]+)\)|\]\[([^\]]+)\]")
LINK = re.compile(r"\[(`?)([^\]`]+)\1\](?:\(([^)\s]+)\))?")
NOT_CRATES = {"crate", "self", "super", "std", "core", "alloc"}
TAG_OPEN = re.compile(r"^<([a-z][a-z0-9-]*)>$")
SHAPE_LINE = re.compile(r"^(\s*#!\[.*\]|\s*pub use [\w:]+(?: as \w+)?;|pub mod \w+ \{|\}|\s*//.*)$")
OWNS = re.compile(r"^- item: (\S+)\s*$")
TERM = re.compile(r"^- (.+?): (.+?)\s*Owner: (\S+)\s*$")
TOUR = re.compile(r"^### Tour: (.+?)\s*$")
PAGE_FIELD = re.compile(r"^(Purpose|Core idea|Need this when|Builds on|Primer sources):\s*(.*)$")
TOUR_FIELD = re.compile(r"^- (How|What if|Why|Example|Diagram):\s*(.*)$")
PAGE_FIELDS = ("Purpose", "Core idea", "Need this when", "Builds on")
TOUR_FIELDS = ("How", "What if", "Why", "Example", "Diagram")

# Surface-check mode per facade. `xtask` runs `cargo xtask api --check` on the
# pinned nightly, `checklist` rewrites variant-field links from the inventory,
# and every other crate gets `none`, which the gate reports as not run.
SURFACE = {"promptforge": "xtask", "harness": "checklist"}


def stop(message):
    print("STOP " + message)
    sys.exit(1)


class Facade:
    """Paths for one facade crate."""

    def __init__(self, crate):
        self.crate = crate
        self.ident = crate.replace("-", "_")
        self.src_rel = f"crates/{crate}/src"
        self.src = REPO / self.src_rel
        self.lib_rs = self.src / "lib.rs"
        self.doc = Path(os.environ.get("CARGO_TARGET_DIR") or REPO / "target") / "doc" / self.ident
        self.scratch = REPO / "target" / f"{TOOL}-{crate}"
        self.plan = REPO / "tools" / TOOL / "plans" / f"{crate}.md"
        self.tool = REPO / "tools" / f"{TOOL}.md"
        self.internal_dir = REPO / "crates" / f"{crate}-internal"
        self.surface = SURFACE.get(crate, "none")
        if not self.lib_rs.exists():
            stop(f"{crate} is not a facade: there is no {self.src_rel}/lib.rs; run from the repository root")

    def lib_rs_text(self):
        return self.lib_rs.read_text(encoding="utf-8")

    def page_rel(self, page):
        return f"{self.src_rel}/{page}"

    def banned_raw(self):
        """Strings no page may contain anywhere, code included: this tool, its scratch path, the internal crates, and long dashes."""
        names = sorted(internal_crates(self))
        return ([TOOL, f"target/{TOOL}-", f"{self.crate}-internal"] + names + [n.replace("_", "-") for n in names]
                + ["\u2014", "\u2013", " -- "])


def args(usage, least, most):
    """Return the Facade named by the first argument and the remaining arguments."""
    rest = sys.argv[2:]
    if len(sys.argv) < 2 or not least <= len(rest) <= most:
        raise SystemExit("usage: python tools/cicerone/scripts/" + usage)
    return Facade(sys.argv[1]), rest


def shape_problems(f):
    """Lines of lib.rs that break the facade shape, as messages."""
    text = f.lib_rs_text()
    problems = [] if 'include_str!("lib.md")' in text else ['lib.rs has no #![doc = include_str!("lib.md")]']
    for n, line in enumerate(text.splitlines(), 1):
        if line.strip() and not SHAPE_LINE.match(line):
            problems.append(f"lib.rs line {n} is not a re-export or a documented pub mod: {line.strip()[:80]}")
    return problems


def module_map(lib_rs_text):
    """Map each `pub mod` to its include_str! page, or None; the crate root is lib.md."""
    mods = {"root": "lib.md"}
    for chunk in re.split(r"(?=^pub mod )", lib_rs_text, flags=re.M)[1:]:
        name = re.match(r"pub mod (\w+)", chunk).group(1)
        page = re.search(r'include_str!\("([^"]+)"\)', chunk)
        mods[name] = page.group(1) if page else None
    return mods


def reexports(lib_rs_text):
    """Map each facade key (module::Name or Name) to (crate ident, defined name)."""
    out = {}
    for chunk in re.split(r"(?=^pub mod )", lib_rs_text, flags=re.M):
        mod = re.match(r"pub mod (\w+)", chunk)
        prefix = f"{mod.group(1)}::" if mod else ""
        for m in re.finditer(r"pub use (\w+)::(?:\w+::)*(\w+)(?: as (\w+))?;", chunk):
            out[prefix + (m.group(3) or m.group(2))] = (m.group(1), m.group(2))
    return out


@lru_cache(maxsize=None)
def crate_dirs():
    """Map each workspace crate's Rust identifier to its repo-relative directory."""
    dirs = {}
    for manifest in (REPO / "crates").rglob("Cargo.toml"):
        if "target" in manifest.parts:
            continue
        name = re.search(r'^\[package\][^\[]*?^name\s*=\s*"([^"]+)"', manifest.read_text(encoding="utf-8"), re.M | re.S)
        if name:
            dirs[name.group(1).replace("-", "_")] = manifest.parent.relative_to(REPO).as_posix()
    return dirs


def internal_crates(f):
    """The facade's private crates by Rust identifier: every crate under crates/CRATE-internal/, or, without
    that directory, the workspace crates the facade re-exports from."""
    dirs = crate_dirs()
    if f.internal_dir.is_dir():
        root = f.internal_dir.relative_to(REPO).as_posix() + "/"
        return {ident for ident, path in dirs.items() if path.startswith(root)}
    roots = {m.group(1) for m in re.finditer(r"^\s*pub use (\w+)::", f.lib_rs_text(), re.M)}
    return {r for r in roots - NOT_CRATES - {f.ident} if r in dirs}


def public_dependencies(f):
    """The facade crates this crate depends on directly. Its reader already uses them, so its pages may name them."""
    manifest = (f.src.parent / "Cargo.toml").read_text(encoding="utf-8")
    parts = re.split(r"^\[dependencies\][ \t]*$", manifest, flags=re.M)
    section = re.split(r"^\[", parts[1], flags=re.M)[0] if len(parts) > 1 else ""
    names = {m.group(1).replace("-", "_") for m in re.finditer(r"^([A-Za-z0-9_-]+)[ \t]*[.=]", section, re.M)}
    dirs, internal, found = crate_dirs(), internal_crates(f), set()
    for name in sorted(names & set(dirs) - internal):
        lib = REPO / dirs[name] / "src" / "lib.rs"
        if lib.is_file() and 'include_str!("lib.md")' in lib.read_text(encoding="utf-8"):
            found.add(name)
    return found


@lru_cache(maxsize=None)
def _rs_files(crate_dir):
    return tuple((p.relative_to(REPO).as_posix(), p.read_text(encoding="utf-8", errors="replace"))
                 for p in sorted((REPO / crate_dir / "src").rglob("*.rs")))


def defining_files(crate_dir, name):
    """Repo-relative files under crate_dir that define `name` or hold an impl block for it."""
    pattern = re.compile(
        r"(pub(\([^)]*\))?\s+((const|async|unsafe)\s+)*(struct|enum|trait|fn|type|const|static|union)\s+"
        + re.escape(name) + r"\b)|(^\s*impl(<[^{]*?>)?\s+([\w:<>, ]+\s+for\s+)?" + re.escape(name) + r"\b)", re.M)
    return {path for path, text in _rs_files(crate_dir) if pattern.search(text)}


def defining_sources(crate_ident, name, depth=0):
    """Files that define a re-exported item, following re-exports through other facades."""
    dirs = crate_dirs()
    if crate_ident not in dirs:
        return set()
    found = defining_files(dirs[crate_ident], name)
    lib = REPO / dirs[crate_ident] / "src" / "lib.rs"
    if found or depth > 2 or not lib.exists():
        return found
    for key, (inner, defined) in reexports(lib.read_text(encoding="utf-8")).items():
        if key.split("::")[-1] == name:
            found |= defining_sources(inner, defined, depth + 1)
    return found


def page_for(module):
    return "lib.md" if module in ("root", "") else f"{module}.md"


def stem(page):
    return Path(page).name[:-3] if page.endswith(".md") else Path(page).name


def page_name(argument):
    name = Path(argument).name
    return name if name.endswith(".md") else name + ".md"


def strip_fences(text):
    out, fence = [], None
    for line in text.splitlines():
        m = FENCE.match(line)
        if m:
            if fence is None:
                fence = m.group(1)
                continue
            if line.strip().startswith(fence):
                fence = None
                continue
        if fence is None:
            out.append(line)
    return "\n".join(out)


def fenced_blocks(text):
    """Every fenced block in text, opening and closing lines included, in order."""
    out, fence, body = [], None, []
    for line in text.split("\n"):
        m = FENCE.match(line)
        if fence is None:
            if m:
                fence, body = m.group(1), [line]
        else:
            body.append(line)
            if m and line.strip() == fence:
                out.append("\n".join(body))
                fence = None
    return out


def load_pages(facade, raw=False):
    pages = {p.name: p.read_text(encoding="utf-8") for p in sorted(facade.src.glob("*.md"))}
    return pages if raw else {name: strip_fences(text) for name, text in pages.items()}


def norm(target, ident):
    target = target.strip().strip("`")
    anchor = re.match(r"^(.*)#variant\.(\w+)\.field\.(\w+)$", target)
    target = f"{anchor.group(1)}::{anchor.group(2)}::{anchor.group(3)}" if anchor else target.split("#")[0]
    target = re.sub(r"^(crate|" + re.escape(ident) + r"|super|self)::", "", target)
    target = re.sub(r"^(struct|enum|trait|fn|type|const|mod|method|field|variant)@", "", target)
    return target.rstrip("()!")


def link_targets(text, ident):
    found = {norm(m.group(1), ident) for m in LINK_BARE.finditer(text)}
    found |= {norm(m.group(1) or m.group(2), ident) for m in LINK_TARGET.finditer(text)}
    return found


def suffixes(targets):
    out = set()
    for t in targets:
        parts = t.split("::")
        out |= {"::".join(parts[i:]) for i in range(len(parts))}
    return out


def load_checklist(path, ident):
    """Return (entries, names, methods). Each entry is (module, key, line)."""
    entries, names, methods, module = [], set(), set(), "root"
    item_line = re.compile(r"^- \S+ " + re.escape(ident) + r"::(\S+)")
    for line in Path(path).read_text(encoding="utf-8").splitlines():
        head = re.match(r"^## (\S+)", line)
        if head:
            module = head.group(1)
            continue
        item = item_line.match(line)
        if item:
            key = item.group(1)
            names.add(key.split("::")[-1])
        else:
            member = re.match(r"^  - (\S+) (\S+)", line)
            if not member:
                continue
            key = member.group(2).replace(".", "::")
            if member.group(1) in ("method", "required-method"):
                methods.add(key.split("::")[-1])
        entries.append((module, key, line.strip()))
    return entries, names, methods


def inventory_items(path, ident):
    """Map each item key (Name or module::Name) to its module, kind, item line, and member lines."""
    items, current, module = {}, None, "root"
    item_line = re.compile(r"^- (\S+) " + re.escape(ident) + r"::(\S+)")
    if not Path(path).exists():
        return items
    for line in Path(path).read_text(encoding="utf-8").splitlines():
        head = re.match(r"^## (\S+)", line)
        item = item_line.match(line)
        member = re.match(r"^  - (\S+ \S+)", line)
        if head:
            module = head.group(1)
        elif item:
            current = items[item.group(2)] = {"module": module, "kind": item.group(1), "line": line, "members": {}}
        elif member and current is not None:
            current["members"][member.group(1)] = line
    return items


def blocks(text):
    """Map each line-anchored <tag> ... </tag> block name in text to the list of its bodies, in order."""
    out, name, body = {}, None, []
    for line in text.splitlines():
        if name is None:
            m = TAG_OPEN.match(line)
            if m:
                name, body = m.group(1), []
        elif line == f"</{name}>":
            out.setdefault(name, []).append("\n".join(body).strip("\n"))
            name = None
        else:
            body.append(line)
    return out


def strip_ident(path, ident):
    return re.sub(r"^(crate|" + re.escape(ident) + r")::", "", path.strip().strip("`"))


def load_plan(f):
    """Parse the crate's plan file, or return None when it does not exist."""
    if not f.plan.exists():
        return None
    to_lf(f.plan)
    tagged = blocks(f.plan.read_text(encoding="utf-8"))
    pages = {}
    for tag, bodies in tagged.items():
        if not tag.startswith("page-"):
            continue
        fields, tours, owns = {}, [], []
        for line in bodies[-1].splitlines():
            field, tour, tour_field, own = PAGE_FIELD.match(line), TOUR.match(line), TOUR_FIELD.match(line), OWNS.match(line)
            if own:
                owns.append(strip_ident(own.group(1), f.ident))
            elif tour:
                tours.append({"name": tour.group(1)})
            elif tour_field and tours:
                tours[-1][tour_field.group(1)] = tour_field.group(2).strip()
            elif field:
                fields[field.group(1)] = field.group(2).strip()
        primer = fields.get("Primer sources", "")
        pages[f"{tag[5:]}.md"] = {
            "tag": tag,
            "fields": fields,
            "owns": owns,
            "primer": [p.strip() for p in primer.split(",") if p.strip() and p.strip().lower() != "none"],
            "tours": tours,
        }
    terms = []
    for body in tagged.get("plan-terms", []):
        for line in body.splitlines():
            m = TERM.match(line)
            if m:
                terms.append({"term": m.group(1).strip(), "definition": m.group(2).strip(), "owner": m.group(3)})
    return {"pages": pages, "terms": terms, "tags": sorted(tagged)}


def load_scope(f):
    path = f.scratch / "scope.json"
    if not path.exists():
        stop("no scope.json; run survey.py first")
    return json.loads(path.read_text(encoding="utf-8"))


def scope_pages(scope):
    """The pages a run writes, in order, lib.md first."""
    pages = [p["page"] for p in scope["pages"] if p["action"] in ("new", "rewrite")]
    return sorted(pages, key=lambda p: (p != "lib.md", p))


def rewrite_variant_links(facade, wanted):
    """Point every prose link to a variant field that wanted(enum, variant, field)
    accepts at Enum#variant.Variant.field.name, the anchor on the re-exported
    enum's page. Return (pages changed, lines changed)."""

    def rewrite(match):
        tick, text, dest = match.group(1), match.group(2), match.group(3)
        target = dest if dest else text
        parts = target.split("::")
        if "#" in target or "://" in target or len(parts) < 3:
            return match.group(0)
        enum, variant, field = parts[-3], parts[-2], parts[-1]
        if not wanted(enum, variant, field):
            return match.group(0)
        return f"[{tick}{text}{tick}]({'::'.join(parts[:-2])}#variant.{variant}.field.{field})"

    total, pages = 0, 0
    for page in sorted(facade.src.glob("*.md")):
        out, fence, changed = [], None, 0
        for line in page.read_text(encoding="utf-8").split("\n"):
            m = FENCE.match(line)
            if m:
                fence = m.group(1) if fence is None else (None if line.strip().startswith(fence) else fence)
            elif fence is None:
                new = LINK.sub(rewrite, line)
                changed += new != line
                line = new
            out.append(line)
        if changed:
            write(page, "\n".join(out))
            total, pages = total + changed, pages + 1
    return pages, total


def mtime(path):
    path = Path(path)
    return path.stat().st_mtime if path.exists() else 0.0


def nonempty(path):
    path = Path(path)
    return path.exists() and path.stat().st_size > 0


def read_log(path):
    raw = Path(path).read_bytes()
    return raw.decode("utf-16") if raw[:2] in (b"\xff\xfe", b"\xfe\xff") else raw.decode("utf-8", errors="replace")


def first_errors(log, limit=20):
    return [line for line in read_log(log).splitlines() if line.lstrip().startswith("error")][:limit]


def write(path, text):
    Path(path).parent.mkdir(parents=True, exist_ok=True)
    Path(path).write_text(text, encoding="utf-8", newline="\n")


def to_lf(path):
    """Rewrite a file that a task wrote with CRLF line endings to LF, the repository's policy; return whether it changed."""
    path = Path(path)
    data = path.read_bytes() if path.exists() else b""
    if b"\r\n" not in data:
        return False
    path.write_bytes(data.replace(b"\r\n", b"\n"))
    return True


def git(*arguments, check=False):
    """Run git in the repository and return its standard output."""
    done = subprocess.run(["git", "-C", str(REPO), *arguments], capture_output=True, text=True,
                          encoding="utf-8", errors="replace")
    if check and done.returncode != 0:
        stop(f"git {' '.join(arguments)} failed: {done.stderr.strip()[:300]}")
    return done.stdout


def git_ok(*arguments):
    return subprocess.run(["git", "-C", str(REPO), *arguments], capture_output=True).returncode == 0


def start(command, log, env=None):
    """Start a command from the repository root with its output in log."""
    Path(log).parent.mkdir(parents=True, exist_ok=True)
    handle = open(log, "wb")
    process = subprocess.Popen(command, cwd=REPO, stdout=handle, stderr=subprocess.STDOUT,
                               env=dict(os.environ, **(env or {})))
    process.log_handle = handle
    return process


def finish(process):
    """Wait for a started command and return its exit code."""
    code = process.wait()
    process.log_handle.close()
    return code


def run(command, log, env=None):
    return finish(start(command, log, env))
