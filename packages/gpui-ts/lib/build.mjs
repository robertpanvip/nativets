// The packaging core: an app's TSX → ONE self-contained exe, with no Rust
// toolchain on the user's machine.
//
//   1. esbuild bundles the app into a single IIFE
//   2. the bundle is appended to a *copy* of the prebuilt host, behind a marker
//   3. the host reads it back out of its own file at startup
//      (host/src/quickjs.rs::bundle_from_self)
//
// Windows' PE loader ignores trailing bytes, so the result is an ordinary exe
// that happens to carry a script.
import fs from "node:fs";
import path from "node:path";

/**
 * Separator between the host binary and the appended bundle.
 *
 * **Must match `TAIL_MARKER` in `host/src/quickjs.rs`** — the two halves are
 * shipped independently (binary vs. JS), so this is a cross-artifact contract.
 */
export const TAIL_MARKER = "\n<<<GPUI_TS_BUNDLE_v1>>>\n";

export const HOST_BASENAME = "gpui-ts-host.exe";

/** esbuild settings shared by `build` and `run`. */
export const ESBUILD_OPTS = {
    bundle: true,
    format: "iife",
    target: "es2020",
    // JSX is sugar for the runtime's `h()` factory (classic transform), so an
    // entry file must import `h` (and `Fragment` for `<></>`).
    jsx: "transform",
    jsxFactory: "h",
    jsxFragment: "Fragment",
    // Escape every non-ASCII code point: the embedded QuickJS parser is strict
    // about UTF-8, and esbuild would otherwise emit e.g. U+00D7 (×) as a lone
    // 0xD7 byte, which is not valid UTF-8. (See ui/build-qjs.mjs.)
    charset: "ascii",
    logLevel: "warning",
};

/** Path of the prebuilt host for a platform, or `null` when we ship none. */
export function hostPathFor(packageRoot, platform = process.platform, arch = process.arch) {
    const p = path.join(packageRoot, "vendor", `${platform}-${arch}`, HOST_BASENAME);
    return fs.existsSync(p) ? p : null;
}

/** Bundle `entry` and return the JS source. */
export async function bundleApp(esbuild, entry, outfile) {
    await esbuild.build({ ...ESBUILD_OPTS, entryPoints: [entry], outfile });
    return fs.readFileSync(outfile, "utf8");
}

/**
 * Copy `host` to `out` and append the bundle.
 *
 * Refuses a bundle that already contains the marker: the host reads from the
 * **last** marker, so a stray one inside the JS would make it split the file at
 * the wrong place and then fail with a confusing parse error.
 */
export function packExe(host, js, out) {
    if (js.includes(TAIL_MARKER)) {
        throw new Error(
            "the bundle contains the tail marker; the host would split the file at the wrong offset",
        );
    }
    fs.mkdirSync(path.dirname(out), { recursive: true });
    fs.copyFileSync(host, out);
    fs.appendFileSync(out, Buffer.from(TAIL_MARKER + js, "utf8"));
}
