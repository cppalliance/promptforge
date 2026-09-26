"""Point the promptforge pages' rejected variant-field links at the facade enum.

usage: python tools/scripts/rewrite_variant_links_promptforge.py [<out-dir>]
Runs the facade surface check, `cargo +<nightly> xtask api --check`, on the
nightly that crates/build-xtask/src/api/toolchain.rs pins, and reports it as
not run when that nightly is not installed. The check rejects a link such as
`Enum::Variant::field`, because it resolves to the internal crate. Each
rejected link is rewritten to `Enum#variant.Variant.field.<field>`, which
targets the re-exported enum, and the check runs again. gates.py runs this as
the promptforge surface gate; its logs go to <out-dir>, by default
target/dokuman-promptforge/surface/.
"""
import re
import subprocess
import sys
from pathlib import Path

sys.dont_write_bytecode = True
import check_docs  # noqa: E402
import facade  # noqa: E402

TOOLCHAIN = "crates/build-xtask/src/api/toolchain.rs"
ALIASES = {"ClientError": "Error", "ClientTimeout": "Timeout"}
TRIPLE = re.compile(r"mentions `[\w:]*::([A-Z]\w*)::([A-Z]\w*)::(\w+)` through its doc link")
VIOLATIONS = re.compile(r"api: (\d+) violations")


def nightly():
    return re.search(r'nightly:\s*"([^"]+)"', (facade.REPO / TOOLCHAIN).read_text(encoding="utf-8")).group(1)


def installed(toolchain):
    listed = subprocess.run(["rustup", "toolchain", "list"], capture_output=True, text=True).stdout
    return any(line.split()[0].startswith(toolchain) for line in listed.splitlines() if line.strip())


def check(toolchain, log):
    code = facade.run(["cargo", f"+{toolchain}", "xtask", "api", "--check"], log)
    found = VIOLATIONS.search(facade.read_log(log))
    return code, found.group(1) if found else "unknown"


def run(f, out):
    toolchain = nightly()
    if not installed(toolchain):
        return None, f"not run: install {toolchain}", {}
    log = out / "api.log"
    code, count = check(toolchain, log)
    triples = set(TRIPLE.findall(facade.read_log(log)))
    _, lines = facade.rewrite_variant_links(f, lambda enum, variant, field: (ALIASES.get(enum, enum), variant, field) in triples)
    if lines:
        log = out / "api-2.log"
        code, count = check(toolchain, log)
    return code == 0, f"violations={count} rewritten={lines}", check_docs.split_logs(f, [log], ("surface",))


if __name__ == "__main__":
    f = facade.Facade("promptforge")
    out = Path(sys.argv[1]) if len(sys.argv) > 1 else f.scratch / "surface"
    out.mkdir(parents=True, exist_ok=True)
    ok, summary, failures = run(f, out)
    print(f"SURFACE {'not-run' if ok is None else ('pass' if ok else 'fail')} {summary}")
    for page, items in sorted(failures.items())[:15]:
        print(f"  {page}: {len(items)}")
