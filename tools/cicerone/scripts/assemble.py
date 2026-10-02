"""File plumbing between Cicerone's tasks.

usage:
  python tools/cicerone/scripts/assemble.py CRATE uses <page>...
  python tools/cicerone/scripts/assemble.py CRATE brief <page>...
  python tools/cicerone/scripts/assemble.py CRATE review <page> <round>
  python tools/cicerone/scripts/assemble.py CRATE mentions
  python tools/cicerone/scripts/assemble.py CRATE consistency
  python tools/cicerone/scripts/assemble.py CRATE voice
  python tools/cicerone/scripts/assemble.py CRATE report

uses pulls each record's heading and its use:, surprise:, and default: lines
from a page's facts files into uses-<stem>.md. brief splits the tour and
reference curators' files into brief-<stem>.md and findings-<stem>.md, so a
writer never sees a finding. review splits an audit into
corrections-<stem>-<round>.md and findings-audit-<stem>-<round>.md, and
scores the matching reader file into review-<stem>-<round>.json. mentions
gathers, for each plan term and each item named on more than one page, the
paragraphs that mention it, in batches of at most 10 subjects; it reads
only pages in scope and pages that pass checks.py, since a failing page
outside the scope is due for a rewrite and is not evidence. consistency
splits the consistency files into fixes-<stem>.md per page in scope and
findings-consistency.md. voice copies the first tour of lib.md to
voice-sample.md when lib.md passes checks.py. report writes report.md and
findings.md.
"""
import json
import re
import sys

sys.dont_write_bytecode = True
import checks  # noqa: E402
import common  # noqa: E402

KEEP_LABELS = ("use", "surprise", "default")
SUBJECTS_PER_BATCH = 10
MENTIONS_PER_SUBJECT = 25
CATEGORIES = ("bug", "doc-code mismatch", "unbuilt", "internal", "rationale", "test method", "message text",
              "unanswered", "outside scope")


def tagged(path, *tags):
    """The body of each tag in the file, stopping the run when one is missing."""
    if not path.exists():
        common.stop(f"{path.name} does not exist")
    found = common.blocks(path.read_text(encoding="utf-8"))
    missing = [t for t in tags if t not in found]
    if missing:
        common.stop(f"{path.name} lacks " + ", ".join(f"<{t}>" for t in missing))
    return [found[t][-1] for t in tags]


def bullets(body):
    return [line for line in body.splitlines() if line.startswith("- ") and line.strip() != "- none"]


def facts_files(f, page):
    return sorted(f.scratch.glob(f"facts-{common.stem(page)}-*.md"), key=lambda p: (p.stem.endswith("primer"), p.name))


def uses(f, pages):
    for page in pages:
        out = [f"# Uses for {page}"]
        for path in facts_files(f, page):
            record, kept = None, []
            for line in path.read_text(encoding="utf-8").splitlines() + ["## end"]:
                if line.startswith("## "):
                    if record and kept:
                        out += ["", record] + kept
                    record, kept = f"{line} ({path.name})", []
                    continue
                label = re.match(r"^- (\w+):", line)
                if record and label and label.group(1) in KEEP_LABELS:
                    kept.append(line)
        common.write(f.scratch / f"uses-{common.stem(page)}.md", "\n".join(out) + "\n")
        print(f"USES {page} lines={len(out)}")


def brief(f, pages):
    for page in pages:
        s = common.stem(page)
        parts = [f.scratch / f"tours-{s}.md"] + sorted(f.scratch.glob(f"ref-{s}-*.md"))
        briefs, findings = [f"# Brief for {page}", ""], [f"# Findings for {page}", ""]
        for path in parts:
            body, found = tagged(path, "brief-part", "findings-part")
            briefs += [body, ""]
            findings += [f"## From {path.name}", ""] + (bullets(found) or ["- none"]) + [""]
        common.write(f.scratch / f"brief-{s}.md", "\n".join(briefs))
        common.write(f.scratch / f"findings-{s}.md", "\n".join(findings))
        fails = checks.brief_failures(f, page)
        common.write(f.scratch / f"check-brief-{s}.md", f"# checks for brief-{s}.md\n\n" + "".join(f"- {x}\n" for x in fails))
        print(f"BRIEF {page} parts={len(parts)} checks={'pass' if not fails else 'fail ' + str(len(fails))}")


