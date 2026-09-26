"""Reconcile a facade's pages against its public API.

usage: python tools/scripts/reconcile.py CRATE
Reads baseline.txt, checklist-head.txt, checklist-base.txt, and head-doc.log
from target/dokuman-CRATE/, which survey.py writes, and writes change-plan.md,
change-plan.json, delta-<stem>-<n>.txt batches of at most 120 entries per
page with changes, and checklist-<stem>.txt per page whose action is new,
update, or rename.

Item classes: added, removed, moved, changed, touched. Module classes: kept,
new, renamed, removed. Page actions: skip, update, new, rename, remove. Every
delta entry ends with a `do:` line naming the page edit it needs, and an entry
whose first line ends in [record] needs facts from the defining source.
Prints STOP when every page is skip.
"""
import json
import re
import sys
from pathlib import Path

sys.dont_write_bytecode = True
import check_docs  # noqa: E402
import facade  # noqa: E402

BATCH = 120
RECORD = {"added", "moved-in", "changed", "touched", "unlinked"}
DO = {
    "added": "Add a Reference entry in the position that matches the page's order. If the page already discusses this area, add one sentence there.",
    "moved-in": "Add a Reference entry in the position that matches the page's order. If the page already discusses this area, add one sentence there.",
    "removed": "Delete its Reference entry. In each sentence that mentions it, delete the mention, or name the replacement when the evidence names one.",
    "moved-out": "Delete its Reference entry, and relink every remaining mention to its new path.",
    "changed": "Revise its entry and every example that uses it to the new signature.",
    "touched": "Compare its entry with the defining source, and revise only the statements the source contradicts.",
    "unlinked": "Link it where the page discusses it, or add a Reference entry when the page does not discuss it.",
    "bare": "Turn this code span into an intra-doc link.",
    "stale-link": "Relink to the new path when one is named; otherwise delete the clause that names it.",
    "broken-link": "Relink to the target's current path, or delete the clause when the target no longer exists.",
    "module-map": "Update `Where to go next` and every sentence that lists the modules.",
}


def entry(cls, rest="", body=""):
    head = cls + (f" {rest}" if rest else "") + (" [record]" if cls in RECORD else "")
    return "\n".join(part for part in (head, body, "do: " + DO[cls]) if part)


def parse_checklist(path, ident):
    items, current, module = {}, None, "root"
    if not Path(path).exists():
        return items
    item_line = re.compile(r"^- (\S+) " + re.escape(ident) + r"::(\S+)")
    for line in Path(path).read_text(encoding="utf-8").splitlines():
        head = re.match(r"^## (\S+)", line)
        item = item_line.match(line)
        member = re.match(r"^  - (\S+ \S+)", line)
        if head:
            module = head.group(1)
        elif item:
            key = item.group(2)
            current = items[key] = {"module": module, "kind": item.group(1), "line": line, "members": {}}
        elif member and current is not None:
            current["members"][member.group(1)] = line
    return items


def crate_dirs():
    dirs = {}
    for manifest in (facade.REPO / "crates").rglob("Cargo.toml"):
        if "target" in manifest.parts:
            continue
        name = re.search(r'^\[package\][^\[]*?^name\s*=\s*"([^"]+)"', manifest.read_text(encoding="utf-8"), re.M | re.S)
        if name:
            dirs[name.group(1).replace("-", "_")] = manifest.parent.relative_to(facade.REPO).as_posix()
    return dirs


def short(key):
    return key.split("::")[-1]


def defining_files(crate_dir, name):
    """Repo-relative files that define `name` or hold an impl block for it."""
    pattern = re.compile(
        r"(pub(\([^)]*\))?\s+((const|async|unsafe)\s+)*(struct|enum|trait|fn|type|const|static|union)\s+"
        + re.escape(name) + r"\b)|(^\s*impl(<[^{]*?>)?\s+(\w+\s+for\s+)?" + re.escape(name) + r"\b)", re.M)
    found = set()
    for path in (facade.REPO / crate_dir / "src").rglob("*.rs"):
        if pattern.search(path.read_text(encoding="utf-8", errors="replace")):
            found.add(path.relative_to(facade.REPO).as_posix())
    return found


