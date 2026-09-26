#!/usr/bin/env python3
"""Inject `import { h } from "../io"` into JSX-using .tsx files that lack it.

The project uses the classic JSX transform with jsxFactory:h, so EVERY file
containing JSX must have `h` in scope (project rule). Used after the Task #85
app.tsx split left the new card modules without it.
"""
import os
import re

BASE = r"E:\AI-workspace\gpui-perryts\ui\src"
files = [
    "app.tsx",
    "cards/bom.tsx", "cards/canvas.tsx", "cards/counter.tsx",
    "cards/delegation.tsx", "cards/extended.tsx", "cards/form.tsx",
    "cards/layout.tsx", "cards/scroll.tsx", "cards/stats.tsx",
    "cards/tasks.tsx", "cards/widgets.tsx",
]
h_re = re.compile(r"\bh\b")

for f in files:
    path = os.path.join(BASE, f)
    with open(path, encoding="utf-8", newline="") as fh:
        lines = fh.readlines()
    if any(h_re.search(ln) for ln in lines if ln.startswith("import ")):
        print(f"{f}: already has h")
        continue
    idx = next((i for i, ln in enumerate(lines) if ln.startswith("import ")), None)
    if idx is None:
        print(f"{f}: NO IMPORT LINE - skipped")
        continue
    src = lines[idx]
    m = re.search(r'from "(\.\./io|\./io)"', src)
    if m:
        new = src.replace("import { ", "import { h, ", 1)
        if new == src:
            new = re.sub(r"import \{", "import { h,", src, count=1)
        lines[idx] = new
        print(f"{f}: injected h into existing io import (line {idx + 1})")
    else:
        rel = '"../io"' if f.startswith("cards/") else '"./io"'
        lines.insert(idx, f"import {{ h }} from {rel};\n")
        print(f"{f}: added standalone h import (before line {idx + 1})")
    with open(path, "w", encoding="utf-8", newline="") as fh:
        fh.writelines(lines)
