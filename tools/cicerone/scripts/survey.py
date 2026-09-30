"""Survey a facade crate at the start of a Cicerone run.

usage: python tools/cicerone/scripts/survey.py CRATE plan|build|update [--pages P1,P2] [--baseline REF|none]

Clears target/cicerone-CRATE/ (keeping baseline-target/, the baseline build's
cargo target directory), checks the facade shape, builds the crate's docs,
and writes the inventory, the item summaries, and usage.txt: the crate's own
tests plus the workspace files outside the family that call it most.

Then, by what exists:
- No plan file: prints NO PLAN and stops. The run writes the plan next.
- A plan that disagrees with the inventory: writes plan-issues.md and prints
  PLAN STALE. The run updates the plan next.
- Mode plan with a sound plan: prints PLAN OK.
- Mode build or update with a sound plan: writes scope.json, one
  batch-<id>.md per collect batch, batches.json, sources-<page>.txt, and
  state.md, and prints SCOPE.

Mode build puts every page in scope. Mode update, against the baseline
(default: the last commit that touched crates/CRATE/src/*.md), puts in scope
each page that owns a changed item, each page failing checks.py, and each
structural change. --pages narrows either scope. Stops when the pages have
uncommitted changes (modes build and update), the baseline is unknown, or a
docs build fails.
"""
import json
import os
import re
import shutil
import sys
from pathlib import Path

sys.dont_write_bytecode = True
import common  # noqa: E402
import inventory  # noqa: E402
import restructure  # noqa: E402

KEEP = "baseline-target"
MODES = ("plan", "build", "update")
BATCH = 20
PER_ITEM_SOURCES = 8
USAGE_CAP = 30
USAGE = "python tools/cicerone/scripts/survey.py CRATE plan|build|update [--pages P1,P2] [--baseline REF|none]"


def parse_args():
    if len(sys.argv) < 3 or sys.argv[2] not in MODES:
        raise SystemExit("usage: " + USAGE)
    f, mode, pages, baseline = common.Facade(sys.argv[1]), sys.argv[2], None, None
    rest = sys.argv[3:]
    while rest:
        flag = rest.pop(0)
        if flag == "--pages" and rest:
            pages = [common.page_name(p.strip()) for p in rest.pop(0).split(",") if p.strip()]
        elif flag == "--baseline" and rest:
            baseline = rest.pop(0)
        else:
            raise SystemExit(f"unknown argument {flag}; usage: {USAGE}")
    return f, mode, pages, baseline


def dirty_pages(f):
    """Uncommitted page changes, ignoring untracked stubs that an earlier survey wrote."""
    lines = []
    for line in common.git("status", "--porcelain", "--", f"{f.src_rel}/*.md").splitlines():
        path = common.REPO / line[3:].strip()
        if line.startswith("??") and path.exists() and path.read_text(encoding="utf-8") == restructure.STUB:
            continue
        lines.append(line)
    return lines


def resolve_baseline(f, argument):
    if argument == "none":
        return "none", "given"
    if argument:
        sha = common.git("rev-parse", "--verify", "--quiet", argument + "^{commit}").strip()
        if not sha:
            common.stop(f"unknown baseline {argument}")
        return sha, "given"
    sha = common.git("log", "-1", "--format=%H", "--", f"{f.src_rel}/*.md").strip()
    return (sha, "last commit touching the pages") if sha else ("none", "no commit touches the pages")


def clear_scratch(f, worktree):
    if worktree.exists():
        common.git("worktree", "remove", "--force", str(worktree))
    f.scratch.mkdir(parents=True, exist_ok=True)
    for path in f.scratch.iterdir():
        if path.name == KEEP:
            continue
        if path.is_dir():
            shutil.rmtree(path)
        else:
            path.unlink()
    common.git("worktree", "prune")