def reading(path):
    """A reader file's goal counts, its partial and missed goal lines, and the places the reader got lost."""
    goals, lost = tagged(path, "goals", "lost")
    score = {kind: len(re.findall(r"^- G\d+ " + kind + r"\b", goals, re.M)) for kind in ("answered", "partial", "missed")}
    gaps = [line for line in bullets(goals) if re.match(r"^- G\d+ (partial|missed)\b", line)]
    score["lost"] = len(bullets(lost))
    return score, gaps, bullets(lost)


def review(f, page, number):
    s = common.stem(page)
    corrections, found = tagged(f.scratch / f"audit-{s}-{number}.md", "corrections", "findings")
    common.write(f.scratch / f"corrections-{s}-{number}.md", f"# Corrections for {page}, round {number}\n\n" + corrections + "\n")
    common.write(f.scratch / f"findings-audit-{s}-{number}.md", f"# Audit findings for {page}, round {number}\n\n" + found + "\n")
    score, _, _ = reading(f.scratch / f"reader-{s}-{number}.md")
    score["corrections"] = len(bullets(corrections))
    score["regenerate"] = bool(score["corrections"] or score["lost"])
    common.write(f.scratch / f"review-{s}-{number}.json", json.dumps(score, indent=2))
    print(f"REVIEW {page} round={number} answered={score['answered']} partial={score['partial']} missed={score['missed']} "
          f"lost={score['lost']} corrections={score['corrections']} regenerate={score['regenerate']}")


def mentions(f):
    """Compare only pages worth trusting: those in this run's scope, and those that already pass checks.py."""
    plan = common.load_plan(f)
    active = set(common.scope_pages(common.load_scope(f)))
    items, _, _ = checks.inventory(f)
    trusted = [p for p in sorted(f.src.glob("*.md")) if p.name in active or not checks.page_failures(f, p.name, plan, items)]
    pages = {p.name: checks.records(p.read_text(encoding="utf-8")) for p in trusted}
    units = {page: checks.units(recs) for page, recs in pages.items()}
    subjects = [(t["term"], t["owner"], re.compile(r"(?<![\w-])" + re.escape(t["term"]) + r"s?(?![\w-])", re.I))
                for t in plan["terms"]]
    for page, entry in plan["pages"].items():
        for key in entry["owns"]:
            name = key.split("::")[-1]
            subjects.append((name, page, re.compile(r"`" + re.escape(name) + r"(?:::[\w:]+)?(?:\(\))?`")))
    batches, current = [], []
    for subject, owner, pattern in subjects:
        hits = [(page, n, text) for page, rows in units.items() for n, text in rows if pattern.search(text)]
        if len({page for page, _, _ in hits}) < 2:
            continue
        ordered = [h for h in hits if h[0] == owner][:3] + [h for h in hits if h[0] != owner]
        current.append((subject, owner, ordered[:MENTIONS_PER_SUBJECT]))
        if len(current) == SUBJECTS_PER_BATCH:
            batches.append(current)
            current = []
    if current:
        batches.append(current)
    for n, batch in enumerate(batches, 1):
        lines = [f"# Mentions, batch {n}", ""]
        for subject, owner, hits in batch:
            lines += [f"## {subject} (owner: {owner})", ""] + [f"- {page} line {line}: {text}" for page, line, text in hits] + [""]
        common.write(f.scratch / f"mentions-{n}.md", "\n".join(lines))
    common.write(f.scratch / "mentions.json", json.dumps({"batches": len(batches)}, indent=2))
    print(f"MENTIONS subjects={sum(len(b) for b in batches)} batches={len(batches)}")


