/**
 * scriptc frontend build — the REAL app graph → build/scriptc/sc-main.lib.c
 *
 * The host links that C statically, together with the vendored MSVC runtime
 * (vendor/scriptc-runtime), via host/build.rs + the `cc` crate: one
 * self-contained exe on the rustc UCRT, no mingw DLL, no runtime LoadLibrary.
 * See vendor/scriptc-runtime/README.md for the embedding shape.
 *
 * Steps (each one proven in sc-spike; see .workbuddy/memory):
 *   1. regenerate the typed graph into ui/build/sc-graph/ via
 *      scripts/jsx-keep-types.mjs (JSX → h()/Fragment(), types KEPT — the only
 *      way scriptc's checker sees a typed graph; esbuild always erases types →
 *      SC4005) and drop the dynamic-engine-only modules (runtime.ts/main.ts
 *      probe `process` and call setInterval — outside scriptc's static subset
 *      BY CONSTRUCTION; the graph starts at sc-main.ts, which imports io.ts)
 *   2. `scriptc build --lib --profile` with the profile pinned to
 *      `emission:"c"`, plus --keep-c, so the generated program TU
 *      (<entry>.lib.c) is retained next to the archive.
 *
 * scriptc itself still needs zigcc to RUN its library build (it compiles the
 * archive as a by-product of the same invocation); that archive is unused —
 * we only want the C. The MSVC host never sees zig: build.rs hands the C to
 * cl.exe. There is no DLL step any more.
 *
 * Output: build/scriptc/sc-main.lib.c (consumed by host/build.rs).
 */
import { execFileSync } from "node:child_process";
import { existsSync, mkdirSync, rmSync, writeFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const here = path.dirname(fileURLToPath(import.meta.url)); // ui/
const repo = path.dirname(here);
const graphDir = path.join(here, "build", "sc-graph");
const profile = path.join(here, "build", "scriptc.profile.json");
const outDir = path.join(repo, "build", "scriptc");
const archive = path.join(outDir, "sc-main.lib.a");
const programC = path.join(outDir, "sc-main.lib.c");
const zigExe =
    process.env.SCRIPTC_ZIG ?? path.join("C:", "Users", "Administrator", "zig-0.16.0", "zig.exe");
const scriptcBin =
    process.env.SCRIPTC_BIN ??
    path.join("E:", "AI-workspace", "sc-spike", "node_modules", "scriptc", "dist", "main.js");
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

// The profile lives inside ui/build/ (gitignored) so it is rebuilt from these
// constants on every run — no stale tracked copy to drift. `emission: "c"` is
// the whole point: the library-mode program TU is emitted as C so the MSVC
// host can compile it.
const profileJson = {
    profile_format: 1,
    name: "gpui-ts-frontend",
    entry: "sc-graph/sc-main.ts", // relative to the profile file's directory
    emission: "c",
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

// 2. library build; --keep-c retains the generated program C. The archive is a
//    by-product we ignore (the host compiles the C itself, with MSVC).
mkdirSync(outDir, { recursive: true });
rmSync(programC, { force: true });
run(
    nodeExe,
    [scriptcBin, "build", "--lib", "--profile", profile, "-o", archive, "--keep-c"],
    {
        env: {
            ...process.env,
            PATH: `${path.dirname(zigExe)};${process.env.PATH}`,
            SCRIPTC_CC: "zigcc",
            SCRIPTC_TARGET: "x86_64-windows-gnu",
        },
    },
);

if (!existsSync(programC)) {
    throw new Error(
        `scriptc kept no program C at ${programC} — is --keep-c still honoured for --lib?`,
    );
}

console.log(`\nOK  ${programC}`);
console.log(`    → host/build.rs compiles it + vendor/scriptc-runtime with cl.exe`);
