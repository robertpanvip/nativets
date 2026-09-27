// Build-time toolchain resolution: figure out WHERE the Rust/MSVC/SDK pieces
// live (setup-managed portable tree, system rustup, Visual Studio, or LLVM +
// xwin sysroot) and produce the env deltas that make `cargo build` work in a
// bare shell. `nativets build --backend scriptc|perry` applies these before
// spawning the repo-side CLI, so an end user never needs vcvars or rustup.
//
// Resolution order per piece (first hit wins):
//   cargo   NATIVETS_HOME\rust\cargo\bin   → PATH
//   perry   NATIVETS_HOME\tools\perry.cmd  → PATH (global `perry` also fine)
//   linker  vswhere MSVC link.exe          → nothing to inject (cc-rs finds it)
//           LLVM lld-link + xwin SDK       → RUSTFLAGS + PERRY_LLD_LINK + LIB
//           LLVM lld-link + MSVC SDK libs  → same, LIB from vswhere
import fs from "node:fs";
import path from "node:path";
import { spawnSync } from "node:child_process";
import { cargoBin, findMsvcLink, findLldLink, nativetsRoot, perryShim, sdkHome } from "./setup.mjs";

/** Locate cargo: setup-managed first, then PATH. Returns {path, via} or null. */
export function findCargo() {
    const managed = path.join(cargoBin(), "cargo.exe");
    if (fs.existsSync(managed)) return { path: managed, via: "portable" };
    const r = spawnSync("where", ["cargo"], { encoding: "utf8", shell: true });
    if (r.status === 0) {
        const first = (r.stdout ?? "").split(/\r?\n/).find((l) => l.trim());
        if (first && fs.existsSync(first.trim())) return { path: first.trim(), via: "system" };
    }
    return null;
}

/** Locate perry: setup-managed shim first, then PATH. */
export function findPerry() {
    if (fs.existsSync(perryShim())) return { path: perryShim(), via: "portable" };
    const r = spawnSync("where", ["perry"], { encoding: "utf8", shell: true });
    if (r.status === 0) {
        const first = (r.stdout ?? "").split(/\r?\n/).find((l) => l.trim());
        if (first && fs.existsSync(first.trim())) return { path: first.trim(), via: "system" };
    }
    return null;
}

/**
 * The MSVC import-library directories, from vswhere (VC tools lib +
 * Windows SDK um/ucrt). Null when no VS is installed.
 */
function msvcLibDirs() {
    const link = findMsvcLink();
    if (link === null) return null;
    // <install>\VC\Tools\MSVC\<ver>\bin\Hostx64\x64\link.exe
    const msvcVer = path.dirname(path.dirname(path.dirname(path.dirname(link))));
    const dirs = [path.join(msvcVer, "lib", "x64")];
    // Windows Kits: scan the standard root for the newest SDK version.
    const kits = path.join("C:", "Program Files (x86)", "Windows Kits", "10", "Lib");
    try {
        const versions = fs.readdirSync(kits).sort().reverse();
        for (const v of versions) {
            const um = path.join(kits, v, "um", "x64");
            const ucrt = path.join(kits, v, "ucrt", "x64");
            if (fs.existsSync(um)) {
                dirs.push(um);
                if (fs.existsSync(ucrt)) dirs.push(ucrt);
                break;
            }
        }
    } catch { /* no SDK */ }
    return dirs.filter((d) => fs.existsSync(d));
}

/**
 * The xwin sysroot import-library directories (perry's exact layout).
 * Null when the SDK isn't populated.
 */
function xwinLibDirs(sdk) {
    const dirs = [
        path.join(sdk, "crt", "lib", "x86_64"),
        path.join(sdk, "sdk", "lib", "um", "x86_64"),
        path.join(sdk, "sdk", "lib", "ucrt", "x86_64"),
    ];
    return dirs.every((d) => fs.existsSync(d)) ? dirs : null;
}

