"""Mechanical restructuring of a facade's pages.

usage: python tools/cicerone/scripts/restructure.py CRATE
Applies the structural part of target/cicerone-CRATE/scope.json: git mv for
renamed pages, git rm for pages whose module is gone, stubs for new pages,
and intra-doc link path rewrites for moved items and renamed modules on every
page. Prints MISSING_DOC_ATTR with each module whose `pub mod` block has no
include_str! doc attribute; add `#![doc = include_str!("<module>.md")]` as the
first line of that block, the only Rust edit Cicerone makes.

survey.py imports stubs, which writes a one-line stub for every include_str!
page that lib.rs names and that does not exist, so the crate builds before
the survey.
"""
import re
import sys

sys.dont_write_bytecode = True
import common  # noqa: E402

STUB = "Documentation for this module is pending.\n"


def stubs(f):
    made = []
    for page in re.findall(r'include_str!\("([^"]+\.md)"\)', f.lib_rs_text()):
        if not (f.src / page).exists():
            common.write(f.src / page, STUB)
            made.append(page)
    return made


def apply(f):
    scope = common.load_scope(f)
    src, rel = f.src, f.src_rel + "/"
    for old, new in scope["renames"]:
        if (src / old).exists() and not (src / new).exists():
            common.git("mv", rel + old, rel + new, check=True)
        elif (src / old).exists() and (src / new).read_text(encoding="utf-8") == STUB:
            (src / new).unlink()
            common.git("mv", rel + old, rel + new, check=True)
    for page in scope["removals"]:
        if (src / page).exists():
            common.git("rm", "-q", rel + page, check=True)
    for item in scope["pages"]:
        if item["action"] == "new" and not (src / item["page"]).exists():
            common.write(src / item["page"], STUB)
    rewrites = [(re.compile(re.escape(old) + r"(?![\w])"), new) for old, new in scope["link_rewrites"]]
    changed = 0
    for page in sorted(src.glob("*.md")):
        text = page.read_text(encoding="utf-8")
        new_text = text
        for pattern, new in rewrites:
            new_text = pattern.sub(new, new_text)
        if new_text != text:
            common.write(page, new_text)
            changed += 1
    print(f"APPLY renames={len(scope['renames'])} removals={len(scope['removals'])} "
          f"link_rewrites={len(rewrites)} pages_rewritten={changed}")
    if scope["missing_doc_attr"]:
        print("MISSING_DOC_ATTR " + " ".join(scope["missing_doc_attr"]))


if __name__ == "__main__":
    apply(common.args("restructure.py CRATE", 0, 0)[0])
