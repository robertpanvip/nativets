/**
 * scriptc DLL build pipeline — the REAL app graph → build/scriptc_fe.dll
 *
 * The graph is not hand-written any more: `scripts/jsx-keep-types.mjs`
 * desugars every `src/*.tsx` to TS (JSX → h()/Fragment() calls, types KEPT),
 * which is the only way scriptc's checker sees a typed graph (esbuild always
 * erases types → SC4005).
 *
 * Steps (each one proven in sc-spike; see .workbuddy/memory):
 *   1. regenerate the typed graph into ui/build/sc-graph/ and drop the
 *      dynamic-engine-only modules (runtime.ts/main.ts probe `process` and
 *      call setInterval — outside scriptc's static subset BY CONSTRUCTION;
 *      the graph starts at sc-main.ts, which imports io.ts directly)
 *   2. `scriptc build --lib --profile` (zigcc, x86_64-windows-gnu) →
 *      entry.lib.a — a gnu-flavored archive: program object + the runtime
 *      objects the profile's graph actually reaches (tree-shaken, SCR_LIB ABI)
 *   3. link a shared library: program object + archive runtime + two abort
 *      stubs for the unlinked async surface, exports via .def
 *
 * zig is REQUIRED by scriptc itself for every native link (bundled llvm
 * helper does codegen only); there is no way to produce the archive without
 * it. The MSVC host, however, never sees zig — it LoadLibrary's the DLL.
 *
 * Output: build/scriptc_fe.dll (consumed by host/src/scriptc.rs via FFI).
 */
import { execFileSync } from "node:child_process";
import { cpSync, existsSync, mkdirSync, rmSync, writeFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const here = path.dirname(fileURLToPath(import.meta.url)); // ui/
const repo = path.dirname(here);
const graphDir = path.join(here, "build", "sc-graph");
const profile = path.join(here, "build", "scriptc.profile.json");
const archive = path.join(graphDir, "entry.lib.a");
const outDll = path.join(repo, "build", "scriptc_fe.dll");
const zigExe = process.env.SCRIPTC_ZIG ?? path.join("C:", "Users", "Administrator", "zig-0.16.0", "zig.exe");
const scriptcBin = process.env.SCRIPTC_BIN
    ?? path.join("E:", "AI-workspace", "sc-spike", "node_modules", "scriptc", "dist", "main.js");
const nodeExe = process.execPath;

function run(cmd, args, opts = {}) {
    console.log(`+ ${path.basename(String(cmd))} ${args.join(" ")}`);
    execFileSync(cmd, args, { stdio: "inherit", ...opts });
}

// 1. regenerate the typed graph from src/, then strip the modules that may
//    never enter a scriptc graph (they exist for the dynamic engines).
run(nodeExe, [path.join(here, "scripts", "jsx-keep-types.mjs")], { cwd: here });

const DYNAMIC_ONLY = ["runtime.ts", "main.ts", "scriptc-entry.ts"];
for (const f of DYNAMIC_ONLY) {
    rmSync(path.join(graphDir, f), { force: true });
}
if (!existsSync(path.join(graphDir, "sc-main.ts"))) {
    throw new Error(`graph entry missing: ${path.join(graphDir, "sc-main.ts")}`);
}

// The profile lives inside ui/build/ (gitignored) so it is rebuilt from
// these constants on every run — no stale tracked copy to drift.
const profileJson = {
    profile_format: 1,
    name: "gpui-ts-frontend",
    entry: "sc-graph/sc-main.ts", // relative to the profile file's directory
    emission: "llvm",
    abi: {
        prefix: "gpts",
        // init runs the module top-level (sink wiring + createRoot(App));
        // appInit is NOT a scriptc export — gpts_init is the whole story.
        init_symbol: "gpts_init",
        sink_register_symbol: "gpts_set_panic_sink",
        collect_symbol: null,
        result_reset_symbol: null,
        callback_register_symbol: null,
    },
    exports: [
        { export: "appTick", symbol: "gpts_tick", params: [], returns: "void" },
        { export: "appPoll", symbol: "gpts_poll", params: [], returns: "string" },
        { export: "appEvent", symbol: "gpts_event", params: ["string"], returns: "void" },
        { export: "appReset", symbol: "gpts_reset", params: [], returns: "void" },
    ],
    callbacks: [],
};
writeFileSync(profile, JSON.stringify(profileJson, null, 2) + "\n");

// 2. scriptc library archive (its own typechecker gates the graph)
run(nodeExe, [
    scriptcBin,
    "build", "--lib",
    "--profile", profile,
    "-o", archive,
], {
    env: {
        ...process.env,
        PATH: `${path.dirname(zigExe)};${process.env.PATH}`,
        SCRIPTC_CC: "zigcc",
        SCRIPTC_TARGET: "x86_64-windows-gnu",
    },
});
if (!existsSync(archive)) throw new Error(`scriptc produced no archive at ${archive}`);

// 3. link the DLL from the archive's members + abort stubs
const dllDir = path.join(here, "build", "scriptc", "dll");
rmSync(dllDir, { recursive: true, force: true });
mkdirSync(dllDir, { recursive: true });
cpSync(archive, path.join(dllDir, "entry.lib.a"));

// Extract archive members (zig ar), then link everything it shipped. The
// archive already contains exactly the tree-shaken runtime the graph needs —
// self-compiled SCR_LIB sources are NOT required (that was the pre-pack
// workaround; the 0.1.3 archive is complete except the async surface).
run(zigExe, ["ar", "x", "entry.lib.a"], { cwd: dllDir });

const defFile = path.join(dllDir, "scriptc_fe.def");
writeFileSync(defFile, [
    "EXPORTS",
    "gpts_init",
    "gpts_tick",
    "gpts_poll",
    "gpts_event",
    "gpts_set_panic_sink",
    "gpts_reset",
    "",
].join("\n"));

// The two promise-settlement hooks live in the async surface (scr_async.c),
// which a v1 async-free library graph does not link. bytes-io references them
// defensively; unreachable by construction — abort loudly if ever reached.
writeFileSync(path.join(dllDir, "scr_opt_stubs.c"), [
    "/* Auto-generated by build-scriptc.mjs — see header comment. */",
    "#include <stdlib.h>",
    "void scr_promise_settled_ref(void) { abort(); }",
    "void scr_promise_settled_void(void) { abort(); }",
    "",
].join("\n"));

const members = execFileSync(zigExe, ["ar", "t", "entry.lib.a"], { cwd: dllDir })
    .toString().split(/\r?\n/).filter((m) => m.endsWith(".o"));
const objects = members.map((m) => m.replace(/\.o$/, ".o"));

mkdirSync(path.dirname(outDll), { recursive: true });
run(zigExe, [
    "cc", "-shared", "-target", "x86_64-windows-gnu",
    "-o", outDll,
    ...objects,
    "scr_opt_stubs.c",
    "scriptc_fe.def",
    "-lws2_32", "-liphlpapi", "-ladvapi32", "-lbcrypt", "-lntdll",
    "-luser32", "-lshell32", "-lole32",
], { cwd: dllDir });

console.log(`\nOK  ${outDll}`);
