"""Build a facade's coverage checklist from its rustdoc HTML.

usage: python tools/scripts/build_coverage.py CRATE <doc-dir> <out-file>
<doc-dir> is the rustdoc output for the crate, for example target/doc/promptforge.
Each item line records its kind, facade path, and, for functions and type
aliases, its declaration. Member lines follow their item.
Trait, auto-trait, and blanket implementation sections are excluded.
"""
import html
import re
import sys
from collections import defaultdict
from pathlib import Path

sys.dont_write_bytecode = True
import facade  # noqa: E402

KIND = {"struct": "struct", "enum": "enum", "trait": "trait", "fn": "fn", "type": "type", "constant": "const"}
CUTOFFS = ['id="trait-implementations"', 'id="synthetic-implementations"',
           'id="blanket-implementations"', 'id="implementors"', 'id="foreign-impls"']
LABEL = {"structfield": "field", "variant": "variant", "variantfield": "variant-field", "method": "method",
         "tymethod": "required-method", "associatedconstant": "assoc-const", "associatedtype": "assoc-type"}


def text(fragment):
    flat = html.unescape(re.sub(r"<[^>]+>", "", fragment))
    return re.sub(r"\s+", " ", flat).replace("( ", "(").replace(", )", ")").strip()


def members(page):
    cut = min([page.find(c) for c in CUTOFFS if page.find(c) != -1] or [len(page)])
    body = page[:cut]
    found = []
    for m in re.finditer(r'<section id="(structfield|variant|method|tymethod|associatedconstant|associatedtype)\.([^"]+)"[^>]*>(.*?)</section>', body, re.S):
        tag, name, inner = m.groups()
        header = re.search(r'<h4 class="code-header">(.*?)</h4>', inner, re.S)
        found.append((tag, name, text(header.group(1)) if header else ""))
    for m in re.finditer(r'<span id="structfield\.([^"]+)" class="structfield[^"]*">(.*?)</span>', body, re.S):
        found.append(("structfield", m.group(1), text(m.group(2))))
    for m in re.finditer(r'<div class="sub-variant-field"><span id="variant\.([^"]+)\.field\.([^"]+)"[^>]*>(.*?)</span>', body, re.S):
        found.append(("variantfield", f"{m.group(1)}.{m.group(2)}", text(m.group(3))))
    seen, unique = set(), []
    for entry in found:
        if (entry[0], entry[1]) not in seen:
            seen.add((entry[0], entry[1]))
            unique.append(entry)
    return unique


def build(ident, doc, out):
    """Write the checklist for the rustdoc output in doc to out, and return a summary."""
    doc = Path(doc)
    all_html = (doc / "all.html").read_text(encoding="utf-8")
    by_module = defaultdict(list)
    for href in re.findall(r'<li><a href="([^"]+\.html)">', all_html):
        parts = href.split("/")
        module = parts[0] if len(parts) > 1 else "root"
        kind, name = parts[-1][:-5].split(".", 1)
        path = f"{ident}::" + ("" if module == "root" else module + "::") + name
        page = (doc / href).read_text(encoding="utf-8")
        decl = re.search(r'<pre class="rust item-decl"><code>(.*?)</code></pre>', page, re.S)
        by_module[module].append((KIND.get(kind, kind), path, name, members(page),
                                  text(decl.group(1)) if decl and kind in ("fn", "type") else ""))

    lines, total = [], 0
    for module in ["root"] + sorted(m for m in by_module if m != "root"):
        lines.append(f"## {module}")
        for kind, path, name, mems, decl in by_module[module]:
            total += 1
            lines.append(f"- {kind} {path}" + (f"  `{decl}`" if decl else ""))
            for tag, mname, sig in mems:
                total += 1
                sep = "." if tag == "structfield" else "::"
                show = sig and tag in ("method", "tymethod", "associatedconstant")
                lines.append(f"  - {LABEL[tag]} {name}{sep}{mname}" + (f"  `{sig}`" if show else ""))
        lines.append("")

    facade.write(out, "\n".join(lines))
    return f"entries={total} modules={len(by_module)} items={sum(len(v) for v in by_module.values())}"


if __name__ == "__main__":
    f, rest = facade.args("build_coverage.py CRATE <doc-dir> <out-file>", 2, 2)
    print(build(f.ident, rest[0], rest[1]))
