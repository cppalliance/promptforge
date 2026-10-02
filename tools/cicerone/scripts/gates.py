"""Run the gates on a facade's pages and split their failures by page.

usage: python tools/cicerone/scripts/gates.py CRATE
Each run makes target/cicerone-CRATE/gates-<n>/ for its logs, one
failures-<stem>.txt per failing page, and summary.json, and runs these gates
in order, one command at a time:

- scope: every page in scope gets LF line endings, the repository's policy,
  and every page the scope skips still matches HEAD; a changed one is
  restored before the builds.
- surface: surface.py, in the crate's surface mode.
- checks: checks.py on every page in scope that its writer has written; a
  skeleton that still holds notes has no prose to check, so it builds no
  failing streak while it waits.
- examples: examples.py verify on every page in scope that has a frozen list.
- docs: cargo doc -p CRATE --no-deps with RUSTDOCFLAGS=-D warnings.
- doctests: cargo test -p CRATE --doc --no-fail-fast.

summary.json records, for each failing page, how many rounds in a row it has
failed. A surface finding in a doc comment of an internal crate goes to
failures-source.txt, since no page edit fixes it.
"""
import json
import sys

sys.dont_write_bytecode = True
import checks  # noqa: E402
import common  # noqa: E402
import examples  # noqa: E402
import surface  # noqa: E402


def scope_gate(f, scope, out):
    for page in common.scope_pages(scope):
        common.to_lf(f.src / page)
    restored = []
    for item in scope["pages"]:
        path = f.page_rel(item["page"])
        if item["action"] == "skip" and (f.src / item["page"]).exists() and not common.git_ok("diff", "--quiet", "HEAD", "--", path):
            common.git("checkout", "HEAD", "--", path, check=True)
            restored.append(item["page"])
    return not restored, " ".join([f"restored={len(restored)}"] + restored), {}


def surface_gate(f, scope, out):
    return surface.run(f, out)


def checks_gate(f, scope, out):
    plan = common.load_plan(f)
    items, _, _ = checks.inventory(f)
    written = [p for p in common.scope_pages(scope)
               if not (f.src / p).exists() or "<!-- note:" not in (f.src / p).read_text(encoding="utf-8")]
    found = {page: checks.page_failures(f, page, plan, items) for page in written}
    found = {page: fails for page, fails in found.items() if fails}
    return not found, f"pages_failing={len(found)}", found


def examples_gate(f, scope, out):
    found = {}
    for page in common.scope_pages(scope):
        if (f.scratch / f"frozen-{common.stem(page)}.json").exists():
            fails = examples.verify(f, page)
            if fails:
                found[page] = fails
    return not found, f"pages_failing={len(found)}", found


def docs_gate(f, scope, out):
    log = out / "doc.log"
    code = common.run(["cargo", "doc", "-p", f.crate, "--no-deps"], log, {"RUSTDOCFLAGS": "-D warnings"})
    return code == 0, f"exit={code}", checks.split_logs(f, [log], ("rustdoc",))


def doctests_gate(f, scope, out):
    log = out / "doctest.log"
    code = common.run(["cargo", "test", "-p", f.crate, "--doc", "--no-fail-fast"], log)
    return code == 0, f"exit={code}", checks.split_logs(f, [log], ("doctest",))


GATES = (("scope", scope_gate), ("surface", surface_gate), ("checks", checks_gate),
         ("examples", examples_gate), ("docs", docs_gate), ("doctests", doctests_gate))
LOGGED = {"surface", "docs", "doctests"}


def rounds(f):
    return sorted(int(p.name[6:]) for p in f.scratch.glob("gates-*") if p.name[6:].isdigit())


def main():
    f, _ = common.args("gates.py CRATE", 0, 0)
    scope = common.load_scope(f)
    done = rounds(f)
    number = (done[-1] if done else 0) + 1
    previous = {}
    if done:
        last = f.scratch / f"gates-{done[-1]}" / "summary.json"
        if last.exists():
            previous = json.loads(last.read_text(encoding="utf-8")).get("failing", {})
    out = f.scratch / f"gates-{number}"
    out.mkdir(parents=True)

    results, failures = [], {}
    for name, gate in GATES:
        ok, summary, found = gate(f, scope, out)
        if ok is False and not found and name in LOGGED:
            summary += " (no page named; read the logs in the round directory)"
        results.append((name, ok, summary))
        for page, items in found.items():
            failures.setdefault(page, []).extend(items)
    for page, items in failures.items():
        stem = "source" if page == checks.SOURCE else common.stem(page)
        common.write(out / f"failures-{stem}.txt", f"# failures for {page}\n\n" + "\n\n".join(items) + "\n")
    pages = sorted(p for p in failures if p != checks.SOURCE)
    failing = {page: {"count": len(failures[page]), "streak": previous.get(page, {}).get("streak", 0) + 1} for page in pages}
    common.write(out / "summary.json", json.dumps({
        "round": number, "failing": failing, "source": len(failures.get(checks.SOURCE, [])),
        "gates": [[name, "not-run" if ok is None else ("pass" if ok else "fail"), summary] for name, ok, summary in results],
    }, indent=2))

    print(f"GATES round={number} dir={out.relative_to(common.REPO).as_posix()}")
    for name, ok, summary in results:
        state = "not-run" if ok is None else ("pass" if ok else "fail")
        print(f"GATE {name} {state} {summary}"[:300])
    if checks.SOURCE in failures:
        print(f"SOURCE {len(failures[checks.SOURCE])} findings in internal doc comments; no page edit fixes them")
    if pages:
        print("FAILING " + " ".join(f"{p}(streak {failing[p]['streak']})" for p in pages))
    else:
        print("GATES " + ("pass" if all(ok is not False for _, ok, _ in results) else "fail"))


if __name__ == "__main__":
    main()
