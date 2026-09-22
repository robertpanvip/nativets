// TS → JS bundle for the QuickJS backend (replaces `perry compile`).
//
// esbuild bundles the TS frontend into a single IIFE so the embedded QuickJS
// engine can eval it directly. The runtime APIs runtime.ts relies on
// (process / setInterval / Promise / JSON) are host-provided globals, not
// bundled — esbuild leaves them as free globals.
//
// Usage:
//   node build-qjs.mjs                        # ui/src/main.ts → ui/dist/main.js
//   node build-qjs.mjs out.js                 # custom output
//   node build-qjs.mjs --entry foo.ts --out bar.js
//
// The output path matters: `host/src/quickjs.rs` embeds it via
// `include_str!("../../ui/dist/main.js")`, so the default must stay put for
// the single-file exe to pick up a custom entry.
import { build } from "esbuild";
import path from "node:path";
import { fileURLToPath } from "node:url";

const uiDir = path.dirname(fileURLToPath(import.meta.url));

let entry = path.join(uiDir, "src", "main.ts");
let out = path.join(uiDir, "dist", "main.js");
const positional = [];
const argv = process.argv.slice(2);
for (let i = 0; i < argv.length; i++) {
    if (argv[i] === "--entry") entry = path.resolve(argv[++i]);
    else if (argv[i] === "--out") out = path.resolve(argv[++i]);
    else if (!argv[i].startsWith("-")) positional.push(argv[i]);
}
if (positional.length > 0) out = path.resolve(positional[0]);

await build({
    entryPoints: [entry],
    bundle: true,
    format: "iife",
    target: "es2020",
    // JSX is sugar for the runtime's `h()` factory (classic transform):
    //   <div padding={16}>hi</div>  →  h("div", { padding: 16 }, "hi")
    // A `.tsx` file must import `h` (and `Fragment` if it uses `<></>`).
    jsx: "transform",
    jsxFactory: "h",
    jsxFragment: "Fragment",
    // charset: "ascii" escapes EVERY non-ASCII code point to \uXXXX — including
    // U+00D7 (×), which esbuild otherwise emits as a lone 0xD7 byte (invalid
    // UTF-8). The embedded QuickJS parser is strict about UTF-8 and rejects the
    // lone byte; CJK already got escaped, but × slipped through. ascii output
    // is valid UTF-8 and Perry-compatible too.
    charset: "ascii",
    outfile: out,
    logLevel: "warning",
    // keep `process`/`setInterval`/etc. as external globals
    define: {},
});
console.log(`[qjs] bundled ${path.relative(uiDir, entry)} → ${path.relative(uiDir, out)}`);