def build_docs(f, baseline, worktree):
    capped = {"RUSTDOCFLAGS": "--cap-lints=warn"}
    command = ["cargo", "doc", "-p", f.crate, "--no-deps"]
    jobs = [("HEAD", common.start(command, f.scratch / "head-doc.log", capped), f.scratch / "head-doc.log")]
    if baseline != "none":
        common.git("worktree", "add", "--detach", str(worktree), baseline, check=True)
        jobs.append(("baseline", common.start(command + ["--manifest-path", str(worktree / "Cargo.toml")],
                                              f.scratch / "base-doc.log",
                                              dict(capped, CARGO_TARGET_DIR=str(f.scratch / KEEP))),
                     f.scratch / "base-doc.log"))
    failed = [(name, log) for name, process, log in jobs if common.finish(process) != 0]
    if baseline != "none":
        common.git("worktree", "remove", "--force", str(worktree))
    for name, log in failed:
        common.stop(f"{name} docs do not build; first errors from {log.name}:\n" + "\n".join(common.first_errors(log)))


def usage_sites(f):
    """The crate's own test files, and the files outside the family and the build tooling that name it most."""
    tests_dir = common.REPO / "crates" / f.crate / "tests"
    tests = sorted(p.relative_to(common.REPO).as_posix() for p in tests_dir.rglob("*.rs")) if tests_dir.exists() else []
    own = (f"crates/{f.crate}/", f"crates/{f.crate}-internal/", "crates/build-")
    pattern = re.compile(r"\b" + re.escape(f.ident) + r"::")
    counts = []
    for root, dirs, files in os.walk(common.REPO / "crates"):
        dirs[:] = [d for d in dirs if d not in ("target", "node_modules", ".git")]
        for name in files:
            if not name.endswith(".rs"):
                continue
            path = Path(root) / name
            rel = path.relative_to(common.REPO).as_posix()
            if rel.startswith(own):
                continue
            hits = len(pattern.findall(path.read_text(encoding="utf-8", errors="replace")))
            if hits:
                counts.append((hits, rel))
    counts.sort(key=lambda c: (-c[0], c[1]))
    return tests, counts[:USAGE_CAP]


def module_pages(mods):
    return {module: page or common.page_for(module) for module, page in mods.items()}


def plan_issues(f, plan, items, mods):
    """Every way the plan disagrees with the inventory or the page rules, as one line each."""
    pages = module_pages(mods)
    issues, owners = [], {}
    for page, entry in plan["pages"].items():
        for key in entry["owns"]:
            owners.setdefault(key, []).append(page)
    for key, item in items.items():
        want = pages.get(item["module"], common.page_for(item["module"]))
        have = owners.get(key, [])
        if not have:
            issues.append(f"unowned {key}: add `- item: {f.ident}::{key}` to <page-{common.stem(want)}>")
        elif len(have) > 1:
            issues.append(f"duplicate {key}: owned by {', '.join(have)}; keep it only in {want}")
        elif have[0] != want:
            issues.append(f"misplaced {key}: owned by {have[0]}; move it to {want}, the page of its module")
    for key in sorted(set(owners) - set(items)):
        issues.append(f"stale {key}: not in the public API; remove it from {', '.join(owners[key])}")
    for page in sorted(set(pages.values()) - set(plan["pages"])):
        issues.append(f"missing <page-{common.stem(page)}> block for {page}")
    for page in sorted(set(plan["pages"]) - set(pages.values())):
        issues.append(f"stale <page-{common.stem(page)}> block: no module documents into {page}")
    for page, entry in sorted(plan["pages"].items()):
        low, high = (3, 6) if page == "lib.md" else (1, 3)
        if not low <= len(entry["tours"]) <= high:
            issues.append(f"{page} has {len(entry['tours'])} tours; it needs {low} to {high}")
        for name in common.PAGE_FIELDS:
            if not entry["fields"].get(name):
                issues.append(f"{page} lacks its `{name}:` line")
        for tour in entry["tours"]:
            for name in common.TOUR_FIELDS:
                if not tour.get(name):
                    issues.append(f"{page} tour `{tour['name']}` lacks its `- {name}:` line")
        for source in entry["primer"]:
            if not (common.REPO / source).exists():
                issues.append(f"{page} primer source {source} does not exist")
    if plan["pages"].get("lib.md") and plan["pages"]["lib.md"]["tours"]:
        if plan["pages"]["lib.md"]["tours"][-1]["name"] != "The complete program":
            issues.append("lib.md's last tour must be `The complete program`")
    for term in plan["terms"]:
        if term["owner"] not in plan["pages"]:
            issues.append(f"term `{term['term']}` names owner {term['owner']}, which has no page block")
    for tag in ("plan-reader", "plan-example", "plan-terms", "plan-links"):
        if tag not in plan["tags"]:
            issues.append(f"missing <{tag}> block")
    return issues


