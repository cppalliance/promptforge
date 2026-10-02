"""Compile, freeze, and verify the examples on a facade's pages.

usage:
  python tools/cicerone/scripts/examples.py CRATE test <page>...
  python tools/cicerone/scripts/examples.py CRATE freeze <page>...
  python tools/cicerone/scripts/examples.py CRATE verify <page>

test runs every doctest of the crate once. Each named page whose doctests
pass is frozen, as freeze does; each page with a failure gets
examples-failures-<stem>-<k>.txt, where k counts that page's failed rounds.
freeze records every fenced block of each page, in order, in
frozen-<stem>.json, and copies the page to skeleton-<stem>.md, which every
writing of the page starts from. verify fails when a page's fenced blocks
differ from its frozen list: the page keeps every block, byte for byte, in
the same order.
"""
import json
import sys

sys.dont_write_bytecode = True
import checks  # noqa: E402
import common  # noqa: E402


def freeze(f, page):
    """Record the page's fenced blocks, and keep a copy of the skeleton, the only draft a writer reads."""
    text = (f.src / page).read_text(encoding="utf-8")
    common.write(f.scratch / f"skeleton-{common.stem(page)}.md", text)
    common.write(f.scratch / f"frozen-{common.stem(page)}.json",
                 json.dumps({"page": page, "blocks": common.fenced_blocks(text)}, indent=2))


def verify(f, page):
    frozen = f.scratch / f"frozen-{common.stem(page)}.json"
    if not frozen.exists():
        return [f"no {frozen.name}; run examples.py test first"]
    want = json.loads(frozen.read_text(encoding="utf-8"))["blocks"]
    have = common.fenced_blocks((f.src / page).read_text(encoding="utf-8"))
    fails = []
    if len(have) != len(want):
        fails.append(f"{len(have)} fenced blocks, but the skeleton froze {len(want)}; keep every example and diagram")
    for n, (a, b) in enumerate(zip(have, want), 1):
        if a != b:
            first = next((i for i, (x, y) in enumerate(zip(a.split("\n"), b.split("\n"))) if x != y), None)
            where = f" at its line {first + 1}" if first is not None else ""
            fails.append(f"fenced block {n} differs from the frozen skeleton{where}; restore it byte for byte")
    return fails


def test(f, pages):
    log = f.scratch / "examples-test.log"
    code = common.run(["cargo", "test", "-p", f.crate, "--doc", "--no-fail-fast"], log)
    failures = checks.split_logs(f, [log], ("doctest",))
    if code != 0 and not failures:
        common.stop("the doctests did not build; first errors:\n" + "\n".join(common.first_errors(log)))
    passed, failed = [], []
    for page in pages:
        found = failures.get(page, [])
        if found:
            k = len(list(f.scratch.glob(f"examples-failures-{common.stem(page)}-*.txt"))) + 1
            common.write(f.scratch / f"examples-failures-{common.stem(page)}-{k}.txt",
                         f"# doctest failures for {page}, round {k}\n\n" + "\n\n".join(found) + "\n")
            failed.append(f"{page} (round {k}, {len(found)} failures)")
        else:
            freeze(f, page)
            passed.append(page)
    print(f"EXAMPLES test exit={code} passed={len(passed)} failed={len(failed)}")
    for line in failed[:15]:
        print("  failed " + line)


def main():
    f, rest = common.args("examples.py CRATE test|freeze|verify <page>...", 2, 99)
    command, pages = rest[0], [common.page_name(p) for p in rest[1:]]
    if command == "test":
        test(f, pages)
    elif command == "freeze":
        for page in pages:
            freeze(f, page)
        print(f"EXAMPLES frozen={len(pages)}")
    elif command == "verify":
        fails = [x for page in pages for x in verify(f, page)]
        print(f"EXAMPLES verify {'pass' if not fails else 'fail ' + str(len(fails))}")
        for line in fails[:15]:
            print("  " + line)
    else:
        raise SystemExit("usage: python tools/cicerone/scripts/examples.py CRATE test|freeze|verify <page>...")


if __name__ == "__main__":
    main()
