"""Point a facade's variant-field links at the re-exported enum, by the crate's surface mode.

usage: python tools/cicerone/scripts/surface.py CRATE [<out-dir>]
The mode comes from common.SURFACE:
- xtask runs `cargo +<nightly> xtask api --check` on the nightly that
  crates/build-xtask/src/api/toolchain.rs pins, and reports not run when that
  nightly is not installed. The check rejects a link such as
  `Enum::Variant::field`, because it resolves to the internal crate. Each
  rejected link is rewritten to `Enum#variant.Variant.field.<field>`, which
  targets the re-exported enum, and the check runs again.
- checklist rewrites every prose link to a variant field that the inventory
  lists, and has no check to run.
- none, the default, reports not run.
gates.py runs this as the surface gate. Logs go to <out-dir>, by default
target/cicerone-CRATE/surface/.
"""
import re
import subprocess
import sys
from pathlib import Path

sys.dont_write_bytecode = True
import checks  # noqa: E402
import common  # noqa: E402

TOOLCHAIN = "crates/build-xtask/src/api/toolchain.rs"
TRIPLE = re.compile(r"mentions `[\w:]*::([A-Z]\w*)::([A-Z]\w*)::(\w+)` through its doc link")
VIOLATIONS = re.compile(r"api: (\d+) violations")
FIELD = re.compile(r"^  - variant-field (\w+)::(\w+)\.(\w+)", re.M)


def nightly():
    return re.search(r'nightly:\s*"([^"]+)"', (common.REPO / TOOLCHAIN).read_text(encoding="utf-8")).group(1)


def installed(toolchain):
    listed = subprocess.run(["rustup", "toolchain", "list"], capture_output=True, text=True).stdout
    return any(line.split()[0].startswith(toolchain) for line in listed.splitlines() if line.strip())


def aliases(f):
    """Map each renamed re-export's facade name to the name it has in its defining crate."""
    return {key.split("::")[-1]: defined for key, (_, defined) in common.reexports(f.lib_rs_text()).items()
            if key.split("::")[-1] != defined}


def api_check(toolchain, log):
    code = common.run(["cargo", f"+{toolchain}", "xtask", "api", "--check"], log)
    found = VIOLATIONS.search(common.read_log(log))
    return code, found.group(1) if found else "unknown"


def xtask(f, out):
    toolchain = nightly()
    if not installed(toolchain):
        return None, f"not run: install {toolchain}", {}
    renamed = aliases(f)
    log = out / "api.log"
    code, count = api_check(toolchain, log)
    triples = set(TRIPLE.findall(common.read_log(log)))
    _, lines = common.rewrite_variant_links(f, lambda enum, variant, field: (renamed.get(enum, enum), variant, field) in triples)
    if lines:
        log = out / "api-2.log"
        code, count = api_check(toolchain, log)
    return code == 0, f"violations={count} rewritten={lines}", checks.split_logs(f, [log], ("surface",))


def checklist(f, out):
    path = f.scratch / "inventory.txt"
    if not path.exists():
        return False, "no inventory.txt; run survey.py first", {}
    fields = set(FIELD.findall(path.read_text(encoding="utf-8")))
    _, lines = common.rewrite_variant_links(f, lambda enum, variant, field: (enum, variant, field) in fields)
    return True, f"fields={len(fields)} rewritten={lines} (no surface check for {f.crate})", {}


def run(f, out):
    if f.surface == "xtask":
        return xtask(f, out)
    if f.surface == "checklist":
        return checklist(f, out)
    return None, f"not run: {f.crate} has no surface check", {}


if __name__ == "__main__":
    f, rest = common.args("surface.py CRATE [<out-dir>]", 0, 1)
    out = Path(rest[0]) if rest else f.scratch / "surface"
    out.mkdir(parents=True, exist_ok=True)
    ok, summary, failures = run(f, out)
    print(f"SURFACE {f.surface} {'not-run' if ok is None else ('pass' if ok else 'fail')} {summary}")
    for page, items in sorted(failures.items())[:15]:
        print(f"  {page}: {len(items)}")