def diff(f, baseline, head, base, head_mods, base_mods):
    """Return ({page: [reasons]}, renames, removals, link_rewrites) for the API change since baseline."""
    reasons = {}

    def mark(page, why):
        reasons.setdefault(page, []).append(why)

    def page_of(mods, module):
        return mods.get(module) or common.page_for(module)

    head_only = {k for k in head if k not in base}
    base_only = {k for k in base if k not in head}
    moved = {}
    for old in sorted(base_only):
        match = [h for h in head_only if h.split("::")[-1] == old.split("::")[-1] and head[h]["kind"] == base[old]["kind"]]
        if len(match) == 1:
            moved[old] = match[0]
            head_only.discard(match[0])
    for key in sorted(head_only):
        mark(page_of(head_mods, head[key]["module"]), f"added {key}")
    for key in sorted(base_only - set(moved)):
        mark(page_of(base_mods, base[key]["module"]), f"removed {key}")
    for old, new in sorted(moved.items()):
        mark(page_of(base_mods, base[old]["module"]), f"moved out {old}")
        mark(page_of(head_mods, head[new]["module"]), f"moved in {new}")
    for key in sorted(head.keys() & base.keys()):
        if head[key]["line"] != base[key]["line"] or head[key]["members"] != base[key]["members"]:
            mark(page_of(head_mods, head[key]["module"]), f"changed {key}")
    files = set(common.git("diff", "--name-only", baseline).split())
    uses = common.reexports(f.lib_rs_text())
    for key in sorted(head.keys() & base.keys()):
        if key in uses and files and common.defining_sources(*uses[key]) & files:
            mark(page_of(head_mods, head[key]["module"]), f"touched {key}")

    new_mods = [m for m in head_mods if m not in base_mods]
    gone_mods = [m for m in base_mods if m not in head_mods]
    renamed = {}
    for old in gone_mods:
        old_names = {k.split("::")[-1] for k, v in base.items() if v["module"] == old}
        for new in new_mods:
            new_names = {k.split("::")[-1] for k, v in head.items() if v["module"] == new}
            if old_names and len(old_names & new_names) >= 0.8 * len(old_names):
                renamed[old] = new
    renames = [[page_of(base_mods, o), page_of(head_mods, n)] for o, n in sorted(renamed.items())]
    removals = [page_of(base_mods, m) for m in gone_mods if m not in renamed]
    link_rewrites = [[f"crate::{o}", f"crate::{n}"] for o, n in sorted(renamed.items())]
    link_rewrites += [[f"crate::{o}", f"crate::{n}"] for o, n in sorted(moved.items())]
    for module in new_mods:
        if module not in renamed.values():
            mark(page_of(head_mods, module), "new module")
    for page in removals:
        reasons.pop(page, None)
    for old, new in renames:
        reasons.pop(old, None)
        mark(new, f"renamed from {old}")
    return reasons, renames, removals, link_rewrites