def consistency(f):
    scope = common.load_scope(f)
    active = set(common.scope_pages(scope))
    count = json.loads((f.scratch / "mentions.json").read_text(encoding="utf-8"))["batches"]
    fixes, findings = {}, ["# Consistency findings", ""]
    for n in range(1, count + 1):
        corrections, found = tagged(f.scratch / f"consistency-{n}.md", "corrections", "findings")
        findings += bullets(found)
        page = None
        for line in corrections.splitlines():
            head = re.match(r"^## (\S+\.md)\s*$", line)
            if head:
                page = head.group(1)
            elif page and line.startswith("- ") and line.strip() != "- none":
                if page in active:
                    fixes.setdefault(page, []).append(line)
                else:
                    findings.append(f"- outside scope: {page}: {line[2:]}")
    for page, lines in fixes.items():
        common.write(f.scratch / f"fixes-{common.stem(page)}.md", f"# Consistency corrections for {page}\n\n" + "\n".join(lines) + "\n")
    common.write(f.scratch / "findings-consistency.md", "\n".join(findings) + "\n")
    common.write(f.scratch / "consistency.json", json.dumps({"pages": sorted(fixes)}, indent=2))
    print(f"CONSISTENCY pages={len(fixes)} findings={len(findings) - 2}")
    for page in sorted(fixes)[:15]:
        print(f"  {page}: {len(fixes[page])} corrections")


def voice(f):
    """Write voice.json naming voice-sample.md, the first tour of lib.md, or null when lib.md fails checks.py."""
    page, sample = f.src / "lib.md", None
    if page.exists() and not checks.page_failures(f, "lib.md"):
        recs = checks.records(page.read_text(encoding="utf-8"))
        heads = [h for h in checks.headings(recs) if h[1] == 1]
        tour = next((k for k, h in enumerate(heads) if h[2] not in (checks.LIB_OPEN, *checks.LIB_CLOSE)), None)
        if tour is not None:
            end = heads[tour + 1][0] if tour + 1 < len(heads) else len(recs)
            common.write(f.scratch / "voice-sample.md", "\n".join(line for _, line, _ in recs[heads[tour][0]:end]).strip() + "\n")
            sample = (f.scratch / "voice-sample.md").relative_to(common.REPO).as_posix()
    common.write(f.scratch / "voice.json", json.dumps({"sample": sample}, indent=2))
    print(f"VOICE {sample or 'none'}")


def report(f):
    scope = common.load_scope(f)
    pages = common.scope_pages(scope)
    rows, asked, answered, short, finals, fell = [], 0, 0, [], 0, 0
    for page in pages:
        s = common.stem(page)
        final = f.scratch / f"reader-{s}-final.md"
        reviews = sorted(f.scratch.glob(f"review-{s}-*.json"))
        blocked = f.scratch / f"blocked-{s}.txt"
        score = None
        if common.nonempty(final):
            score, gaps, lost = reading(final)
            finals += 1
            if gaps or lost:
                short += [f"### {page}", ""] + gaps + lost + [""]
        elif reviews:
            score = json.loads(reviews[-1].read_text(encoding="utf-8"))
        if score:
            total = score["answered"] + score["partial"] + score["missed"]
            asked, answered = asked + total, answered + score["answered"]
            status = (f"{score['answered']} of {total} goals answered, {score['partial']} partial, {score['missed']} missed, "
                      f"{score.get('lost', 0)} places where the reader got lost")
            before = json.loads(reviews[-1].read_text(encoding="utf-8"))["answered"] if reviews and common.nonempty(final) else 0
            if before > score["answered"]:
                fell += 1
                status += f"; down from {before} at the last review"
        else:
            status = "not reviewed"
        if blocked.exists():
            status += "; blocked: " + blocked.read_text(encoding="utf-8").strip().splitlines()[0][:160]
        rows.append(f"- {page}: {status}")
    done = sorted(int(p.name[6:]) for p in f.scratch.glob("gates-*") if p.name[6:].isdigit())
    gates = []
    if done:
        summary = json.loads((f.scratch / f"gates-{done[-1]}" / "summary.json").read_text(encoding="utf-8"))
        gates = [f"- {name}: {state} ({text})" for name, state, text in summary["gates"]]
        gates += [f"- still failing: {page} ({v['count']} failures)" for page, v in summary["failing"].items()]
    summary_lines = checks.summaries_report(f, required=False)
    grouped = {c: {} for c in CATEGORIES}
    grouped["other"] = {}
    seen, filed = set(), 0
    for path, owner in finding_sources(f):
        for line in bullets(path.read_text(encoding="utf-8")):
            label = re.match(r"^- ([a-z][a-z -]*?):\s", line)
            category = label.group(1) if label and label.group(1) in grouped else "other"
            filed += 1
            key = finding_key(category, line)
            if key not in seen:
                seen.add(key)
                grouped[category].setdefault(owner, []).append(line)
    unanswered = by_owner(grouped["unanswered"], "###")
    findings = ["# Findings", "",
                "Everything the curators and auditors kept off the pages, by category and page. "
                "A finding filed at several stages appears once, in its latest wording.", ""]
    for category, owners in grouped.items():
        if owners:
            findings += [f"## {category.capitalize()}", ""] + by_owner(owners, "###")
    common.write(f.scratch / "findings.md", "\n".join(findings))
    counts = ", ".join(f"{k} {sum(len(v) for v in owners.values())}" for k, owners in grouped.items() if owners) or "none"
    report_lines = [
        f"# Cicerone report: {f.crate}", "",
        f"Mode {scope['mode']}, baseline {scope['baseline']}. {answered} of {asked} learning goals answered on the "
        + ("final reading of the pages as they ship." if pages and finals == len(pages) else
           f"latest reading; {finals} of {len(pages)} pages had a final reading.")
        + (f" {fell} pages answer fewer goals than at their last review, because a later edit dropped them." if fell else ""), "",
        "## Pages", ""] + rows + [""] + (
        ["## Where the final reading fell short", ""] + (short or ["- none", ""]) if finals else []) + [
        "## Questions the evidence could not answer", ""] + (unanswered or ["- none"]) + ["",
        "## Gates", ""] + (gates or ["- not run"]) + ["",
        "## Summary lines from doc comments", "",
        f"{len(summary_lines)} rustdoc summary lines fail the phrase checks. They come from doc comments in the defining crates, so no page edit fixes them.", ""]
    report_lines += summary_lines[:40] + ["", "## Findings", "", f"By category: {counts}. The full list is in the findings file.", ""]
    common.write(f.scratch / "report.md", "\n".join(report_lines))
    kept = sum(len(v) for owners in grouped.values() for v in owners.values())
    print(f"REPORT pages={len(pages)} goals={answered}/{asked} findings={kept} filed={filed}")


