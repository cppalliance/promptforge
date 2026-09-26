"""Shared paths and parsing for the facade documentation scripts.

A facade is a crate at crates/<crate>/ whose lib.rs holds single-item
`pub use` re-exports grouped into `pub mod` blocks, each block documented by
an `include_str!` page beside lib.rs. Every script in this directory runs
from the repository root, takes the facade crate name first, keeps its
scratch files in target/dokuman-<crate>/, and prints at most 20 lines. A line
starting with STOP ends the run.

Each script sets sys.dont_write_bytecode before importing its siblings, so
no __pycache__ directory appears in the repository.
"""
import os
import re
import subprocess
import sys
from pathlib import Path

REPO = Path.cwd()
FENCE = re.compile(r"^\s*(`{3,}|~{3,})")
LINK_BARE = re.compile(r"\[`([^`\]]+)`\](?![(\[])")
LINK_TARGET = re.compile(r"\]\(([^)\s]+)\)|\]\[([^\]]+)\]")
LINK = re.compile(r"\[(`?)([^\]`]+)\1\](?:\(([^)\s]+)\))?")
NOT_CRATES = {"crate", "self", "super", "std", "core", "alloc"}


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
        self.scratch = REPO / "target" / f"dokuman-{crate}"
        if not self.lib_rs.exists():
            stop(f"no facade crate at {self.src_rel}/lib.rs; run from the repository root")

    def lib_rs_text(self):
        return self.lib_rs.read_text(encoding="utf-8")

    def page_rel(self, page):
        return f"{self.src_rel}/{page}"

    def banned(self):
        """Strings no page may contain: this tool's name, the internal crates, and long dashes."""
        names = sorted(internal_crates(self.lib_rs_text()))
        return (["dokuman", f"{self.crate}-internal"] + names + [n.replace("_", "-") for n in names]
                + ["\u2014", "\u2013", " -- "])


def args(usage, least, most):
    """Return the Facade named by the first argument and the remaining arguments."""
    rest = sys.argv[2:]
    if len(sys.argv) < 2 or not least <= len(rest) <= most:
        raise SystemExit("usage: python tools/scripts/" + usage)
    return Facade(sys.argv[1]), rest


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


def internal_crates(lib_rs_text):
    """The crates the facade re-exports from, by Rust identifier."""
    return {m.group(1) for m in re.finditer(r"^\s*pub use (\w+)::", lib_rs_text, re.M)} - NOT_CRATES


def page_for(module):
    return "lib.md" if module in ("root", "") else f"{module}.md"


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


def read_log(path):
    raw = Path(path).read_bytes()
    return raw.decode("utf-16") if raw[:2] in (b"\xff\xfe", b"\xfe\xff") else raw.decode("utf-8", errors="replace")


def first_errors(log, limit=20):
    return [line for line in read_log(log).splitlines() if line.lstrip().startswith("error")][:limit]


def write(path, text):
    Path(path).parent.mkdir(parents=True, exist_ok=True)
    Path(path).write_text(text, encoding="utf-8", newline="\n")


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
