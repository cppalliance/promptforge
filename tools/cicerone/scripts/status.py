"""Name the next steps of a Cicerone run from the files on disk.

usage: python tools/cicerone/scripts/status.py CRATE
Reads scope.json and batches.json, which survey.py writes, and every
artifact in target/cicerone-CRATE/. Each page moves through the same chain;
an artifact counts as done when it exists, is not empty, and is newer than
the inputs it was made from:

  collect -> uses -> curate-tour and curate-reference -> brief -> examples
  -> examples-test -> write -> gates -> read -> audit -> review
  -> regenerate, then read -> audit -> review -> polish (at most 3 review rounds)

Only the first round rewrites a page whole. Later rounds polish it: they
change only the lines a correction names, so a reviewed page keeps what
already works.

Module pages are written only after lib.md finishes its review, and after
voice picks the voice sample. When every page is done or blocked, the run
moves through mentions -> consistency -> consistency-split -> settle ->
gates -> final-read -> report. The final read scores each page as it
ships, and changes nothing. The crate-wide cargo steps, examples-test and
gates, are offered only when no page is still producing input for them.

Prints one NEXT line per step with work, then WAIT lines for held work, or
DONE. A page with a blocked-<stem>.txt file is skipped.
"""
import json
import sys

sys.dont_write_bytecode = True
import common  # noqa: E402

NOTE = "<!-- note:"
PAGE_CHANGING = {"examples", "examples-fix", "write", "fix", "regenerate", "polish", "settle"}
EXAMPLE_ROUNDS = 3
FIX_STREAK = 4
REVIEW_ROUNDS = 3


