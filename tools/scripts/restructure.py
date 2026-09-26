"""Mechanical restructuring of a facade's pages.

usage: python tools/scripts/restructure.py CRATE
Applies target/dokuman-CRATE/change-plan.json: git mv for renamed pages, git
rm for removed pages, stubs for new pages, and intra-doc link path rewrites
for moved items and renamed modules on every page. Prints MISSING_DOC_ATTR
with each module whose `pub mod` block has no include_str! doc attribute.

survey.py imports stubs, which writes a one-line stub for every include_str!
page that lib.rs names and that does not exist, so the crate builds before
reconciliation.
"""
import json
import re
import sys

sys.dont_write_bytecode = True
import facade  # noqa: E402

STUB = "Documentation for this module is pending.\n"


def stubs(f):
    made = []
    for page in re.findall(r'include_str!\("([^"]+\.md)"\)', f.lib_rs_text()):
        if not (f.src / page).exists():
            facade.write(f.src / page, STUB)
            made.append(page)
    return made


def apply(f):
    plan_path = f.scratch / "change-plan.json"
    if not plan_path.exists():
        facade.stop("no change-plan.json; run survey.py first")
    plan = json.loads(plan_path.read_text(encoding="utf-8"))
    src, rel = f.src, f.src_rel + "/"
    for old, new in plan["renames"]:
        if (src / old).exists() and not (src / new).exists():
            facade.git("mv", rel + old, rel + new, check=True)
        elif (src / old).exists() and (src / new).read_text(encoding="utf-8") == STUB:
            (src / new).unlink()
            facade.git("mv", rel + old, rel + new, check=True)
    for page in plan["removals"]:
        if (src / page).exists():
            facade.git("rm", "-q", rel + page, check=True)
    for item in plan["pages"]:
        if item["action"] == "new" and not (src / item["page"]).exists():
            facade.write(src / item["page"], STUB)
    rewrites = [(re.compile(re.escape(old) + r"(?![\w])"), new) for old, new in plan["link_rewrites"]]
    changed = 0
    for page in sorted(src.glob("*.md")):
        text = page.read_text(encoding="utf-8")
        new_text = text
        for pattern, new in rewrites:
            new_text = pattern.sub(new, new_text)
        if new_text != text:
            facade.write(page, new_text)
            changed += 1
    print(f"APPLY renames={len(plan['renames'])} removals={len(plan['removals'])} "
          f"link_rewrites={len(rewrites)} pages_rewritten={changed}")
    if plan["missing_doc_attr"]:
        print("MISSING_DOC_ATTR " + " ".join(plan["missing_doc_attr"]))


if __name__ == "__main__":
    apply(facade.args("restructure.py CRATE", 0, 0)[0])