/** MSVC CRT/SDK headers for clang-cl (scriptc C runtime compilation). */
function xwinIncludeDirs(sdk) {
    const dirs = [
        path.join(sdk, "crt", "include"),
        path.join(sdk, "sdk", "include", "ucrt"),
        path.join(sdk, "sdk", "include", "um"),
        path.join(sdk, "sdk", "include", "shared"),
    ];
    return dirs.filter((d) => fs.existsSync(d));
}

/**
 * Everything the AOT build needs, as one snapshot.
 *
 * Returns null when a required piece is missing (caller explains). `env` is
 * the delta to merge over process.env when spawning cargo; it is EMPTY when
 * the system toolchain is complete (cc-rs + perry talk to MSVC by default).
 */
export function resolveToolchain() {
    const cargo = findCargo();
    if (cargo === null) return { ok: false, missing: ["rust (cargo)"] };
    const perry = findPerry();
    if (perry === null) return { ok: false, missing: ["perry (npm @perryts/perry)"] };

    const env = {};
    const extraPath = [];

    if (cargo.via === "portable") {
        // The setup-managed tree is a merged sysroot: <root>\rust\bin\cargo.exe
        // + <root>\rust\bin\rustc.exe + <root>\rust\lib\rustlib\... . Both
        // executables sit in the same dir, so one PATH entry suffices.
        extraPath.push(path.dirname(cargo.path));
    }
    if (perry.via === "portable") extraPath.push(path.dirname(perry.path));

    // Linker: MSVC → nothing needed. LLVM+SDK → wire it up by hand.
    const msvc = findMsvcLink();
    let linker = null;
    if (msvc !== null) {
        linker = { mode: "msvc", path: msvc };
    } else {
        const lld = findLldLink();
        if (lld === null) return { ok: false, missing: ["linker (MSVC or LLVM lld-link)"] };
        const sdk = sdkHome();
        const xwinDirs = xwinLibDirs(sdk);
        const msvcDirs = msvcLibDirs();
        const libDirs = xwinDirs ?? msvcDirs;
        if (libDirs === null || libDirs.length === 0) {
            return { ok: false, missing: ["Windows SDK import libraries (run `nativets setup`)"] };
        }
        // Make rustc use lld-link for every crate's link step.
        env.RUSTFLAGS = ((process.env.RUSTFLAGS ?? "") + " -Clinker-flavor=lld-link -Clinker=lld-link").trim();
        env.PERRY_LLD_LINK = lld.path.includes(path.sep) || lld.path.includes("/")
            ? lld.path
            : String.raw`C:\Program Files\LLVM\bin\lld-link.exe`;
        env.LIB = libDirs.join(";");
        // clang-cl (scriptc C runtime, via cc crate) needs the SDK headers too.
        if (xwinDirs !== null) {
            const inc = xwinIncludeDirs(sdk);
            if (inc.length > 0) env.INCLUDE = inc.join(";");
            const cl = path.join("C:", "Program Files", "LLVM", "bin", "clang-cl.exe");
            if (fs.existsSync(cl)) env.CC_x86_64_pc_windows_msvc = cl;
        }
        linker = { mode: "lld", path: env.PERRY_LLD_LINK, sdk: xwinDirs !== null ? sdk : "msvc-libs-only" };
    }

    if (extraPath.length > 0) {
        env.PATH = extraPath.join(path.delimiter) + path.delimiter + (process.env.PATH ?? "");
    }
    return { ok: true, cargo, perry, linker, env };
}

/** One-line status for `nativets doctor`. */
export function toolchainSummary() {
    const cargo = findCargo();
    const perry = findPerry();
    const msvc = findMsvcLink();
    const lld = msvc === null ? findLldLink() : null;
    const sdk = msvc === null && lld !== null
        ? (xwinLibDirs(sdkHome()) !== null ? "xwin sdk ok" : "sdk missing")
        : "";
    return {
        cargo: cargo === null ? "missing" : `ok (${cargo.via})`,
        perry: perry === null ? "missing" : `ok (${perry.via})`,
        linker: msvc !== null ? "ok (msvc)" : lld !== null ? `ok (lld-link, ${sdk})` : "missing",
        root: nativetsRoot(),
    };
}