def tests_for(source):
    """The sibling `<name>-tests.rs` file that a source attaches with `#[path]`, when there is one."""
    if not source.endswith(".rs") or source.endswith("-tests.rs"):
        return None
    sibling = source[:-3] + "-tests.rs"
    return sibling if (common.REPO / sibling).is_file() else None


def batches(f, plan, items, pages):
    uses = common.reexports(f.lib_rs_text())
    out = []
    for page in pages:
        entry = plan["pages"][page]
        keys = [k for k in entry["owns"] if k in items]
        for n in range(0, len(keys), BATCH):
            group, sources = keys[n:n + BATCH], []
            for key in group:
                found = sorted(common.defining_sources(*uses[key]))[:PER_ITEM_SOURCES] if key in uses else []
                sources += [s for s in found if s not in sources]
            sources += [t for t in (tests_for(s) for s in list(sources)) if t and t not in sources]
            out.append({"id": f"{common.stem(page)}-{n // BATCH + 1}", "page": page, "items": group, "sources": sources})
        if entry["primer"]:
            out.append({"id": f"{common.stem(page)}-primer", "page": page, "items": [], "sources": entry["primer"]})
    return out


def write_batches(f, work, items):
    for batch in work:
        lines = [f"# Batch {batch['id']} for {batch['page']}", "", "## Items", ""]
        if batch["items"]:
            for key in batch["items"]:
                lines += [items[key]["line"]] + list(items[key]["members"].values())
        else:
            lines.append("None. This batch collects the primer: what the reader must know about the domain before the first tour.")
        lines += ["", "## Sources", ""] + [f"- {s}" for s in batch["sources"]] + [""]
        common.write(f.scratch / f"batch-{batch['id']}.md", "\n".join(lines))
    pages = sorted({b["page"] for b in work})
    for page in pages:
        found = []
        for batch in work:
            if batch["page"] == page:
                found += [s for s in batch["sources"] if s not in found]
        common.write(f.scratch / f"sources-{common.stem(page)}.txt", "\n".join(found) + "\n")
    common.write(f.scratch / "batches.json", json.dumps(work, indent=2))


def write_goals(f, plan, pages):
    """One goals file per page in scope: the fixed purpose goal G0, then three goals per tour in plan order."""
    for page in pages:
        lines = [f"# Goals for {page}", "",
                 "- G0 (purpose): From the page's opening alone, its summary sentence and the paragraph after it, "
                 "what is this page for, and when would you need it?"]
        n = 0
        for tour in plan["pages"][page]["tours"]:
            for kind in ("How", "What if", "Why"):
                n += 1
                lines.append(f"- G{n} ({tour['name']}, {kind}): {tour[kind]}")
        common.write(f.scratch / f"goals-{common.stem(page)}.md", "\n".join(lines) + "\n")


