// `nativets setup` — one command that makes the AOT backends work with no
// Visual Studio and no rustup: a portable Rust toolchain, the perry compiler
// via npm, and (when no MSVC is installed) a lightweight Microsoft CRT/SDK
// sysroot fetched with cargo-xwin. Nothing touches PATH or the registry;
// everything lives under one root (default %USERPROFILE%\.nativets) and is
// removed by deleting that directory.
//
// Layout under the root:
//   rust\          portable rustc/cargo (rust standalone tar.gz, extracted)
//   tools\         npm prefix for @perryts/perry (perry.cmd shim)
//   cargo-xwin.exe prebuilt xwin driver (only when no MSVC is found)
//   windows-sdk\   MS CRT + Windows SDK import libs (~1.1 GB on disk)
//
// The MS components are redistributed under the Microsoft Software License
// Terms (https://go.microsoft.com/fwlink/?LinkId=2086102); they are DOWNLOADED
// at setup time, never shipped inside the npm package.
import { spawnSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";

/** Root of the portable toolchain tree. */
export function nativetsRoot() {
    return process.env.NATIVETS_HOME ?? path.join(os.homedir(), ".nativets");
}

export function rustHome() {
    return path.join(nativetsRoot(), "rust");
}

export function cargoBin() {
    // Merged sysroot layout: everything in <root>\rust\bin.
    return path.join(rustHome(), "bin");
}

export function toolsHome() {
    return path.join(nativetsRoot(), "tools");
}

export function sdkHome() {
    // Same env override perry honours, so one variable steers both.
    return process.env.PERRY_WINDOWS_SYSROOT ?? path.join(nativetsRoot(), "windows-sdk");
}

/** perry.cmd / perry shim inside the setup-managed npm prefix. */
export function perryShim() {
    return path.join(toolsHome(), "perry.cmd");
}

function log(msg) {
    console.log(`nativets setup: ${msg}`);
}

function run(cmd, args, opts = {}) {
    const r = spawnSync(cmd, args, { stdio: "inherit", shell: process.platform === "win32", ...opts });
    return r.status === 0;
}

/** Is `cmd` invocable? (PATH probe; also accepts an absolute path) */
export function haveTool(cmd) {
    if (cmd.includes(path.sep) || cmd.includes("/")) return fs.existsSync(cmd);
    const probe = process.platform === "win32" ? "where" : "command";
    const args = process.platform === "win32" ? [cmd] : ["-v", cmd];
    const r = spawnSync(probe, args, { stdio: "ignore", shell: process.platform === "win32" });
    return r.status === 0;
}

// ---------------------------------------------------------------------------
// MSVC detection — if a real Visual Studio is present we skip the whole SDK
// download; cc-rs and perry already know how to talk to it.
// ---------------------------------------------------------------------------

const VSWHERE = path.join(
    "C:", "Program Files (x86)", "Microsoft Visual Studio", "Installer", "vswhere.exe",
);

/** Path of MSVC link.exe according to vswhere, or null. */
export function findMsvcLink() {
    if (!fs.existsSync(VSWHERE)) return null;
    const r = spawnSync(VSWHERE, [
        "-latest", "-products", "*", "-requires", "Microsoft.VisualStudio.Component.VC.Tools.x86.x64",
        "-property", "installationPath",
    ], { encoding: "utf8" });
    if (r.status !== 0) return null;
    const install = (r.stdout ?? "").trim();
    if (!install) return null;
    // Newest VC tools version wins.
    const msvcRoot = path.join(install, "VC", "Tools", "MSVC");
    let versions = [];
    try {
        versions = fs.readdirSync(msvcRoot).filter((v) => fs.existsSync(path.join(msvcRoot, v, "bin", "Hostx64", "x64", "link.exe")));
    } catch { return null; }
    versions.sort().reverse();
    return versions.length > 0 ? path.join(msvcRoot, versions[0], "bin", "Hostx64", "x64", "link.exe") : null;
}

/** Standard LLVM install (provides lld-link + clang-cl). */
export function findLldLink() {
    const standalone = path.join("C:", "Program Files", "LLVM", "bin", "lld-link.exe");
    if (fs.existsSync(standalone)) return standalone;
    if (haveTool("lld-link")) return "lld-link";
    return null;
}

// ---------------------------------------------------------------------------
// Step 1 — portable Rust
// ---------------------------------------------------------------------------

async function currentStableVersion() {
    const r = await fetch("https://static.rust-lang.org/dist/channel-rust-stable.toml");
    if (!r.ok) throw new Error(`cannot read the Rust stable channel (HTTP ${r.status})`);
    const text = await r.text();
    const m = text.match(/\[pkg\.rust\][\s\S]*?version\s*=\s*"(\d+\.\d+\.\d+)/);
    if (!m) throw new Error("cannot parse the Rust stable channel manifest");
    return m[1];
}

/**
 * Extract a .tar.gz on stock Windows: bsdtar ships with Windows 10 1803+ and
 * understands both the gzip and the long-path (//?/) entries in the Rust
 * archive. No npm tar dependency needed.
 */
function extractTarGz(archive, destDir) {
    fs.mkdirSync(destDir, { recursive: true });
    const r = spawnSync("bsdtar", ["-xf", archive, "-C", destDir, "--strip-components=1"], {
        stdio: ["ignore", "pipe", "inherit"],
        shell: process.platform === "win32",
    });
    if (r.status !== 0) throw new Error("bsdtar extraction failed (is this Windows 10 1803+?)");
}

/**
 * Merge the standalone distribution's per-component directories (cargo\,
 * rustc\, rust-std-<triple>\ …) into ONE sysroot tree. The archive ships the
 * rust installer layout; without this merge, cargo-on-PATH finds rustc but
 * rustc cannot find its own std crates (E0463) — the components only form a
 * working sysroot once copied under a common prefix. `install.sh` does this
 * on POSIX; here it is robocopy merges (fast, no shims, no bash needed).
 */
function mergeRustComponents(home) {
    const comps = fs.readdirSync(home, { withFileTypes: true })
        .filter((d) => d.isDirectory() && (
            d.name === "cargo" ||
            d.name === "rustc" ||
            d.name.startsWith("rust-std-") ||
            d.name === "clippy-preview" ||
            d.name === "rustfmt-preview"
        ))
        .map((d) => d.name);
    for (const comp of comps) {
        copyInto(path.join(home, comp), home);
    }
    // Post-merge the canonical layout is <home>\bin\cargo.exe + <home>\lib\rustlib.
    if (!fs.existsSync(path.join(home, "bin", "cargo.exe"))) {
        throw new Error("Rust component merge did not produce <home>\\bin\\cargo.exe");
    }
    // Drop the now-redundant component directories (they double the size).
    for (const comp of comps) {
        try { fs.rmSync(path.join(home, comp), { recursive: true, force: true }); } catch { /* best effort */ }
    }
}

function copyInto(srcDir, destDir) {
    fs.mkdirSync(destDir, { recursive: true });
    // Robocopy's exit codes: 0-7 = success variants, >=8 = failure.
    const r = spawnSync("robocopy", [srcDir, destDir, "/E", "/NFL", "/NDL", "/NJH", "/NJS", "/NP"], {
        stdio: "ignore",
        shell: process.platform === "win32",
    });
    if (r.status === null || r.status >= 8) {
        throw new Error(`robocopy ${srcDir} → ${destDir} failed (code ${r.status})`);
    }
}

export async function ensureRust() {
    const bin = cargoBin();
    if (fs.existsSync(path.join(bin, "cargo.exe"))) {
        log(`✔ portable Rust already present (${bin})`);
        return;
    }
    const version = await currentStableVersion();
    const triple = `${process.arch === "x64" ? "x86_64" : "aarch64"}-pc-windows-msvc`;
    const url = `https://static.rust-lang.org/dist/rust-${version}-${triple}.tar.gz`;
    log(`downloading portable Rust ${version} (${triple})…`);
    log("  this is a ~250 MB download, one time only");
    const res = await fetch(url);
    if (!res.ok) throw new Error(`Rust download failed (HTTP ${res.status}): ${url}`);
    const root = nativetsRoot();
    fs.mkdirSync(root, { recursive: true });
    const archive = path.join(root, "rust.tar.gz");
    fs.writeFileSync(archive, Buffer.from(await res.arrayBuffer()));
    log("extracting…");
    try {
        extractTarGz(archive, rustHome());
        log("merging components into a single sysroot…");
        mergeRustComponents(rustHome());
    } finally {
        try { fs.rmSync(archive, { force: true }); } catch { /* leftover ok */ }
    }
    if (!fs.existsSync(path.join(bin, "cargo.exe"))) {
        throw new Error(`extraction did not produce ${bin}\\cargo.exe`);
    }
    log(`✔ portable Rust ${version} ready (${bin})`);
}

// ---------------------------------------------------------------------------
// Step 2 — perry compiler via npm (platform package comes in as optional dep)
// ---------------------------------------------------------------------------

export function ensurePerry() {
    if (fs.existsSync(perryShim())) {
        log(`✔ perry already present (${perryShim()})`);
        return;
    }
    if (haveTool("perry")) {
        log("✔ perry already on PATH (global install) — skipping");
        return;
    }
    log("installing the perry compiler via npm (~330 MB with the runtime libs)…");
    fs.mkdirSync(toolsHome(), { recursive: true });
    if (!run("npm", ["install", "--prefix", toolsHome(), "@perryts/perry"])) {
        throw new Error("npm install @perryts/perry failed");
    }
    if (!fs.existsSync(perryShim())) {
        throw new Error(`npm did not produce ${perryShim()}`);
    }
    log(`✔ perry ready (${perryShim()})`);
}

// ---------------------------------------------------------------------------
// Step 3 — linker strategy: MSVC if present, else LLVM lld-link + xwin SDK
// ---------------------------------------------------------------------------

/**
 * Linker strategy — MSVC when a Visual Studio is installed, otherwise LLVM's
 * lld-link plus an xwin-fetched Microsoft CRT/SDK sysroot (the same layout
 * perry's PERRY_WINDOWS_SYSROOT probing expects: crt/lib/<arch>,
 * sdk/lib/um/<arch>, sdk/lib/ucrt/<arch>). Downloads cargo-xwin + the SDK on
 * first use; every step is idempotent and safe to re-run.
 */
export async function ensureXwinSdk({ acceptLicense }) {
    const msvc = findMsvcLink();
    if (msvc !== null) {
        log(`✔ MSVC found (${msvc}) — no SDK download needed`);
        return { mode: "msvc" };
    }
    const lld = findLldLink();
    if (lld === null) {
        throw new Error(
            "no MSVC and no LLVM found. Install LLVM, then re-run setup:\n" +
            "  winget install LLVM.LLVM",
        );
    }
    const sdk = sdkHome();
    const xwinExe = path.join(nativetsRoot(), "cargo-xwin.exe");
    if (!fs.existsSync(path.join(sdk, "crt", "lib", "x86_64"))) {
        if (!acceptLicense) {
            console.error(
                "\nThe Microsoft CRT + Windows SDK libraries are needed for linking" +
                " (no Visual Studio on this machine). They are redistributed under the" +
                " Microsoft Software License Terms: https://go.microsoft.com/fwlink/?LinkId=2086102",
            );
            throw new Error(
                "license not accepted — re-run with `nativets setup --accept-license`" +
                " (or install Visual Studio Build Tools instead)",
            );
        }
        if (!fs.existsSync(xwinExe)) {
            log("downloading cargo-xwin (the xwin driver, ~3 MB)…");
            const arch = process.arch === "x64" ? "x64" : "x86";
            const ver = XWIN_VERSION;
            const url =
                `https://github.com/rust-cross/cargo-xwin/releases/download/v${ver}/` +
                `cargo-xwin-v${ver}.windows-${arch}.zip`;
            const res = await fetch(url);
            if (!res.ok) throw new Error(`cargo-xwin download failed (HTTP ${res.status})`);
            const root = nativetsRoot();
            fs.mkdirSync(root, { recursive: true });
            const zip = path.join(root, "cargo-xwin.zip");
            fs.writeFileSync(zip, Buffer.from(await res.arrayBuffer()));
            // Windows 10 1803+ ships bsdtar, which also handles .zip.
            const r = spawnSync("bsdtar", ["-xf", zip, "-C", root, "-x"], {
                stdio: ["ignore", "pipe", "inherit"], shell: process.platform === "win32",
            });
            fs.rmSync(zip, { force: true });
            if (r.status !== 0) throw new Error("extracting cargo-xwin failed");
        }
        if (!fs.existsSync(xwinExe)) throw new Error(`extracting cargo-xwin did not produce ${xwinExe}`);

        log("downloading the Microsoft CRT + Windows SDK (~1.1 GB on disk, one time)…");
        if (!run(xwinExe, ["cache", "xwin"], {
            env: {
                ...process.env,
                XWIN_CACHE_DIR: sdk,
                XWIN_ARCH: "x86_64",
                // Accepted programmatically — the caller already gated on the
                // --accept-license flag, which points at MS's license URL.
                XWIN_ACCEPT_LICENSE: "1",
            },
        })) {
            throw new Error(`SDK download failed — the partial tree at ${sdk} is safe to re-run over`);
        }
    } else {
        log(`✔ Windows SDK already present (${sdk})`);
    }
    log("✔ lightweight Windows toolchain ready (lld-link + xwin SDK)");
    return { mode: "lld", lld, sdk };
}

/** cargo-xwin release to fetch the xwin driver from. */
export const XWIN_VERSION = "0.23.1";
