/**
 * EXPERIMENT: can the real app graph (TSX pretranspiled to TS, types kept)
 * pass scriptc's library-mode checker? Decides whether a full-UI scriptc
 * backend is reachable via the existing perry-style pretranspile step.
 *
 * Run: node scripts/sc-graph-probe.mjs   (from ui/)
 */
import { build } from "esbuild";
import { mkdirSync, rmSync, writeFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const here = path.dirname(fileURLToPath(import.meta.url)); // ui/scripts
const uiDir = path.dirname(here); // ui/
const graphDir = path.join(uiDir, "build", "sc-graph");
rmSync(graphDir, { recursive: true, force: true });
mkdirSync(graphDir, { recursive: true });

// 1. pretranspile the real graph: TSX -> TS, ESM graph kept (types survive)
await build({
    entryPoints: [path.join(uiDir, "src", "app.tsx")],
    bundle: false,
    jsx: "transform",
    jsxFactory: "h",
    jsxFragment: "Fragment",
    outdir: graphDir,
    outExtension: { ".js": ".ts" },
    logLevel: "silent",
});
console.log("[1] pretranspiled:", graphDir);

// 2. scriptc profile over the pretranspiled graph
//    exports must live in the ENTRY module — use a facade that re-exports?
//    No: SC4002 wants function declarations. app.tsx exports App; the
//    protocol exports live in scriptc-entry.ts. For the probe, point the
//    entry at app.ts and export a marker to see how far the CHECKER gets.
const profile = path.join(uiDir, "build", "sc-graph.profile.json");
writeFileSync(profile, JSON.stringify({
    profile_format: 1,
    name: "probe",
    entry: "sc-graph/app.ts",
    emission: "llvm",
    abi: {
        prefix: "gpts",
        init_symbol: "gpts_init",
        sink_register_symbol: "gpts_set_panic_sink",
        collect_symbol: null,
        result_reset_symbol: null,
        callback_register_symbol: null,
    },
    exports: [
        { export: "App", symbol: "gpts_app", params: [], returns: "string" },
    ],
    callbacks: [],
}, null, 2));

console.log("[2] profile:", profile);
console.log("[3] now run scriptc with SCRIPTC_BIN pointing at sc-spike");
