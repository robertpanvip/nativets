// gpui-ts — TypeScript → 单文件原生 EXE 编译器 CLI
//
//   node scripts/gpui-ts.mjs [entry.ts] [-o out.exe] [--backend quickjs|perry]
//
// 两个后端，协议与前端源码完全一致（JSONL mutation 协议），只是运行时层不同：
//
//   ── quickjs（默认）──────────────────────────────────────────────────────
//   1. esbuild 把 TS 打包成单个 IIFE JS（ui/dist/main.js，charset=ascii）
//   2. host 内嵌 QuickJS 引擎（rquickjs），bundle 用 include_str! 编进 exe，
//      10ms 自驱事件循环（promise jobs + timers + stdin 派发）
//   产物 ~11MB，零 Node、零子进程、零 sidecar 文件。
//
//   ── perry（历史后端，--backend perry）────────────────────────────────────
//   1. perry compile <entry> --output-type staticlib
//      （PERRY_WORKSPACE_ROOT auto-optimize：按 imports 裁剪 stdlib，体积 -66%）
//   2. host build.rs 链接 app staticlib + perry runtime/stdlib + gpui
//   3. perry_poll + js_stdlib_process_pending 驱动
//   产物 ~16MB；有 500ms I/O 量子、STDLIB_PUMP_FN 间接注册链、
//   /FORCE:MULTIPLE 符号分家等已知问题（见 docs/gpui-ts-plan.md）。
//
// 产物运行：直接双击 / 命令行执行即可，GPUI 窗口即 TS 应用 UI。
import { spawnSync } from "node:child_process";
import { existsSync, mkdirSync, copyFileSync, statSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const args = process.argv.slice(2);

let entry = "ui/src/main.ts";
let out = null;
let backend = process.env.GPUI_TS_BACKEND ?? "quickjs";
for (let i = 0; i < args.length; i++) {
    if (args[i] === "-o") {
        out = args[++i];
    } else if (args[i] === "--backend") {
        backend = args[++i];
    } else if (args[i] === "-h" || args[i] === "--help") {
        console.log("用法：node scripts/gpui-ts.mjs [entry.ts] [-o out.exe] [--backend quickjs|perry]");
        process.exit(0);
    } else {
        entry = args[i];
    }
}
if (backend !== "quickjs" && backend !== "perry") {
    console.error(`[gpui-ts] 未知后端: ${backend}（可选 quickjs | perry）`);
    process.exit(1);
}

const entryPath = path.resolve(root, entry);
if (!existsSync(entryPath)) {
    console.error(`[gpui-ts] 入口不存在: ${entryPath}`);
    process.exit(1);
}
if (!out) {
    out = path.join(
        root,
        "build",
        "dist",
        path.basename(entry).replace(/\.tsx?$/, "") + ".exe"
    );
}
const buildDir = path.join(root, "build");
const uiDir = path.join(root, "ui");
const HOST_EXE = path.join(root, "host", "target", "release", "gpui-perryts-host.exe");

/** 路径转成 node_modules/.bin 下可直接 spawn 的形式 */
function run(cmd, cmdArgs, opts = {}) {
    const r = spawnSync(cmd, cmdArgs, {
        stdio: "inherit",
        shell: process.platform === "win32",
        ...opts,
    });
    if (r.status !== 0) process.exit(r.status ?? 1);
    return r;
}

function emit() {
    mkdirSync(path.dirname(out), { recursive: true });
    copyFileSync(HOST_EXE, out);
    const mb = (statSync(out).size / 1024 / 1024).toFixed(1);
    console.log(`[gpui-ts] ✔ ${out}  ${mb} MB · 单文件 · 零子进程 · 无 Node/V8 依赖`);
    console.log(`[gpui-ts]   运行：直接执行即可；GPUI 窗口即 TS 应用 UI`);
}

// ===========================================================================
// backend: quickjs — esbuild bundle → 内嵌引擎编译 → 单 exe
// ===========================================================================
if (backend === "quickjs") {
    if (!existsSync(path.join(uiDir, "node_modules", "esbuild"))) {
        console.error("[gpui-ts] 未找到 esbuild，请先在 ui/ 下 npm install");
        process.exit(1);
    }
    console.log(`[gpui-ts] 1/2 esbuild bundle ${entry} → ui/dist/main.js（QuickJS 直接 eval）`);
    run("node", [path.join(uiDir, "build-qjs.mjs"), "--entry", entryPath], { cwd: uiDir });

    // 产物必须落在 ui/dist/main.js —— quickjs.rs 用 include_str! 在编译期内嵌它
    const bundle = path.join(uiDir, "dist", "main.js");
    if (!existsSync(bundle)) {
        console.error("[gpui-ts] esbuild 未产出 ui/dist/main.js");
        process.exit(1);
    }

    console.log("[gpui-ts] 2/2 cargo build --release（QUICKJS_EMBED=1：内嵌 QuickJS + bundle）");
    run(
        "cargo",
        ["build", "--release", "--manifest-path", path.join(root, "host", "Cargo.toml")],
        { env: { ...process.env, QUICKJS_EMBED: "1" } }
    );
    if (!existsSync(HOST_EXE)) {
        console.error(`[gpui-ts] 未找到产物 ${HOST_EXE}`);
        process.exit(1);
    }
    emit();
    process.exit(0);
}

// ===========================================================================
// backend: perry — perry compile staticlib → 链接 → 单 exe
// ===========================================================================
// staticlib 基名：仅安全字符（进 MSVC link 命令行）
const libBase =
    "app-" + path.basename(entry).replace(/\.[jt]sx?$/, "").replace(/[^A-Za-z0-9_]/g, "_");

const libDir = path.join(uiDir, "node_modules", "@perryts", "perry-win32-x64", "lib");
if (!existsSync(path.join(libDir, "perry_runtime.lib"))) {
    console.error("[gpui-ts] 未找到 perry 运行时库，请先在 ui/ 下 npm install");
    process.exit(1);
}
const perrySrc = process.env.PERRY_WORKSPACE_ROOT ?? "E:\\AI-workspace\\perry-src";
const useSizeOpt =
    process.env.PERRY_NO_SIZE_OPT !== "1" &&
    existsSync(path.join(perrySrc, "crates", "perry-runtime", "Cargo.toml"));

console.log(`[gpui-ts] 1/3 perry compile ${entry} → build/${libBase}.lib (staticlib)`);
run(
    "npx",
    [
        "perry",
        "compile",
        path.relative(uiDir, entryPath).replace(/\\/g, "/"),
        "--output-type",
        "staticlib",
        "-o",
        path.join(buildDir, libBase),
    ],
    {
        cwd: uiDir,
        env: {
            ...process.env,
            PERRY_RS4GC: "0",
            PERRY_RUNTIME_DIR: libDir,
            ...(useSizeOpt ? { PERRY_WORKSPACE_ROOT: perrySrc, PERRY_SIZE_OPT: "z" } : {}),
        },
    }
);
if (!existsSync(path.join(buildDir, `${libBase}.lib`))) {
    console.error("[gpui-ts] perry 未产出 staticlib（可能因源码 checkout 与编译器版本不一致被拒绝）");
    process.exit(1);
}

console.log("[gpui-ts] 2/3 cargo build --release（单 exe 链接：gpui + perry runtime）");
run("cargo", ["build", "--release", "--manifest-path", path.join(root, "host", "Cargo.toml")], {
    env: { ...process.env, PERRY_STATIC_LIB_BASE: libBase },
});
if (!existsSync(HOST_EXE)) {
    console.error(`[gpui-ts] 未找到产物 ${HOST_EXE}`);
    process.exit(1);
}

console.log("[gpui-ts] 3/3 拷贝产物");
emit();
