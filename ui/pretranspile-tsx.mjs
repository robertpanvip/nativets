/**
 * TSX → Perry-friendly TS (pre-transpile).
 *
 * perry 0.5.1520 has no JSX support at all — `perry check src/app.tsx` answers
 * "No TypeScript files found" ("Input TypeScript file" only accepts `.ts`), and
 * its parser has no JSX branch. So before sources reach `perry compile`, files
 * are prepared in two different ways:
 *
 *   - `.tsx` files go through esbuild: JSX → `h()` calls (classic transform,
 *     same factory the runtime exports); type annotations erased — JSX cannot
 *     carry them anyway.
 *   - `.ts` files are COPIED VERBATIM, types and all. Perry's own checker is
 *     a full TypeScript checker and compiles BETTER with annotations intact:
 *     under the type-erased pipeline every cross-module signal getter
 *     (`bars()`, `chartTone()`, …) resolved to `undefined` at runtime (the
 *     dynamic-dispatch path of 坑 #8) — the typed pipeline is the one that
 *     produced the last known-good full mount.
 *
 * ESM import structure is left untouched in both paths (`./runtime` stays
 * `./runtime`), so perry resolves the same graph it always did.
 *
 * Output lands in `ui/build/gen/*.ts` — that directory is what perry compiles.
 * The QuickJS/node backends don't use this: they bundle with esbuild directly
 * and never hand JSX to perry.
 */
import { copyFileSync, mkdirSync, readdirSync, rmSync, statSync } from "node:fs";
import path from "node:path";
import { build as esbuildBuild } from "esbuild";

/**
 * Recursively collect every `.ts`/`.tsx` source under `dir` (skipping `.d.ts`).
 * Subdirectories matter: app components live in `src/cards/*`, and with
 * `bundle:false` esbuild never walks imports — every module must be listed
 * explicitly or perry sees a dangling `./cards/header` import whose specifier
 * degrades to `true` at runtime (`h(true, null)` → `create op tag:true`,
 * a.k.a. the blank-window bug).
 *
 * @param {string} dir  absolute path to walk
 * @returns {string[]} absolute file paths, sorted for deterministic output
 */
function collectSources(dir) {
    const out = [];
    for (const name of readdirSync(dir).sort()) {
        const p = path.join(dir, name);
        if (statSync(p).isDirectory()) {
            out.push(...collectSources(p));
        } else if (/\.tsx?$/.test(name) && !name.endsWith(".d.ts")) {
            out.push(p);
        }
    }
    return out;
}

/**
 * @param {string} uiDir  absolute path to `ui/`
 * @param {string} entryName  basename of the app entry (e.g. `main.ts`)
 * @returns {Promise<{dir: string, entry: string, entryRel: string}>}
 */
export async function pretranspileTsx(uiDir, entryName = "main.ts") {
    const srcDir = path.join(uiDir, "src");
    const genDir = path.join(uiDir, "build", "gen");

    const all = collectSources(srcDir);
    // Only JSX-bearing files need lowering; `.ts` modules keep their types.
    const tsxPoints = all.filter((f) => f.endsWith(".tsx"));
    const tsCopies = all.filter((f) => f.endsWith(".ts"));

    rmSync(genDir, { recursive: true, force: true });
    mkdirSync(genDir, { recursive: true });

    // Verbatim copies preserve the typed pipeline for pure-TS modules.
    for (const src of tsCopies) {
        const rel = path.relative(srcDir, src);
        const dst = path.join(genDir, rel);
        mkdirSync(path.dirname(dst), { recursive: true });
        copyFileSync(src, dst);
    }

    if (tsxPoints.length > 0) {
        await esbuildBuild({
            entryPoints: tsxPoints,
            outdir: genDir,
            // Mirror the src/ subdirectory structure so relative imports
            // (`./cards/header` from app.tsx) resolve identically in gen/.
            // `root` is implicit: outdir paths follow each entry's location
            // relative to the common ancestor (srcDir), which is what we want.
            bundle: false,
            format: "esm",
            target: "es2020",
            jsx: "transform",
            jsxFactory: "h",
            jsxFragment: "Fragment",
            charset: "ascii",
            outExtension: { ".js": ".ts" },
            logLevel: "warning",
        });
    }

    const entryFile = entryName.replace(/\.tsx$/, ".ts");
    return {
        dir: genDir,
        entry: path.join(genDir, entryFile),
        entryRel: path.join("build", "gen", entryFile).replace(/\\/g, "/"),
    };
}
