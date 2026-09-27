#!/usr/bin/env node
// gpui-ts (nativets) CLI — turn a TypeScript app into a native desktop window.
//
//   nativets dev  src/app.tsx    # watch → rebuild → restart the app window
//   nativets build src/app.tsx -o myapp.exe   # produce one self-contained exe
//   nativets run   src/app.tsx                # build to a temp file and run it
//   nativets doctor                           # is this install usable?
//
// No Rust, no MSVC, no codegen: the host binary ships prebuilt in `vendor/` and
// the app is appended to it as script data (see lib/build.mjs).
import { spawn, spawnSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { ESBUILD_OPTS, hostPathFor, nativetsRuntimePlugin, packExe } from "../lib/build.mjs";
import { ensureRust, ensurePerry, ensureXwinSdk, nativetsRoot } from "../lib/setup.mjs";
import { resolveToolchain, toolchainSummary } from "../lib/toolchain.mjs";

const pkgRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");

const VERSION = JSON.parse(
    fs.readFileSync(path.join(pkgRoot, "package.json"), "utf8"),
).version;

const USAGE = `nativets ${VERSION} — TypeScript → native desktop app

Usage:
  nativets dev   <entry.tsx> [-o <out.exe>]   Watch, rebuild & restart on save
  nativets build <entry.tsx> [-o <out.exe>] [--backend <name>]
                                              Bundle and pack into one exe
  nativets run   <entry.tsx>                  Pack to a temp file and run it
  nativets setup [--accept-license]           Install the AOT toolchain
  nativets doctor                             Report what this install can do
  nativets --help | --version

Backends (--backend):
  quickjs   (default) zero toolchain — the app is appended to the prebuilt
            host as script data; runs on an embedded QuickJS engine
  scriptc   native AOT — TypeScript is compiled to C and linked into the
            binary at build time. Requires the Rust/MSVC toolchain; run
            \`nativets setup\` once to install it (~5 min, ~1.4 GB)
  perry     full perry runtime linked in at build time. Same toolchain
            requirements; \`nativets setup\` covers the compiler + linker too

Your entry file is an ordinary TSX module. It must bring in the JSX factory:

  import { createRoot, h, appendChild } from "nativets";
  import { App } from "./app";

  createRoot({ title: "My App" }, App);

\`App(root)\` builds the UI with \`h\`/\`<div/>\` and the runtime's signals. It may
either mount its UI itself (\`appendChild(root, <div/>)\`) or return it — both
work, e.g. \`function App() { return <div/>; }\`.
`;

/** esbuild resolves from this package's own dependencies. */
async function loadEsbuild() {
    try {
        return await import("esbuild");
    } catch {
        // Inside the source monorepo the dependency sits next to the UI sources
        // instead of in this package's own node_modules.
        const local = path.resolve(pkgRoot, "..", "..", "ui", "node_modules", "esbuild", "lib", "main.js");
        if (fs.existsSync(local)) return await import(pathToFileURL(local).href);
        console.error("nativets: esbuild is missing — reinstall the package.");
        process.exit(2);
    }
}

function parseFlags(argv) {
    const out = { entry: null, out: null, backend: process.env.GPUI_TS_BACKEND ?? "quickjs" };
    for (let i = 0; i < argv.length; i++) {
        const a = argv[i];
        if (a === "-o" || a === "--out") {
            out.out = argv[++i];
            if (out.out === undefined) fail(`missing path after ${a}`);
        } else if (a === "--backend" || a === "-b") {
            out.backend = argv[++i];
            if (out.backend === undefined) fail(`missing backend name after ${a}`);
        } else if (!a.startsWith("-")) {
            if (out.entry === null) out.entry = a;
            else fail(`unexpected extra argument: ${a}`);
        } else {
            fail(`unknown option: ${a}`);
        }
    }
    return out;
}

function fail(msg) {
    console.error(`nativets: ${msg}`);
    console.error("run `gpui-ts --help` for usage");
    process.exit(2);
}

function requireHost() {
    const host = hostPathFor(pkgRoot);
    if (host === null) {
        console.error(
            `nativets: no prebuilt host for ${process.platform}-${process.arch}.`,
        );
        const vendor = path.join(pkgRoot, "vendor");
        const have = fs.existsSync(vendor) ? fs.readdirSync(vendor).join(", ") : "(none)";
        console.error(`  this package ships: ${have}`);
        console.error(
            "  build the host from source and drop it into vendor/<platform>-<arch>/,",
        );
        console.error("  or use a platform that is shipped.");
        process.exit(2);
    }
    return host;
}

function resolveEntry(entry) {
    const p = path.resolve(entry);
    if (!fs.existsSync(p)) fail(`entry file not found: ${entry}`);
    return p;
}

function defaultOut(entry) {
    const base = path.basename(entry).replace(/\.[jt]sx?$/, "");
    return path.join(process.cwd(), base + ".exe");
}

/** Is a command invocable on PATH? (cheap spawnSync probe) */
function haveTool(cmd) {
    const probe = process.platform === "win32" ? "where" : "command";
    const args = process.platform === "win32" ? [cmd] : ["-v", cmd];
    const r = spawnSync(probe, args, { stdio: "ignore", shell: process.platform === "win32" });
    return r.status === 0;
}

/**
 * AOT backends (scriptc / perry) bake the app into the host binary at build
 * time — they need the repo's source tree + a Rust/MSVC toolchain, not just
 * the npm package. Locate a checkout (env NATIVETS_REPO or walk up from cwd)
 * and hand off to the repo-side CLI, which drives the whole compile.
 *
 * The toolchain itself does NOT need to be on PATH: `nativets setup` installs
 * a portable tree under %USERPROFILE%\.nativets and this function injects its
 * cargo/perry/linker env into the child build.
 */
async function buildAot(entryPath, outPath, backend) {
    let repo = process.env.NATIVETS_REPO ?? null;
    if (repo === null) {
        // Walk up from the entry file looking for the repo marker.
        let dir = path.dirname(entryPath);
        for (let i = 0; i < 12; i++) {
            if (fs.existsSync(path.join(dir, "scripts", "gpui-ts.mjs"))) { repo = dir; break; }
            const parent = path.dirname(dir);
            if (parent === dir) break;
            dir = parent;
        }
    }
    if (repo === null || !fs.existsSync(path.join(repo, "scripts", "gpui-ts.mjs"))) {
        console.error(`nativets: --backend ${backend} compiles TypeScript to native code at build time.`);
        console.error("  that needs the nativets source checkout + a Rust toolchain (not just this npm package).");
        console.error("  point NATIVETS_REPO at a checkout of github.com/robertpanvip/nativets,");
        console.error("  or use the default zero-toolchain backend: nativets build <entry.tsx>");
        process.exit(2);
    }
    const tc = resolveToolchain();
    if (!tc.ok) {
        console.error(`nativets: --backend ${backend} is missing: ${tc.missing.join(", ")}`);
        console.error(`  run \`nativets setup\` to install everything into ${nativetsRoot()} (no PATH/registry changes),`);
        console.error("  or use the default quickjs backend (no toolchain).");
        process.exit(2);
    }
    const cli = path.join(repo, "scripts", "gpui-ts.mjs");
    const args = [cli, entryPath, "-o", outPath, "--backend", backend];
    console.log(`nativets: AOT build via ${backend} → ${path.relative(process.cwd(), outPath)}`);
    console.log(`  toolchain: cargo ${tc.cargo.via}, perry ${tc.perry.via}, linker ${tc.linker.mode}`);
    const r = spawnSync(process.execPath, args, {
        stdio: "inherit",
        cwd: repo,
        env: { ...process.env, ...tc.env, GPUI_TS_BACKEND: backend },
    });
    process.exit(r.status ?? (r.error ? 1 : 0));
}

async function cmdBuild(argv) {
    const { entry, out, backend } = parseFlags(argv);
    if (entry === null) fail("build needs an entry file");
    if (backend !== "quickjs" && backend !== "scriptc" && backend !== "perry") {
        fail(`unknown backend: ${backend} (quickjs | scriptc | perry)`);
    }
    const entryPath = resolveEntry(entry);
    const outPath = path.resolve(out === null ? defaultOut(entry) : out);
    if (backend !== "quickjs") {
        await buildAot(entryPath, outPath, backend);
    }
    const host = requireHost();

    const esbuild = await loadEsbuild();
    const tmp = path.join(os.tmpdir(), `gpui-ts-${process.pid}.js`);
    let js;
    try {
        js = await esbuild.build({
            ...ESBUILD_OPTS,
            entryPoints: [entryPath],
            outfile: tmp,
            plugins: [nativetsRuntimePlugin(pkgRoot)],
        }).then(() => fs.readFileSync(tmp, "utf8"));
    } finally {
        fs.rmSync(tmp, { force: true });
    }
    packExe(host, js, outPath);

    const mb = (fs.statSync(outPath).size / 1048576).toFixed(1);
    console.log(`nativets: ${path.relative(process.cwd(), outPath)}  (${mb} MB, self-contained)`);
    return outPath;
}

async function cmdRun(argv) {
    const { entry, out, backend } = parseFlags(argv);
    if (entry === null) fail("run needs an entry file");
    const tmpExe = path.join(
        os.tmpdir(),
        `gpui-ts-${path.basename(entry).replace(/\.[jt]sx?$/, "")}-${process.pid}.exe`,
    );
    await cmdBuild([entry, "-o", tmpExe, ...(backend !== "quickjs" ? ["--backend", backend] : [])]);
    try {
        const r = spawnSync(tmpExe, [], { stdio: "inherit" });
        process.exit(r.status === null ? 1 : r.status);
    } finally {
        fs.rmSync(tmpExe, { force: true });
    }
}

/**
 * `nativets dev` — the Go-like inner loop.
 *
 * esbuild's context API gives us watch mode + dependency discovery (it only
 * reports the files that actually made it into the bundle). On every rebuild
 * we repack host+bundle and restart the app window, so the user always looks
 * at a live process. Ctrl+C quits.
 */
async function cmdDev(argv) {
    const { entry, out } = parseFlags(argv);
    if (entry === null) fail("dev needs an entry file");
    const entryPath = resolveEntry(entry);
    const outPath = path.resolve(out === null ? defaultOut(entry) : out);
    const host = requireHost();
    const esbuild = await loadEsbuild();

    const tmp = path.join(os.tmpdir(), `nativets-dev-${process.pid}.js`);
    let ctx;
    try {
        ctx = await esbuild.context({
            ...ESBUILD_OPTS,
            entryPoints: [entryPath],
            outfile: tmp,
            plugins: [nativetsRuntimePlugin(pkgRoot)],
        });
    } catch (e) {
        fail(`dev: ${e.message}`);
    }

    let child = null;      // the running app (exe) process
    let restartTimer = null;
    let building = false;
    let pending = false;
    let quitting = false;
    let gen = 0;           // exe generation — never reuse a locked image name
    const spawnedExes = []; // images we launched; best-effort deleted on quit

    function scheduleRestart() {
        // Debounce: esbuild can fire several changes in quick succession
        // (editors do write+rename); rebuild once things settle for 120ms.
        if (restartTimer !== null) clearTimeout(restartTimer);
        restartTimer = setTimeout(() => {
            restartTimer = null;
            rebuildAndRestart();
        }, 120);
    }

    async function rebuildAndRestart() {
        if (quitting) return;
        if (building) { pending = true; return; }
        building = true;
        try {
            // NOTE: we deliberately never call ctx.rebuild() here. esbuild's
            // watch mode has already rewritten the outfile by the time the
            // poller notices the mtime change — and an explicit rebuild()
            // issued while the watch service is mid-rebuild can deadlock the
            // CLI (observed on Windows: the promise never settles). Reading
            // the rewritten outfile keeps us strictly a consumer of watch.
            const js = fs.readFileSync(tmp, "utf8");
            // Generation tag: if the previous exe image is still locked
            // (Windows keeps running images exclusive), packExe falls back to
            // `<out>.<gen>.<seq>.exe` next to the target instead of failing.
            gen += 1;
            const runPath = packExe(host, js, outPath, { fallbackTag: `g${gen}` });
            spawnedExes.push(runPath);
            console.log(`nativets dev: rebuilt ${new Date().toLocaleTimeString()}`);
            if (child !== null) {
                const old = child;
                child = null;          // detach first so its exit handler no-ops
                old.kill();
                // Give the window a beat to close so the new one doesn't
                // fight it for the same taskbar slot.
                await new Promise((r) => setTimeout(r, 200));
            }
            child = spawn(runPath, [], { stdio: "inherit" });
            child.on("exit", (code) => {
                if (child !== null && !child.killed) {
                    // The app quit on its own (e.g. closed by the user) —
                    // note it; watch stays alive so the next save relaunches.
                    child = null;
                    console.log(`nativets dev: app exited (${code ?? "signal"})`);
                }
            });
        } catch (e) {
            // esbuild watch keeps serving; compile errors are non-fatal here.
            console.error(`nativets dev: ${e.message.split("\n")[0]}`);
        } finally {
            building = false;
            if (pending && !quitting) { pending = false; rebuildAndRestart(); }
        }
    }

    console.log(`nativets dev ${VERSION} — watching ${path.basename(entryPath)}`);
    console.log(`  app: ${path.relative(process.cwd(), outPath)}  (Ctrl+C to quit)`);
    // esbuild's watch mode rewrites the outfile on every change to any file
    // in the bundle's dependency graph. We deliberately do NOT rely on the
    // `ctx.watch(callback)` callback: on Windows it has proven unreliable to
    // the point of never firing (esbuild 0.24), and fs.watchFile stops seeing
    // the path once esbuild replaces it via write+rename. A plain interval
    // poll of the mtime is dumb, portable, and cannot miss.
    //
    // esbuild writes the outfile with write+rename, so the path is briefly
    // absent on every rebuild — every stat must tolerate ENOENT (NaN).
    await ctx.watch();
    await rebuildAndRestart();
    const mtimeOf = () => { try { return fs.statSync(tmp).mtimeMs; } catch { return NaN; } };
    let lastMtime = mtimeOf();
    const poller = setInterval(() => {
        const m = mtimeOf();
        if (!Number.isNaN(m) && m !== lastMtime) {
            lastMtime = m;
            scheduleRestart();
        }
    }, 250);

    const shutdown = () => {
        quitting = true;
        if (restartTimer !== null) clearTimeout(restartTimer);
        clearInterval(poller);
        ctx.dispose();
        if (child !== null) child.kill();
        fs.rmSync(tmp, { force: true });
        // Best-effort cleanup of the app images we spawned. Images that are
        // still running (or just exiting) cannot be deleted on Windows — the
        // generation naming means that is harmless, just a leftover file.
        for (const exe of spawnedExes) {
            try {
                fs.rmSync(exe, { force: true, maxRetries: 3, retryDelay: 150 });
            } catch { /* best effort */ }
        }
        process.exit(0);
    };
    process.on("SIGINT", shutdown);
    process.on("SIGTERM", shutdown);
    // Keep the event loop alive for the watcher + child.
    setInterval(() => {}, 1 << 30);
}

function cmdDoctor() {
    const host = hostPathFor(pkgRoot);
    console.log(`nativets ${VERSION} (gpui-ts)`);
    console.log(`  node      ${process.version} (${process.platform}-${process.arch})`);
    console.log(`  host      ${host === null ? "MISSING for this platform" : host}`);
    if (host !== null) {
        console.log(`  host size ${(fs.statSync(host).size / 1048576).toFixed(1)} MB`);
    }
    const vendor = path.join(pkgRoot, "vendor");
    console.log(`  shipped   ${fs.existsSync(vendor) ? fs.readdirSync(vendor).join(", ") : "(none)"}`);
    // AOT backends: what did `nativets setup` (or the system) provide?
    const tc = toolchainSummary();
    console.log(`  home      ${tc.root}`);
    console.log("  backends  quickjs (default, zero toolchain)");
    console.log(`  backends  scriptc AOT — cargo:${tc.cargo} linker:${tc.linker}`);
    console.log(`  backends  perry     — cargo:${tc.cargo} perry:${tc.perry} linker:${tc.linker}`);
    if (tc.linker === "missing" || tc.cargo === "missing") {
        console.log(`  hint      run \`nativets setup\` to install the missing pieces (no PATH/registry changes)`);
    }
}

/**
 * `nativets setup` — make the AOT backends work with zero manual steps:
 * portable Rust + perry via npm + (when no Visual Studio exists) LLVM's
 * lld-link with a downloaded Microsoft CRT/SDK. Idempotent; each already-
 * present piece is skipped, so re-running after a failure resumes cleanly.
 */
async function cmdSetup(argv) {
    const acceptLicense = argv.includes("--accept-license");
    try {
        console.log(`nativets setup — toolchain root: ${nativetsRoot()}`);
        console.log("");
        await ensureRust();
        console.log("");
        ensurePerry();
        console.log("");
        await ensureXwinSdk({ acceptLicense });
        console.log("");
        console.log("nativets setup complete — AOT backends are ready:");
        console.log("  nativets build app.tsx --backend scriptc");
        console.log("  nativets build app.tsx --backend perry");
    } catch (e) {
        console.error(`nativets setup: ${e.message}`);
        process.exit(1);
    }
}

const argv = process.argv.slice(2);
const cmd = argv[0];
switch (cmd) {
    case "dev":
        await cmdDev(argv.slice(1));
        break;
    case "build":
        await cmdBuild(argv.slice(1));
        break;
    case "run":
        await cmdRun(argv.slice(1));
        break;
    case "doctor":
        cmdDoctor();
        break;
    case "setup":
        await cmdSetup(argv.slice(1));
        break;
    case "--version":
    case "-v":
        console.log(VERSION);
        break;
    case undefined:
    case "--help":
    case "-h":
        console.log(USAGE);
        break;
    default:
        fail(`unknown command: ${cmd}`);
}