def batch_names(page, count):
    return [f"delta-{page[:-3]}-{n}.txt" for n in range(1, -(-count // BATCH) + 1)]


def reconcile(f):
    out = f.scratch
    if not (out / "baseline.txt").exists():
        facade.stop("no baseline.txt; run survey.py first")
    baseline = (out / "baseline.txt").read_text(encoding="utf-8").split()[0]
    head_list, base_list, doc_log = out / "checklist-head.txt", out / "checklist-base.txt", out / "head-doc.log"
    lib_rs = f"{f.src_rel}/lib.rs"

    head, base = parse_checklist(head_list, f.ident), parse_checklist(base_list, f.ident)
    head_mods = facade.module_map(f.lib_rs_text())
    base_mods = facade.module_map(facade.git("show", f"{baseline}:{lib_rs}")) if baseline != "none" else {"root": "lib.md"}
    if baseline == "none":
        base = {}

    moved, changed, touched = {}, {}, set()
    head_only = {k for k in head if k not in base}
    base_only = {k for k in base if k not in head}
    for old in sorted(base_only):
        match = [h for h in head_only if short(h) == short(old) and head[h]["kind"] == base[old]["kind"]]
        if len(match) == 1:
            moved[old] = match[0]
            head_only.discard(match[0])
    added, removed = head_only, base_only - set(moved)

    for key in head.keys() & base.keys():
        h, b = head[key], base[key]
        diff = [f"now: {line.strip()}" for name, line in h["members"].items() if b["members"].get(name) != line]
        diff += [f"gone: {line.strip()}" for name, line in b["members"].items() if name not in h["members"]]
        if h["line"] != b["line"]:
            diff.insert(0, f"now: {h['line'].strip()}")
        if diff:
            changed[key] = diff

    if baseline != "none":
        dirs = crate_dirs()
        files = set(facade.git("diff", "--name-only", baseline).split())
        uses = facade.reexports(f.lib_rs_text())
        for key in sorted(head.keys() & base.keys()):
            if key in changed or key not in uses or not files:
                continue
            crate, name = uses[key]
            if crate in dirs and defining_files(dirs[crate], name) & files:
                touched.add(key)

    new_mods = [m for m in head_mods if m not in base_mods]
    gone_mods = [m for m in base_mods if m not in head_mods]
    renamed = {}
    for old in gone_mods:
        old_names = {short(k) for k, v in base.items() if v["module"] == old}
        for new in new_mods:
            new_names = {short(k) for k, v in head.items() if v["module"] == new}
            if old_names and len(old_names & new_names) >= 0.8 * len(old_names):
                renamed[old] = new

    renamed_paths = {o: n for o, n in moved.items() if renamed.get(base[o]["module"]) == head[n]["module"]}
    moved = {o: n for o, n in moved.items() if o not in renamed_paths}

    def page_of(module, mods):
        return mods.get(module) or f"{module}.md"

    deltas = {}

    def add(page, text):
        deltas.setdefault(page, []).append(text)

    def with_members(item):
        return "\n".join([item["line"]] + list(item["members"].values()))

    for key in sorted(added):
        add(page_of(head[key]["module"], head_mods), entry("added", body=with_members(head[key])))
    for key in sorted(removed):
        add(page_of(base[key]["module"], base_mods), entry("removed", key))
    for old, new in sorted(moved.items()):
        add(page_of(base[old]["module"], base_mods), entry("moved-out", f"{old} -> {new}"))
        add(page_of(head[new]["module"], head_mods), entry("moved-in", f"{old} -> {new}", with_members(head[new])))
    for key, diff in sorted(changed.items()):
        add(page_of(head[key]["module"], head_mods), entry("changed", key, "\n".join(diff)))
    for key in sorted(touched):
        add(page_of(head[key]["module"], head_mods), entry("touched", "(defining source changed)", with_members(head[key])))

    pages = facade.load_pages(f)
    entries, names, methods = facade.load_checklist(head_list, f.ident)
    if baseline != "none":
        for module, _, line in check_docs.coverage_missing(pages, entries, f.ident):
            add(page_of(module, head_mods), entry("unlinked", line))
        for name, n, span in check_docs.bare_hits(pages, names, methods):
            add(name, entry("bare", f"line {n}: `{span}`"))
        gone_keys = set(removed) | set(moved) | set(renamed_paths)
        for name, text in pages.items():
            linked = facade.suffixes(facade.link_targets(text, f.ident))
            for key in sorted(gone_keys):
                if key in linked and name != page_of(base[key]["module"], base_mods):
                    target = moved.get(key) or renamed_paths.get(key)
                    add(name, entry("stale-link", key + (f" -> {target}" if target else "")))
        log = facade.read_log(doc_log).splitlines() if doc_log.exists() else []
        broken = re.compile(r"^\s*--> crates[\\/]" + re.escape(f.crate) + r"[\\/]src[\\/](\w+\.md):(\d+)")
        for i, line in enumerate(log):
            m = broken.match(line)
            if m:
                add(m.group(1), entry("broken-link", f"line {m.group(2)}: {log[i - 1].strip()}"))
    if new_mods or gone_mods:
        add("lib.md", entry("module-map", ", ".join([f"new {m}" for m in new_mods] + [f"removed {m}" for m in gone_mods])))

    plan = {"baseline": baseline, "pages": [], "renames": [], "removals": [], "link_rewrites": [], "missing_doc_attr": []}
    for module, page in head_mods.items():
        if page is None:
            plan["missing_doc_attr"].append(module)
            page = f"{module}.md"
        source = next((o for o, n in renamed.items() if n == module), None)
        if baseline == "none" or (module in new_mods and source is None):
            action = "new"
        elif source is not None:
            action = "rename"
            plan["renames"].append([page_of(source, base_mods), page])
            plan["link_rewrites"].append([f"crate::{source}", f"crate::{module}"])
        else:
            action = "update" if page in deltas else "skip"
        if action == "new" and module != "root":
            deltas[page] = [entry("added", body=with_members(v)) for v in head.values() if v["module"] == module]
        plan["pages"].append({"page": page, "module": module, "action": action, "from": source,
                              "delta": batch_names(page, len(deltas.get(page, []))),
                              "delta_count": len(deltas.get(page, []))})
    for module in gone_mods:
        if module not in renamed:
            plan["removals"].append(page_of(module, base_mods))
            plan["pages"].append({"page": page_of(module, base_mods), "module": module, "action": "remove",
                                  "from": None, "delta": [], "delta_count": 0})
    plan["link_rewrites"] += [[f"crate::{old}", f"crate::{new}"] for old, new in sorted(moved.items())]

    out.mkdir(parents=True, exist_ok=True)
    for item in plan["pages"]:
        if item["action"] in ("new", "update", "rename"):
            mine = [with_members(v) for v in head.values() if v["module"] == item["module"]]
            facade.write(out / f"checklist-{item['page'][:-3]}.txt", f"## {item['module']}\n" + "\n".join(mine) + "\n")
    for page, items in deltas.items():
        names_out = batch_names(page, len(items))
        for n, name in enumerate(names_out):
            batch = items[n * BATCH:(n + 1) * BATCH]
            facade.write(out / name, f"# delta for {page}, batch {n + 1} of {len(names_out)}\n\n" + "\n\n".join(batch) + "\n")
    facade.write(out / "change-plan.json", json.dumps(plan, indent=2))
    rows = [f"| {p['page']} | {p['action']} | {p['delta_count']} |" for p in plan["pages"]]
    summary = (f"added={len(added)} removed={len(removed)} moved={len(moved)} changed={len(changed)} "
               f"touched={len(touched)} new_modules={len(new_mods)} removed_modules={len(gone_mods)} renamed={len(renamed)}")
    facade.write(out / "change-plan.md", f"# Change plan\n\n- baseline: {baseline}\n- {summary}\n\n"
                 "| page | action | deltas |\n|---|---|---|\n" + "\n".join(rows) + "\n")

    actions = {}
    for p in plan["pages"]:
        actions[p["action"]] = actions.get(p["action"], 0) + 1
    print(f"RECONCILE baseline={baseline[:12]} {summary}")
    print("ACTIONS " + " ".join(f"{a}={n}" for a, n in sorted(actions.items())))
    busy = [p for p in plan["pages"] if p["action"] != "skip"]
    for p in busy[:12]:
        print(f"  {p['page']}: {p['action']} deltas={p['delta_count']} files={len(p['delta'])}")
    if len(busy) > 12:
        print(f"  ... {len(busy) - 12} more in {out / 'change-plan.md'}")
    if not busy:
        facade.stop("nothing to update: every page is skip")


if __name__ == "__main__":
    reconcile(facade.args("reconcile.py CRATE", 0, 0)[0])
