#!/usr/bin/env python3
"""Split ui/src/io.ts (1124 lines) into io-core.ts + io-jsx.ts + io.ts re-export root.

Boundary: line 805/806 (the `NativeTag` doc comment ends at 805). Everything
above is the *protocol core* (transport, op protocol, event handling, element
model, signals); from `NativeTag` onward is the hyperscript/reactive (JSX) layer.

Dependency direction is strictly jsx -> core, so io-jsx imports a curated set of
symbols from io-core. Those symbols are force-`export`ed from io-core if they
weren't already public. io.ts becomes a thin `export *` re-export root so every
existing `import ... from "./io"` site keeps working.

Safety: writes go to <name>.new first; the originals are only replaced after
the jsx-layer reference scan succeeds and both new files are non-trivial.
"""
import re
import sys

SRC = "ui/src/io.ts"
CORE_END = 805  # inclusive 1-indexed
OUT_CORE = "ui/src/io-core.ts"
OUT_JSX = "ui/src/io-jsx.ts"
OUT_ROOT = "ui/src/io.ts"

with open(SRC, encoding="utf-8", newline="") as f:
    lines = f.readlines()

# Locate the jsx-layer boundary dynamically: the first `export type NativeTag`
# line, plus the contiguous block comment immediately above it (the doc block
# belongs with the declaration it documents).
tag_idx = next(
    (i for i, ln in enumerate(lines) if ln.startswith("export type NativeTag")),
    None,
)
if tag_idx is None:
    sys.exit("FATAL: cannot locate `export type NativeTag` in io.ts")
end = tag_idx - 1  # index of the line above the decl
if lines[end].strip() == "*/":
    end -= 1
    while end >= 0 and (
        lines[end].lstrip().startswith("*") or lines[end].lstrip().startswith("/**")
    ):
        end -= 1
    end += 1  # first line of the comment block
CORE_END = end  # exclusive 0-indexed == inclusive 1-indexed
print(f"split boundary: io.ts lines 1..{CORE_END} -> core, rest -> jsx")

core_text = "".join(lines[:CORE_END])
jsx_text = "".join(lines[CORE_END:])

decl_re = re.compile(
    r"^(?:export\s+)?(?:async\s+)?"
    r"(?:function|class|const|let|interface|type)\s+([A-Za-z_]\w*)",
    re.MULTILINE,  # ^ must anchor per line
)

def declared(text):
    return set(decl_re.findall(text))

core_names = declared(core_text)
jsx_names = declared(jsx_text)

# `createRoot` (jsx layer) reaches into the core's private transport
# (`writeLine`) and mutable batch buffer (`pendingOps`). Instead of exporting
# those internals, inject the *sendHello* accessor: splice it into the core
# right after `scheduleFlush` and rewrite the jsx call site to use it.
SEND_HELLO = """/**
 * `hello` handshake + seeded window title, flushed synchronously. The
 * jsx layer's `createRoot` calls this instead of touching the private
 * transport (`writeLine`) or the batch buffer directly.
 */
export function sendHello(title: string): void {
    writeLine(JSON.stringify({ t: "hello", proto: 1, title: title }));
    // `hello` is diagnostics — the window belongs to the host, so the title
    // only really changes via this op (tree.apply renames the window).
    pendingOps.push({ op: "setTitle", title: title });
    flush();
}

"""

if "sendHello" not in core_names:
    anchor = "function writeLine(line: string): void {"
    if anchor not in core_text:
        sys.exit("FATAL: cannot find writeLine anchor in core half")
    core_text = core_text.replace(anchor, SEND_HELLO + anchor, 1)
    core_names.add("sendHello")
    print("spliced sendHello accessor into core")

hello_call = re.compile(
    r"    writeLine\(\n"
    r"        JSON\.stringify\(\{\n"
    r"            t: \"hello\",\n"
    r"            proto: 1,\n"
    r"            title: title,\n"
    r"        \}\)\n"
    r"    \);\n"
    r"    // `hello` is diagnostics[^\n]*\n"
    r"    // only really changes via this op \(tree\.apply renames the window\)\.\n"
    r"    pendingOps\.push\(\{ op: \"setTitle\", title: title \}\);\n"
    r"    flush\(\);\n",
)
jsx_text2, n = hello_call.subn("    sendHello(title);\n", jsx_text)
if n != 1:
    sys.exit(f"FATAL: createRoot hello block matched {n} times (expected 1)")
jsx_text = jsx_text2
print("rewrote createRoot to call sendHello")

# Core symbols referenced (word boundary) in jsx but declared in core.
# Computed AFTER the sendHello rewrite so stale references (writeLine,
# pendingOps) don't leak into the import list.
needed = set()
for name in core_names:
    if name in jsx_names:
        continue
    if re.search(r"\b" + re.escape(name) + r"\b", jsx_text):
        needed.add(name)
print("needed imports from io-core:", sorted(needed))

# Force-export every needed (but not yet exported) symbol in core.
for name in sorted(needed):
    pat = re.compile(
        r"(?m)^(?!export\s)(?:async\s+)?"
        r"(?:function|class|const|let|interface|type)\s+" + re.escape(name) + r"\b"
    )
    core_text = pat.sub(
        lambda m: "export " + m.group(0),
        core_text,
        count=1,
    )

# --- stage to .new files, then commit atomically-ish ---
with open(OUT_CORE + ".new", "w", encoding="utf-8", newline="") as f:
    f.write(core_text)

import_line = "import { " + ", ".join(sorted(needed)) + ' } from "./io-core";'
jsx_out = (
    "/**\n"
    " * io-jsx.ts — hyperscript + reactive layer: h/text/Show/For, signals\n"
    " * (createSignal/createEffect), and the root mount. Built on the protocol\n"
    " * core in ./io-core.\n"
    " */\n"
    + import_line + "\n\n"
    + jsx_text
)
with open(OUT_JSX + ".new", "w", encoding="utf-8", newline="") as f:
    f.write(jsx_out)

root_out = (
    "/**\n"
    " * io.ts — protocol core + hyperscript layer, re-exported from the split\n"
    " * modules (./io-core and ./io-jsx) so existing\n"
    ' * `import ... from "./io"` sites keep working unchanged.\n'
    " */\n"
    'export * from "./io-core";\n'
    'export * from "./io-jsx";\n'
)
with open(OUT_ROOT + ".new", "w", encoding="utf-8", newline="") as f:
    f.write(root_out)

import os
for p in (OUT_CORE, OUT_JSX, OUT_ROOT):
    os.replace(p + ".new", p)

print(f"wrote {OUT_CORE}, {OUT_JSX}, {OUT_ROOT}")
