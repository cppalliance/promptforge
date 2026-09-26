"""Point the harness pages' variant-field links at the facade enum.

usage: python tools/scripts/rewrite_variant_links_harness.py
The harness facade has no surface check, so the variant fields come from the
variant-field lines of target/dokuman-harness/checklist-head.txt, which
survey.py writes. Every prose link to one, such as `LogError::Database::source`,
is rewritten to `LogError#variant.Database.field.source`, the anchor on the
re-exported enum's page. gates.py runs this as the harness surface gate.
"""
import re
import sys

sys.dont_write_bytecode = True
import facade  # noqa: E402

FIELD = re.compile(r"^  - variant-field (\w+)::(\w+)\.(\w+)", re.M)


def run(f, out):
    checklist = f.scratch / "checklist-head.txt"
    if not checklist.exists():
        return False, "no checklist-head.txt; run survey.py first", {}
    fields = set(FIELD.findall(checklist.read_text(encoding="utf-8")))
    _, lines = facade.rewrite_variant_links(f, lambda enum, variant, field: (enum, variant, field) in fields)
    return True, f"fields={len(fields)} rewritten={lines} (no surface check for harness)", {}


if __name__ == "__main__":
    f = facade.Facade("harness")
    ok, summary, _ = run(f, f.scratch)
    print(f"SURFACE {'pass' if ok else 'fail'} {summary}")
