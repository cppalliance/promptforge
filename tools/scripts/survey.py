"""Survey a facade crate for a Dokuman run.

usage: python tools/scripts/survey.py CRATE [<baseline>|none]
Builds the crate's docs at HEAD and at the baseline, in parallel and with
lints capped to warnings so an API change cannot fail the build, then builds
both coverage checklists and runs reconcile.py. The baseline defaults to the
last commit that touched crates/CRATE/src/*.md, or none when no commit did;
none skips the baseline build and makes every page new.

Stops when the pages have uncommitted changes, the baseline is unknown, or a
build fails. Clears target/dokuman-CRATE/ first, except baseline-target/, the
baseline build's cargo target directory, which later runs reuse.
"""
import shutil
import sys

sys.dont_write_bytecode = True
import build_coverage  # noqa: E402
import facade  # noqa: E402
import reconcile  # noqa: E402
import restructure  # noqa: E402

KEEP = "baseline-target"


def dirty_pages(f):
    """Uncommitted page changes, ignoring untracked stubs that an earlier survey wrote."""
    lines = []
    for line in facade.git("status", "--porcelain", "--", f"{f.src_rel}/*.md").splitlines():
        path = facade.REPO / line[3:].strip()
        if line.startswith("??") and path.exists() and path.read_text(encoding="utf-8") == restructure.STUB:
            continue
        lines.append(line)
    return lines


def resolve_baseline(f, argument):
    if argument == "none":
        return "none", "given"
    if argument:
        sha = facade.git("rev-parse", "--verify", "--quiet", argument + "^{commit}").strip()
        if not sha:
            facade.stop(f"unknown baseline {argument}")
        return sha, "given"
    sha = facade.git("log", "-1", "--format=%H", "--", f"{f.src_rel}/*.md").strip()
    return (sha, "last commit touching the pages") if sha else ("none", "no commit touches the pages")


def clear_scratch(f, worktree):
    if worktree.exists():
        facade.git("worktree", "remove", "--force", str(worktree))
    f.scratch.mkdir(parents=True, exist_ok=True)
    for path in f.scratch.iterdir():
        if path.name == KEEP:
            continue
        if path.is_dir():
            shutil.rmtree(path)
        else:
            path.unlink()
    facade.git("worktree", "prune")


def main():
    f, rest = facade.args("survey.py CRATE [<baseline>|none]", 0, 1)
    dirty = dirty_pages(f)
    if dirty:
        facade.stop("pages have uncommitted changes:\n" + "\n".join(dirty[:15]))
    worktree = f.scratch / "baseline"
    clear_scratch(f, worktree)
    baseline, reason = resolve_baseline(f, rest[0] if rest else None)
    facade.write(f.scratch / "baseline.txt", f"{baseline}\n{reason}\n")
    made = restructure.stubs(f)

    capped = {"RUSTDOCFLAGS": "--cap-lints=warn"}
    command = ["cargo", "doc", "-p", f.crate, "--no-deps"]
    base_target = f.scratch / KEEP
    jobs = [("HEAD", facade.start(command, f.scratch / "head-doc.log", capped), f.scratch / "head-doc.log")]
    if baseline != "none":
        facade.git("worktree", "add", "--detach", str(worktree), baseline, check=True)
        jobs.append(("baseline", facade.start(command + ["--manifest-path", str(worktree / "Cargo.toml")],
                                              f.scratch / "base-doc.log",
                                              dict(capped, CARGO_TARGET_DIR=str(base_target))),
                     f.scratch / "base-doc.log"))
    failed = [(name, log) for name, process, log in jobs if facade.finish(process) != 0]
    if baseline != "none":
        facade.git("worktree", "remove", "--force", str(worktree))
    for name, log in failed:
        facade.stop(f"{name} docs do not build; first errors from {log.name}:\n" + "\n".join(facade.first_errors(log)))

    print(f"SURVEY {f.crate} baseline={baseline[:12]} ({reason}) stubs={len(made)}")
    print("CHECKLIST head " + build_coverage.build(f.ident, f.doc, f.scratch / "checklist-head.txt"))
    if baseline != "none":
        print("CHECKLIST base " + build_coverage.build(f.ident, base_target / "doc" / f.ident,
                                                       f.scratch / "checklist-base.txt"))
    reconcile.reconcile(f)


if __name__ == "__main__":
    main()
