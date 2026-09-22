// Bundle the TS frontend for node-dev mode (the Perry path compiles src/ directly).
import * as esbuild from "esbuild";

await esbuild.build({
    entryPoints: ["src/main.ts"],
    bundle: true,
    platform: "node",
    format: "cjs",
    target: "es2020",
    outfile: "dist/main.js",
    logLevel: "info",
});