def main():
    f, mode, only, baseline_arg = parse_args()
    problems = common.shape_problems(f)
    if problems:
        common.stop(f"{f.crate} does not have the facade shape:\n" + "\n".join(problems[:12]))
    if mode != "plan":
        dirty = dirty_pages(f)
        if dirty:
            common.stop("pages have uncommitted changes; commit or discard them first:\n" + "\n".join(dirty[:12]))
    worktree = f.scratch / "baseline"
    clear_scratch(f, worktree)
    baseline, reason = resolve_baseline(f, baseline_arg) if mode == "update" else ("none", f"mode {mode}")
    made = restructure.stubs(f)
    build_docs(f, baseline, worktree)

    head_list = f.scratch / "inventory.txt"
    print(f"SURVEY {f.crate} mode={mode} baseline={baseline[:12]} ({reason}) stubs={len(made)}")
    print("INVENTORY " + inventory.build(f.ident, f.doc, head_list, f.scratch / "summaries.txt"))
    tests, sites = usage_sites(f)
    common.write(f.scratch / "usage.txt", "# Tests\n\n" + "".join(f"- {t}\n" for t in tests)
                 + "\n# Usage sites\n\n" + "".join(f"- {path} ({hits})\n" for hits, path in sites))
    print(f"USAGE tests={len(tests)} sites={len(sites)}")

    plan = common.load_plan(f)
    if plan is None:
        print(f"NO PLAN {f.plan.relative_to(common.REPO).as_posix()} does not exist")
        return
    items = common.inventory_items(head_list, f.ident)
    mods = common.module_map(f.lib_rs_text())
    issues = plan_issues(f, plan, items, mods)
    if issues:
        common.write(f.scratch / "plan-issues.md", "# Plan issues\n\n" + "".join(f"- {i}\n" for i in issues))
        print(f"PLAN STALE {len(issues)} issues in {(f.scratch / 'plan-issues.md').relative_to(common.REPO).as_posix()}")
        for line in issues[:8]:
            print("  " + line)
        return
    if mode == "plan":
        print(f"PLAN OK pages={len(plan['pages'])} items={len(items)}")
        return

    pages = module_pages(mods)
    reasons, renames, removals, link_rewrites = {}, [], [], []
    if mode == "build":
        reasons = {page: ["build"] for page in pages.values()}
    else:
        if baseline != "none":
            base_list = f.scratch / "inventory-base.txt"
            inventory.build(f.ident, f.scratch / KEEP / "doc" / f.ident, base_list)
            base_mods = common.module_map(common.git("show", f"{baseline}:{f.src_rel}/lib.rs"))
            reasons, renames, removals, link_rewrites = diff(
                f, baseline, items, common.inventory_items(base_list, f.ident), mods, base_mods)
        import checks  # noqa: E402
        for page in sorted(pages.values()):
            if page not in reasons and (f.src / page).exists():
                found = checks.page_failures(f, page, plan, items)
                if found:
                    reasons[page] = [f"checks: {len(found)} failures"]
    if only is not None:
        reasons = {page: why for page, why in reasons.items() if page in only}

    missing_attr = [m for m, p in mods.items() if p is None]
    entries = []
    for module, page in sorted(pages.items(), key=lambda mp: (mp[1] != "lib.md", mp[1])):
        stub = not (f.src / page).exists() or (f.src / page).read_text(encoding="utf-8") == restructure.STUB
        action = ("new" if stub else "rewrite") if page in reasons else "skip"
        entries.append({"page": page, "module": module, "action": action, "reasons": reasons.get(page, [])})
    scope = {"crate": f.crate, "mode": mode, "baseline": baseline, "pages": entries, "renames": renames,
             "removals": removals, "link_rewrites": link_rewrites, "missing_doc_attr": missing_attr}
    common.write(f.scratch / "scope.json", json.dumps(scope, indent=2))
    active = common.scope_pages(scope)
    work = batches(f, plan, items, active)
    write_batches(f, work, items)
    write_goals(f, plan, active)
    common.write(f.scratch / "state.md", "\n".join([
        f"# Cicerone run: {f.crate}, mode {mode}", "",
        f"- Baseline: {baseline} ({reason})",
        f"- Pages in scope: {', '.join(active) or 'none'}",
        f"- Collect batches: {len(work)}",
        "- `lib.md` goes first when it is in scope; module pages wait for its review before they are written.", "",
        f"Run `python tools/cicerone/scripts/status.py {f.crate}` for the next step.", ""]))

    print(f"SCOPE pages={len(active)} batches={len(work)} renames={len(renames)} removals={len(removals)}")
    for entry in [e for e in entries if e["action"] != "skip"][:10]:
        print(f"  {entry['page']}: {entry['action']} ({'; '.join(entry['reasons'])[:120]})")
    if len(active) > 10:
        print(f"  ... {len(active) - 10} more in scope.json")
    if renames or removals or link_rewrites or missing_attr:
        print("RESTRUCTURE run restructure.py before collecting")
    if not active:
        print("NOTHING every page is up to date")


if __name__ == "__main__":
    main()
