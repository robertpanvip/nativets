// gpui-ts — TypeScript → 单文件原生 EXE 编译器 CLI
//
//   node scripts/gpui-ts.mjs [entry.ts] [-o out.exe] [--backend quickjs|perry]
//
// 两个后端，协议与前端源码完全一致（JSONL mutation 协议），只是运行时层不同：
//
//   ── quickjs（默认）──────────────────────────────────────────────────────
//   1. esbuild 把 TS 打包成单个 IIFE JS（ui/dist/main.js，charset=ascii）
//   2. host 内嵌 QuickJS 引擎（rquickjs），bundle 用 include_str! 编进 exe，
//      自驱事件循环（promise jobs + timers + 事件派发；事件到达即唤醒，
//      不必等下一个 tick）
//   3. 协议不走 stdio：宿主注入 __hostEmit 收 op，事件直灌引擎入站队列
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
import { existsSync, mkdirSync, copyFileSync, statSync, readFileSync, readdirSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

import { pretranspileTsx } from "../ui/pretranspile-tsx.mjs";

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

/**
 * 断言 exe 里真的含有这一版前端的代码（仅 perry 后端需要）。
 *
 * perry 的 app archive 是在 /FORCE:MULTIPLE 下链进宿主的：archive 名字与
 * build.rs 期望的路径一旦对不上，链接照样成功、应用照样启动，只是跑的是
 * **上一版冻结的源码**——界面一动不动，且宿主日志里没有任何告警。
 * 这个坑花掉过一整轮排查（docs/gpui-ts-plan.md 坑 #7）。
 *
 * 哨兵选法：从 gen 产物（即真正喂给 perry 的那份代码）里抽带 CJK 的字符串
 * 字面量，均匀取几条去 exe 二进制里找。CJK 字面量在 perry 运行时是原样
 * UTF-8 存储的，所以 indexOf 就能验证。全部命中才算通过——陈旧 archive
 * 必然缺掉新版才有的字面量。
 */
function verifyEmbeddedApp(exePath, genDir) {
    if (process.env.PERRY_SKIP_EMBED_VERIFY === "1") return;
    const literals = [];
    for (const f of readdirSync(genDir).filter((f) => f.endsWith(".ts"))) {
        const src = readFileSync(path.join(genDir, f), "utf8");
        for (const m of src.matchAll(/"([^"\n]{4,})"/g)) {
            // gen 产物是 charset=ascii，CJK 全部写成 \uXXXX / \xNN
            const s = m[1].replace(
                /\\(?:u([0-9a-fA-F]{4})|x([0-9a-fA-F]{2}))/g,
                (_, u, x) => String.fromCharCode(parseInt(u ?? x, 16))
            );
            if (/[\u3400-\u9fff]/.test(s) && !s.includes("\\")) literals.push(s);
        }
    }
    const uniq = [...new Set(literals)];
    if (uniq.length === 0) return; // 没有可用哨兵（非 CJK 项目），跳过

    const step = Math.max(1, Math.floor(uniq.length / 5));
    const probes = [];
    for (let i = 0; i < uniq.length && probes.length < 5; i += step) probes.push(uniq[i]);

    const buf = readFileSync(exePath);
    const missing = probes.filter((p) => buf.indexOf(Buffer.from(p, "utf8")) < 0);
    if (missing.length === 0) {
        console.log(`[gpui-ts] ✔ 嵌入校验：exe 命中当前前端字符串 ${probes.length}/${probes.length}`);
        return;
    }
    console.error(`[gpui-ts] ✘ 嵌入校验失败：exe 缺少当前前端的 ${missing.length} 条字符串`);
    for (const p of missing) console.error(`[gpui-ts]     · ${p}`);
    console.error("[gpui-ts]   说明：链接成功但内容陈旧 —— 多半是 build.rs 链到了别的 app archive。");
    console.error("[gpui-ts]   检查 build/ 下各 .lib 的时间戳，以及 build.rs 解析的 PERRY_STATIC_LIB_BASE。");
    console.error("[gpui-ts]   确认无误时可用 PERRY_SKIP_EMBED_VERIFY=1 跳过。");
    process.exit(1);
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

console.log("[gpui-ts] 1/4 esbuild 预转 TSX → ui/build/gen（perry 解析器无 JSX 分支）");
const gen = await pretranspileTsx(uiDir, path.basename(entryPath));

console.log(`[gpui-ts] 2/4 perry compile ${gen.entryRel} → build/${libBase}.lib (staticlib)`);
run(
    "npx",
    [
        "perry",
        "compile",
        gen.entryRel,
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

console.log("[gpui-ts] 3/4 cargo build --release（单 exe 链接：gpui + perry runtime）");
run("cargo", ["build", "--release", "--manifest-path", path.join(root, "host", "Cargo.toml")], {
    env: { ...process.env, PERRY_STATIC_LIB_BASE: libBase },
});
if (!existsSync(HOST_EXE)) {
    console.error(`[gpui-ts] 未找到产物 ${HOST_EXE}`);
    process.exit(1);
}

console.log("[gpui-ts] 4/4 拷贝产物");
emit();
verifyEmbeddedApp(out, gen.dir);