class Run:
    def __init__(self, f):
        self.f, self.x = f, f.scratch
        self.scope = common.load_scope(f)
        self.batches = json.loads((self.x / "batches.json").read_text(encoding="utf-8"))
        self.pages = common.scope_pages(self.scope)
        done = sorted(int(p.name[6:]) for p in self.x.glob("gates-*") if p.name[6:].isdigit() and (p / "summary.json").exists())
        self.gates_file = self.x / f"gates-{done[-1]}" / "summary.json" if done else None
        self.gates = json.loads(self.gates_file.read_text(encoding="utf-8")) if self.gates_file else None

    def t(self, name):
        return common.mtime(self.x / name)

    def gate_step(self, page):
        """None when the latest gates round is newer than the page and passes it; otherwise the step it needs."""
        if self.gates is None or common.mtime(self.gates_file) < common.mtime(self.f.src / page):
            return ("gates", [])
        failing = self.gates["failing"].get(page)
        if not failing:
            return None
        if failing["streak"] >= FIX_STREAK:
            return ("block", [page, f"still failing after {FIX_STREAK - 1} fix rounds; see gates-{self.gates['round']}"])
        return ("fix", [page, f"gates-{self.gates['round']}/failures-{common.stem(page)}.txt"])

    def page_step(self, page, lib_done):
        s, x, src = common.stem(page), self.x, self.f.src / page
        ids = [b["id"] for b in self.batches if b["page"] == page]
        facts = [f"facts-{i}.md" for i in ids]
        missing = [i for i, name in zip(ids, facts) if not common.nonempty(x / name)]
        if missing:
            return ("collect", missing)
        if self.t(f"uses-{s}.md") < max((self.t(name) for name in facts), default=0) or not common.nonempty(x / f"uses-{s}.md"):
            return ("uses", [page])
        curate = []
        if self.t(f"tours-{s}.md") < self.t(f"uses-{s}.md"):
            curate.append(("curate-tour", page))
        for i in ids:
            if not i.endswith("-primer") and self.t(f"ref-{i}.md") < self.t(f"facts-{i}.md"):
                curate.append(("curate-reference", f"{page}:facts-{i}.md->ref-{i}.md"))
        if curate:
            return ("curate", curate)
        parts = [f"tours-{s}.md"] + [f"ref-{i}.md" for i in ids if not i.endswith("-primer")]
        if self.t(f"brief-{s}.md") < max(self.t(p) for p in parts):
            return ("brief", [page])
        check = x / f"check-brief-{s}.md"
        failing_parts = sorted({line[2:].split(":", 1)[0] for line in check.read_text(encoding="utf-8").splitlines()
                                if line.startswith("- ")}) if check.exists() else []
        if failing_parts:
            return ("curate-fix", [f"{p} (failures in check-brief-{s}.md)" for p in failing_parts])
        text = src.read_text(encoding="utf-8") if src.exists() else ""
        brief, frozen, page_time = self.t(f"brief-{s}.md"), self.t(f"frozen-{s}.json"), common.mtime(src)
        written = NOTE not in text and page_time > frozen > brief
        if not written:
            if NOTE in text and page_time > brief:
                if frozen > page_time:
                    if not lib_done and page != "lib.md":
                        return ("wait-write", [page])
                    return ("write", [page])
                rounds = sorted(x.glob(f"examples-failures-{s}-*.txt"), key=common.mtime)
                if rounds and common.mtime(rounds[-1]) > page_time:
                    if len(rounds) >= EXAMPLE_ROUNDS:
                        return ("block", [page, f"examples still fail after {EXAMPLE_ROUNDS} rounds; see {rounds[-1].name}"])
                    return ("examples-fix", [page, rounds[-1].name])
                return ("examples-test", [page])
            return ("examples", [page])
        gate = self.gate_step(page)
        if gate:
            return gate
        for r in range(1, REVIEW_ROUNDS + 1):
            reader, audit, review = f"reader-{s}-{r}.md", f"audit-{s}-{r}.md", f"review-{s}-{r}.json"
            if not common.nonempty(x / review):
                if not common.nonempty(x / reader) or self.t(reader) < page_time:
                    return ("read", [f"{page} round {r}"])
                if not common.nonempty(x / audit) or self.t(audit) < self.t(reader):
                    return ("audit", [f"{page} round {r}"])
                return ("review", [f"{page} {r}"])
            if not json.loads((x / review).read_text(encoding="utf-8"))["regenerate"]:
                return ("done", [page])
            if page_time < self.t(review):
                if r == 1:
                    return ("regenerate", [f"{page} with corrections-{s}-{r}.md and {reader}"])
                return ("polish", [page, f"corrections-{s}-{r}.md"])
        return ("done", [page])

    def steps(self):
        blocked = [p for p in self.pages if (self.x / f"blocked-{common.stem(p)}.txt").exists()]
        active = [p for p in self.pages if p not in blocked]
        lib_done = "lib.md" not in active
        if not lib_done:
            lib_done = self.page_step("lib.md", True)[0] == "done"
        voice = self.x / "voice.json"
        if lib_done and (not voice.exists() or common.mtime(voice) < common.mtime(self.f.src / "lib.md")):
            if any(self.page_step(p, False)[0] == "wait-write" for p in active if p != "lib.md"):
                return {"voice": ["pick the voice sample"]}, blocked, 0
        want = {}
        for page in active:
            step, values = self.page_step(page, lib_done and voice.exists())
            if step == "curate":
                for name, value in values:
                    want.setdefault(name, []).append(value)
            else:
                want.setdefault(step, []).extend(values)
        done = len(want.pop("done", []))
        if PAGE_CHANGING & set(want):
            want.pop("gates", None)
        if {"examples", "examples-fix"} & set(want):
            want.pop("examples-test", None)
        if want:
            return want, blocked, done
        return self.finish_steps(active), blocked, done

    def finish_steps(self, active):
        x = self.x
        latest = max(common.mtime(self.f.src / p) for p in active) if active else 0
        if not (x / "mentions.json").exists():
            return {"mentions": ["gather cross-page mentions"]}
        count = json.loads((x / "mentions.json").read_text(encoding="utf-8"))["batches"]
        pending = [f"mentions-{n}.md->consistency-{n}.md" for n in range(1, count + 1)
                   if self.t(f"consistency-{n}.md") < self.t(f"mentions-{n}.md")]
        if pending:
            return {"consistency": pending}
        if not (x / "consistency.json").exists():
            return {"consistency-split": ["split the consistency files by page"]}
        settle = [p for p in json.loads((x / "consistency.json").read_text(encoding="utf-8"))["pages"]
                  if p in active and common.mtime(self.f.src / p) < self.t("consistency.json")]
        if settle:
            return {"settle": [f"{p} with fixes-{common.stem(p)}.md" for p in settle]}
        for page in active:
            gate = self.gate_step(page)
            if gate:
                return {gate[0]: gate[1] or ["run the gates"]}
        finals = {p: f"reader-{common.stem(p)}-final.md" for p in active}
        unread = [p for p, name in finals.items() if not common.nonempty(x / name) or self.t(name) < common.mtime(self.f.src / p)]
        if unread:
            return {"final-read": [f"{p} final" for p in unread]}
        newest = max([latest, common.mtime(self.gates_file) if self.gates_file else 0] + [self.t(n) for n in finals.values()])
        if self.t("report.md") < newest:
            return {"report": ["write report.md and findings.md"]}
        return {}


def main():
    f, _ = common.args("status.py CRATE", 0, 0)
    run = Run(f)
    want, blocked, done = run.steps()
    print(f"STATUS {f.crate} pages={len(run.pages)} done={done} blocked={len(blocked)}")
    lines = 0
    for step, values in want.items():
        kind = "WAIT" if step.startswith("wait") else "NEXT"
        shown = values[:8]
        more = f" +{len(values) - 8} more" if len(values) > 8 else ""
        detail = (": " + "; ".join(str(v) for v in shown) + more) if values else ""
        print(f"{kind} {step.replace('wait-', '')}{detail}")
        lines += 1
        if lines >= 18:
            break
    if not want:
        print("DONE the run is complete; report.md and findings.md are ready")


if __name__ == "__main__":
    main()
