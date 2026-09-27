// Pack a TSX app + the prebuilt host into ONE self-contained exe — no Rust
// toolchain, no MSVC, no codegen on the user's machine.
//
//   node scripts/pack.mjs <entry.tsx> [-o out.exe] [--host host.exe]
//
// How it works: esbuild bundles the app into a single IIFE, that bundle is
// appended to a *copy* of the host executable behind a marker, and the host
// reads it back out of its own file at startup (see
// `host/src/quickjs.rs::bundle_from_self`). Windows' PE loader ignores trailing
// bytes, so the result is a normal exe that happens to carry a script.
//
// This is the mechanism the published npm package uses; the repo version just
// defaults `--host` to the freshly built binary.
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");

/**
 * esbuild sits where it was installed. In a published package it is a normal
 * dependency and resolves directly; inside this monorepo it lives in `ui/`.
 */
async function loadEsbuild() {
    try {
        return await import("esbuild");
    } catch {
        const local = path.join(root, "ui", "node_modules", "esbuild", "lib", "main.js");
        if (!fs.existsSync(local)) {
            console.error("[pack] esbuild not found — run `npm install` in ui/ (or install esbuild)");
            process.exit(2);
        }
        return await import(pathToFileURL(local).href);
    }
}

const { build } = await loadEsbuild();

/** Must match `TAIL_MARKER` in host/src/quickjs.rs. */
const TAIL_MARKER = "\n<<<GPUI_TS_BUNDLE_v1>>>\n";
const HOST_DEFAULT = path.join(root, "host", "target", "release", "nativets-host.exe");

let entry = null;
let out = null;
let host = HOST_DEFAULT;

const argv = process.argv.slice(2);
for (let i = 0; i < argv.length; i++) {
    const a = argv[i];
    if (a === "-o" || a === "--out") out = path.resolve(argv[++i]);
    else if (a === "--host") host = path.resolve(argv[++i]);
    else if (!a.startsWith("-")) entry = path.resolve(a);
}

if (entry === null) {
    console.error("usage: node scripts/pack.mjs <entry.tsx> [-o out.exe] [--host host.exe]");
    process.exit(2);
}
if (out === null) {
    const base = path.basename(entry).replace(/\.[jt]sx?$/, "");
    out = path.join(process.cwd(), base + ".exe");
}
if (!fs.existsSync(host)) {
    console.error(`[pack] host not found: ${host}`);
    console.error("       build it first (node scripts/gpui-ts.mjs) or pass --host <path>");
    process.exit(2);
}
if (path.resolve(out) === path.resolve(host)) {
    console.error("[pack] refusing to overwrite the host itself — pick another -o path");
    process.exit(2);
}

const tmpJs = path.join(os.tmpdir(), `gpui-ts-pack-${process.pid}.js`);

await build({
    entryPoints: [entry],
    bundle: true,
    format: "iife",
    target: "es2020",
    // Same JSX contract as the framework's own build: classic transform onto the
    // runtime's `h` factory, so `<div/>` needs `h` in scope (imported from the
    // package). See ui/build-qjs.mjs for why charset must be "ascii".
    jsx: "transform",
    jsxFactory: "h",
    jsxFragment: "Fragment",
    charset: "ascii",
    outfile: tmpJs,
    logLevel: "warning",
});

const js = fs.readFileSync(tmpJs, "utf8");
fs.rmSync(tmpJs, { force: true });

if (js.includes(TAIL_MARKER)) {
    console.error("[pack] bundle contains the tail marker; the host would mis-split it");
    process.exit(1);
}

fs.mkdirSync(path.dirname(out), { recursive: true });
fs.copyFileSync(host, out);
fs.appendFileSync(out, Buffer.from(TAIL_MARKER + js, "utf8"));

const hostMb = (fs.statSync(host).size / 1048576).toFixed(1);
const jsKb = (js.length / 1024).toFixed(0);
console.log(`[pack] ${path.relative(process.cwd(), entry)}  →  ${out}`);
console.log(`[pack] host ${hostMb} MB + bundle ${jsKb} KB · 单文件 · 零工具链依赖`);
