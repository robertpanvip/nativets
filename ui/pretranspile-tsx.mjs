/**
 * TSX → Perry-friendly TS (pre-transpile).
 *
 * perry 0.5.1520 has no JSX support at all — `perry check src/app.tsx` answers
 * "No TypeScript files found" ("Input TypeScript file" only accepts `.ts`), and
 * its parser has no JSX branch. So before sources reach `perry compile`, every
 * `.ts`/`.tsx` file is lowered by esbuild:
 *
 *   - JSX → `h()` calls (classic transform, same factory the runtime exports)
 *   - type annotations erased
 *   - ESM import structure and specifiers left untouched (`./runtime` stays
 *     `./runtime`), so perry resolves the same graph it always did
 *
 * Output lands in `ui/build/gen/*.ts` — that directory is what perry compiles.
 * The QuickJS/node backends don't use this: they bundle with esbuild directly
 * and never hand JSX to perry.
 */
import { readdirSync, rmSync } from "node:fs";
import path from "node:path";
import { build as esbuildBuild } from "esbuild";

/**
 * @param {string} uiDir  absolute path to `ui/`
 * @param {string} entryName  basename of the app entry (e.g. `main.ts`)
 * @returns {Promise<{dir: string, entry: string, entryRel: string}>}
 */
export async function pretranspileTsx(uiDir, entryName = "main.ts") {
    const srcDir = path.join(uiDir, "src");
    const genDir = path.join(uiDir, "build", "gen");

    // Every module, not just the entry: esbuild does not walk imports when
    // bundling is off, so the file list has to be explicit. `.d.ts` files are
    // declaration-only and must not be emitted.
    const entryPoints = readdirSync(srcDir)
        .filter((f) => /\.tsx?$/.test(f) && !f.endsWith(".d.ts"))
        .sort()
        .map((f) => path.join(srcDir, f));

    rmSync(genDir, { recursive: true, force: true });
    await esbuildBuild({
        entryPoints,
        outdir: genDir,
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

    const entryFile = entryName.replace(/\.tsx$/, ".ts");
    return {
        dir: genDir,
        entry: path.join(genDir, entryFile),
        entryRel: path.join("build", "gen", entryFile).replace(/\\/g, "/"),
    };
}
