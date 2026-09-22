// Bundle the TSX frontend for node-dev mode (`node dist/main.js`).
// The QuickJS path reuses the same settings in build-qjs.mjs; the Perry path
// pre-transpiles with the same JSX options in build-perry.mjs.
//
// JSX → `h()` (classic transform), so any `.tsx` file must import `h`
// (and `Fragment` when it uses `<></>`).
import * as esbuild from "esbuild";

await esbuild.build({
    entryPoints: ["src/main.ts"],
    bundle: true,
    platform: "node",
    format: "cjs",
    target: "es2020",
    jsx: "transform",
    jsxFactory: "h",
    jsxFragment: "Fragment",
    outfile: "dist/main.js",
    logLevel: "info",
});
