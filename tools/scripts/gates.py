"""Run the gates on a facade's pages and split their failures by page.

usage: python tools/scripts/gates.py CRATE
Each run makes target/dokuman-CRATE/gates-<n>/ for its logs and one
failures-<stem>.txt per failing page, and runs these gates in order, one
command at a time:

- untouched: every page the change plan skips still matches HEAD; a changed
  one is restored before the builds.
- surface: rewrite_variant_links_CRATE.py.
- coverage: the check_docs.py checks over every page.
- docs: cargo doc -p CRATE --no-deps with RUSTDOCFLAGS=-D warnings.
- doctests: cargo test -p CRATE --doc --no-fail-fast.

A surface finding in a doc comment of an internal crate goes to
failures-source.txt, since no page edit fixes it.
"""
import importlib
import json
import sys

sys.dont_write_bytecode = True
import check_docs  # noqa: E402
import facade  # noqa: E402


def untouched(f, plan, out):
    restored = []
    for item in plan["pages"]:
        path = f.page_rel(item["page"])
        if item["action"] == "skip" and not facade.git_ok("diff", "--quiet", "HEAD", "--", path):
            facade.git("checkout", "HEAD", "--", path, check=True)
            restored.append(item["page"])
    return not restored, " ".join([f"restored={len(restored)}"] + restored), {}


def surface(f, plan, out):
    name = "rewrite_variant_links_" + f.ident
    try:
        module = importlib.import_module(name)
    except ModuleNotFoundError:
        return False, f"no tools/scripts/{name}.py", {}
    return module.run(f, out)


def coverage(f, plan, out):
    counts, _, per_page = check_docs.coverage(f, f.scratch / "checklist-head.txt")
    ok = not (counts["missing"] or counts["bare"] or counts["banned"])
    return ok, f"missing={counts['missing']} of {counts['entries']} bare={counts['bare']} banned={counts['banned']}", per_page


def docs(f, plan, out):
    log = out / "doc.log"
    code = facade.run(["cargo", "doc", "-p", f.crate, "--no-deps"], log, {"RUSTDOCFLAGS": "-D warnings"})
    return code == 0, f"exit={code}", check_docs.split_logs(f, [log], ("rustdoc",))


def doctests(f, plan, out):
    log = out / "doctest.log"
    code = facade.run(["cargo", "test", "-p", f.crate, "--doc", "--no-fail-fast"], log)
    return code == 0, f"exit={code}", check_docs.split_logs(f, [log], ("doctest",))


GATES = (("untouched", untouched), ("surface", surface), ("coverage", coverage), ("docs", docs), ("doctests", doctests))
LOGGED = {"surface", "docs", "doctests"}


def main():
    f, _ = facade.args("gates.py CRATE", 0, 0)
    plan_path = f.scratch / "change-plan.json"
    if not plan_path.exists():
        facade.stop("no change-plan.json; run survey.py first")
    plan = json.loads(plan_path.read_text(encoding="utf-8"))
    rounds = [int(p.name[6:]) for p in f.scratch.glob("gates-*") if p.name[6:].isdigit()]
    number = max(rounds, default=0) + 1
    out = f.scratch / f"gates-{number}"
    out.mkdir(parents=True)

    results, failures = [], {}
    for name, gate in GATES:
        ok, summary, found = gate(f, plan, out)
        if ok is False and not found and name in LOGGED:
            summary += " (no page named; read the logs in the round directory)"
        results.append((name, ok, summary))
        for page, items in found.items():
            failures.setdefault(page, []).extend(items)
    for page, items in failures.items():
        stem = "source" if page == check_docs.SOURCE else page[:-3]
        facade.write(out / f"failures-{stem}.txt", f"# failures for {page}\n\n" + "\n\n".join(items) + "\n")

    print(f"GATES round={number} dir={out}")
    for name, ok, summary in results:
        state = "not-run" if ok is None else ("pass" if ok else "fail")
        print(f"GATE {name} {state} {summary}"[:300])
    if check_docs.SOURCE in failures:
        print(f"SOURCE {len(failures[check_docs.SOURCE])} findings in internal doc comments; no page edit fixes them")
    pages = sorted(p for p in failures if p != check_docs.SOURCE)
    if pages:
        print("FAILING " + " ".join(pages))
    else:
        print("GATES " + ("pass" if all(ok is not False for _, ok, _ in results) else "fail"))


if __name__ == "__main__":
    main()