CITE = re.compile(r"\[(?:code|comment|test|doc) ([^\]:\s]+):(\d+)")
ACROSS = "Across pages"


def finding_sources(f):
    """Each findings file with the page it belongs to, latest audit round first, so the kept wording is the latest."""
    ranked = []
    for path in f.scratch.glob("findings-*.md"):
        name = path.stem[len("findings-"):]
        audit = re.fullmatch(r"audit-(.+)-(\d+)", name)
        if audit:
            ranked.append(((0, -int(audit.group(2))), path, audit.group(1) + ".md"))
        elif name == "consistency":
            ranked.append(((2, 0), path, ACROSS))
        else:
            ranked.append(((1, 0), path, name + ".md"))
    return [(path, owner) for _, path, owner in sorted(ranked, key=lambda r: (r[0], r[2]))]


def finding_key(category, line):
    """Findings that share a category and a first cited line are one finding filed at several stages."""
    m = CITE.search(line)
    return (category, m.group(1), m.group(2)) if m else (category, re.sub(r"\W+", " ", line.lower()).strip())


def by_owner(owners, level):
    """The findings under one heading per page, in page order, with cross-page findings last."""
    out = []
    for owner in sorted(owners, key=lambda o: (o == ACROSS, o != "lib.md", o)):
        out += [f"{level} {owner}", ""] + owners[owner] + [""]
    return out


def main():
    f, rest = common.args("assemble.py CRATE uses|brief|review|mentions|consistency|voice|report [args]", 1, 99)
    command, values = rest[0], rest[1:]
    if command == "uses":
        uses(f, [common.page_name(p) for p in values])
    elif command == "brief":
        brief(f, [common.page_name(p) for p in values])
    elif command == "review" and len(values) == 2:
        review(f, common.page_name(values[0]), int(values[1]))
    elif command == "mentions":
        mentions(f)
    elif command == "consistency":
        consistency(f)
    elif command == "voice":
        voice(f)
    elif command == "report":
        report(f)
    else:
        raise SystemExit("usage: python tools/cicerone/scripts/assemble.py CRATE uses|brief|review|mentions|consistency|voice|report [args]")


if __name__ == "__main__":
    main()
