// Perry 原生编译脚本：把 TS 前端编译成原生可执行文件（不含 Node/V8）。
//
// 绕过 perry 0.5.1520 在 Windows 上的两个已知问题：
// 1) PERRY_RS4GC=0 —— RS4GC stackmap 生成的 .seh_* 指令会被 perry 自带汇编器
//    拒绝（unknown directive .seh_startepilogue，与上游 #7354 同源）；
//    关闭 RS4GC 后 codegen 全部通过（本项目代码无 try/catch、无 for...of）。
// 2) PERRY_RUNTIME_DIR —— 链接期需要 perry_runtime.lib，npm 包实际附带该库，
//    但以 zstd 压缩发布（lib/*.lib.zst），首次运行先解压再指向该目录。
//
// 体积优化（实测 17.1MB → 5.8MB，-66%）：
//   PERRY_WORKSPACE_ROOT 指向 perry 源码 checkout（需含 crates/perry-runtime），
//   触发 auto-optimize：按应用 imports 只重编所需 feature 的 stdlib
//   （本应用只需 async-runtime）+ panic=abort；PERRY_SIZE_OPT=z 用 opt-level=z。
//   注意：① 源码 checkout 必须与 perry 版本一致（0.5.1520 → git checkout v0.5.1520），
//   否则有 undefined symbol 风险；② 重编约 7 分钟（hash 缓存，二次秒用）；
//   ③ PERRY_SIZE_LTO=fat 可再压（编译更慢，未启用）。
import { spawnSync } from "node:child_process";
import { existsSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const uiDir = path.dirname(fileURLToPath(import.meta.url));
const libDir = path.join(uiDir, "node_modules", "@perryts", "perry-win32-x64", "lib");

// 用法：node build-perry.mjs [outBase] [--staticlib]
//   默认：编译独立可执行文件 → ../build/perry-app(.exe)
//   --staticlib：编译静态库（供 host build.rs 嵌入链接）→ ../build/perry-app-static(.lib)
const staticlib = process.argv.includes("--staticlib");
const out =
    process.argv[2] && !process.argv[2].startsWith("--")
        ? process.argv[2]
        : path.join("..", "build", staticlib ? "perry-app-static" : "perry-app");
const compileArgs = ["perry", "compile", "src/main.ts", "-o", out];
if (staticlib) compileArgs.push("--output-type", "staticlib");

if (!existsSync(path.join(libDir, "perry_runtime.lib"))) {
    console.log("[perry] 首次运行：zstd 解压 perry 静态库...");
    const zsts = ["perry_runtime.lib.zst", "perry_stdlib.lib.zst", "perry_ui_windows.lib.zst"]
        .filter((f) => existsSync(path.join(libDir, f)));
    if (zsts.length === 0) {
        console.error(`[perry] 未找到 ${libDir} 下的 .lib.zst，请先 npm install`);
        process.exit(1);
    }
    for (const f of zsts) {
        const r = spawnSync("zstd", ["-d", "-f", f, "-o", f.replace(/\.zst$/, "")], {
            cwd: libDir,
            stdio: "inherit",
        });
        if (r.error || r.status !== 0) {
            console.error(`[perry] zstd 解压失败（PATH 里没有 zstd？msys2 自带）。`);
            console.error(`[perry] 手动方式：cd "${libDir}" && zstd -d *.lib.zst`);
            process.exit(1);
        }
    }
}

// size-optimized stdlib：默认指向本地 perry-src（版本必须与 npx perry 一致）。
// 设 PERRY_NO_SIZE_OPT=1 可跳过（回落预编译全量 stdlib，17MB）。
const perrySrc = process.env.PERRY_WORKSPACE_ROOT ?? "E:\\AI-workspace\\perry-src";
const useSizeOpt =
    process.env.PERRY_NO_SIZE_OPT !== "1" &&
    existsSync(path.join(perrySrc, "crates", "perry-runtime", "Cargo.toml"));
if (useSizeOpt) {
    console.log(`[perry] size-optimized stdlib: ${perrySrc}（首次约 7 分钟 cargo 重编，之后走缓存）`);
} else {
    console.log("[perry] 使用预编译全量 stdlib（体积约 17MB；需要 perry-src checkout 以启用裁剪）");
}

const r = spawnSync("npx", compileArgs, {
    cwd: uiDir,
    stdio: "inherit",
    shell: process.platform === "win32",
    env: {
        ...process.env,
        PERRY_RS4GC: "0",
        PERRY_RUNTIME_DIR: libDir,
        ...(useSizeOpt
            ? { PERRY_WORKSPACE_ROOT: perrySrc, PERRY_SIZE_OPT: "z" }
            : {}),
    },
});
process.exit(r.status ?? 1);
