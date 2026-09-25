#!/usr/bin/env node
// gpui-ts CLI — turn a TypeScript app into a native desktop window.
//
//   gpui-ts build src/app.tsx -o myapp.exe   # produce one self-contained exe
//   gpui-ts run   src/app.tsx                # build to a temp file and run it
//   gpui-ts doctor                           # is this install usable?
//
// No Rust, no MSVC, no codegen: the host binary ships prebuilt in `vendor/` and
// the app is appended to it as script data (see lib/build.mjs).
import { spawnSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { bundleApp, hostPathFor, packExe } from "../lib/build.mjs";

const pkgRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");

const VERSION = JSON.parse(
    fs.readFileSync(path.join(pkgRoot, "package.json"), "utf8"),
).version;

const USAGE = `gpui-ts ${VERSION} — TypeScript → native desktop app

Usage:
  gpui-ts build <entry.tsx> [-o <out.exe>]   Bundle and pack into one exe
  gpui-ts run   <entry.tsx>                  Pack to a temp file and run it
  gpui-ts doctor                             Report what this install can do
  gpui-ts --help | --version

Your entry file is an ordinary TSX module. It must bring in the JSX factory:

  import { createRoot, h, appendChild } from "gpui-ts";
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
        console.error("gpui-ts: esbuild is missing — reinstall the package.");
        process.exit(2);
    }
}

function parseFlags(argv) {
    const out = { entry: null, out: null };
    for (let i = 0; i < argv.length; i++) {
        const a = argv[i];
        if (a === "-o" || a === "--out") {
            out.out = argv[++i];
            if (out.out === undefined) fail(`missing path after ${a}`);
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
    console.error(`gpui-ts: ${msg}`);
    console.error("run `gpui-ts --help` for usage");
    process.exit(2);
}

function requireHost() {
    const host = hostPathFor(pkgRoot);
    if (host === null) {
        console.error(
            `gpui-ts: no prebuilt host for ${process.platform}-${process.arch}.`,
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

async function cmdBuild(argv) {
    const { entry, out } = parseFlags(argv);
    if (entry === null) fail("build needs an entry file");
    const entryPath = resolveEntry(entry);
    const outPath = path.resolve(out === null ? defaultOut(entry) : out);
    const host = requireHost();

    const esbuild = await loadEsbuild();
    const tmp = path.join(os.tmpdir(), `gpui-ts-${process.pid}.js`);
    let js;
    try {
        js = await bundleApp(esbuild, entryPath, tmp);
    } finally {
        fs.rmSync(tmp, { force: true });
    }
    packExe(host, js, outPath);

    const mb = (fs.statSync(outPath).size / 1048576).toFixed(1);
    console.log(`gpui-ts: ${path.relative(process.cwd(), outPath)}  (${mb} MB, self-contained)`);
    return outPath;
}

async function cmdRun(argv) {
    const { entry } = parseFlags(argv);
    if (entry === null) fail("run needs an entry file");
    const tmpExe = path.join(
        os.tmpdir(),
        `gpui-ts-${path.basename(entry).replace(/\.[jt]sx?$/, "")}-${process.pid}.exe`,
    );
    await cmdBuild([entry, "-o", tmpExe]);
    try {
        const r = spawnSync(tmpExe, [], { stdio: "inherit" });
        process.exit(r.status === null ? 1 : r.status);
    } finally {
        fs.rmSync(tmpExe, { force: true });
    }
}

function cmdDoctor() {
    const host = hostPathFor(pkgRoot);
    console.log(`gpui-ts ${VERSION}`);
    console.log(`  node      ${process.version} (${process.platform}-${process.arch})`);
    console.log(`  host      ${host === null ? "MISSING for this platform" : host}`);
    if (host !== null) {
        console.log(`  host size ${(fs.statSync(host).size / 1048576).toFixed(1)} MB`);
    }
    const vendor = path.join(pkgRoot, "vendor");
    console.log(`  shipped   ${fs.existsSync(vendor) ? fs.readdirSync(vendor).join(", ") : "(none)"}`);
}

const argv = process.argv.slice(2);
const cmd = argv[0];
switch (cmd) {
    case "build":
        await cmdBuild(argv.slice(1));
        break;
    case "run":
        await cmdRun(argv.slice(1));
        break;
    case "doctor":
        cmdDoctor();
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
