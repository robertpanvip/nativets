// Unit test: pruneXwinSdk keeps exactly the dirs toolchain.mjs consumes
// (xwinLibDirs + xwinIncludeDirs) and removes bookkeeping ballast.
// Run: node test/setup-prune.mjs
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { pruneXwinSdk } from "../lib/setup.mjs";

let failures = 0;
function check(cond, msg) {
    if (cond) {
        console.log(`  ok  ${msg}`);
    } else {
        console.error(`FAIL  ${msg}`);
        failures++;
    }
}

// --- build a fake xwin sysroot -------------------------------------------
const sdk = fs.mkdtempSync(path.join(os.tmpdir(), "xwin-prune-"));
const mk = (rel, file = null) => {
    const d = path.join(sdk, rel);
    fs.mkdirSync(d, { recursive: true });
    if (file !== null) fs.writeFileSync(path.join(d, file), "x");
    return d;
};

// kept trees (what toolchain.mjs reads)
mk("crt/lib/x86_64", "libcmt.lib");
mk("crt/include", "corecrt.h");
mk("sdk/lib/um/x86_64", "kernel32.lib");
mk("sdk/lib/ucrt/x86_64", "ucrt.lib");
mk("sdk/include/ucrt", "stdio.h");
mk("sdk/include/um", "windows.h");
mk("sdk/include/shared", "windef.h");

// removable ballast
mk("sdk/lib/um/aarch64", "kernel32.lib");       // other arch
mk("crt/lib/x86_64/.progress", "marker");        // xwin bookkeeping dir
mk("crt/src", "uuid.c");                          // CRT sources (unused)
mk("sdk/inc", "manifest.txt");                    // variant manifest

// bookkeeping files inside kept trees
fs.writeFileSync(path.join(sdk, "crt", "xwin.mutex"), "");
fs.writeFileSync(path.join(sdk, "sdk", "um.progress"), "");

// --- run ------------------------------------------------------------------
const removed = pruneXwinSdk(sdk);
console.log("removed:", removed.join(", ") || "(nothing)");

// --- assertions ------------------------------------------------------------
const exists = (rel) => fs.existsSync(path.join(sdk, rel));
check(exists("crt/lib/x86_64/libcmt.lib"), "kept crt/lib/x86_64 (link libs)");
check(exists("crt/include/corecrt.h"), "kept crt/include (headers)");
check(exists("sdk/lib/um/x86_64/kernel32.lib"), "kept sdk/lib/um/x86_64");
check(exists("sdk/lib/ucrt/x86_64/ucrt.lib"), "kept sdk/lib/ucrt/x86_64");
check(exists("sdk/include/ucrt/stdio.h"), "kept sdk/include/ucrt");
check(exists("sdk/include/um/windows.h"), "kept sdk/include/um");
check(exists("sdk/include/shared/windef.h"), "kept sdk/include/shared");
check(!exists("sdk/lib/um/aarch64"), "removed other-arch libs");
check(!exists("crt/lib/x86_64/.progress"), "removed .progress dir inside kept tree");
check(!exists("crt/src"), "removed crt sources");
check(!exists("sdk/inc"), "removed variant manifests");
check(!fs.existsSync(path.join(sdk, "crt", "xwin.mutex")), "removed .mutex file in kept tree");
check(!fs.existsSync(path.join(sdk, "sdk", "um.progress")), "removed .progress file in kept tree");
check(removed.includes("aarch64") || removed.length >= 3, "reported removals");

// idempotent second run
const removed2 = pruneXwinSdk(sdk);
check(removed2.length === 0, "idempotent — second run removes nothing");

// still-kept tree still complete after second run
check(exists("sdk/lib/um/x86_64/kernel32.lib"), "kept libs survive second run");

fs.rmSync(sdk, { recursive: true, force: true });
if (failures > 0) {
    console.error(`\n${failures} FAILURE(S)`);
    process.exit(1);
}
console.log("\nall prune tests passed");
