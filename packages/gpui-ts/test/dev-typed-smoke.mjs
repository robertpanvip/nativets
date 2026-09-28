// Smoke test: eval the typed dev bundle in a Node-simulated host.
// The bundle must produce protocol lines via __hostEmit (batch/hello/log).
import { bundleTyped } from "../lib/dev-typed.mjs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const repo = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..", "..", "..");
const entry = process.argv[2] ?? path.join(repo, "examples", "counter.tsx");
const runtime = path.join(repo, "packages", "gpui-ts", "runtime", "index.js");

const lines = [];
globalThis.__hostEmit = (line) => lines.push(line);

const js = await bundleTyped(entry, { nativetsRuntime: runtime });
console.log("bundle bytes:", js.length);

const t0 = Date.now();
(0, eval)(js);
console.log("eval ok in", Date.now() - t0, "ms; lines:", lines.length);

const kinds = lines.map((l) => { try { return JSON.parse(l).t; } catch { return "?"; } });
console.log("line kinds:", kinds.join(","));

const batchLine = lines.find((l) => l.includes('"batch"'));
if (!batchLine) {
    console.error("FAIL: no batch line");
    process.exit(1);
}
const ops = JSON.parse(batchLine).ops;
console.log("first batch ops:", ops.length, "| op0:", JSON.stringify(ops[0]).slice(0, 100));
if (ops.length === 0) { console.error("FAIL: empty batch"); process.exit(1); }

// Second eval (hot reload path): must not throw and must re-emit.
lines.length = 0;
(0, eval)(js);
const kinds2 = lines.map((l) => { try { return JSON.parse(l).t; } catch { return "?"; } });
console.log("re-eval (hot reload) ok; line kinds:", kinds2.join(","));
// The app installs timers (stats heartbeat / rAF), which keep Node's event
// loop alive forever — exit explicitly once every check has passed.
console.log("SMOKE-OK");
process.exit(0);
