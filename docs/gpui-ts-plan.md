# gpui-ts：TypeScript → 原生 EXE Runtime

> 定位：一个把 TypeScript 代码编译为原生 EXE 的桌面开发方案。
> gpui 负责窗口系统与原生渲染，TS 编译产物负责业务与 UI 逻辑。

## 项目愿景（原文要点）

**核心价值**：TypeScript 直接编译到原生桌面应用。开发者用熟悉的 TS/SolidJS 写 UI，
编译产物是一个原生 EXE，不携带 Node/V8/WebView。生态未成熟时先走"自建树"路线，
成熟后可平滑替换运行时层。

**四层架构**：
1. TS 编译层 —— perry 编译器，`tsc -p` + `--target native`，输出原生指令
2. 逻辑运行时 —— perry app runtime，内嵌 IR/字节码，提供 setTimeout/fetch/console 等 API
3. UI FFI 桥 —— `@perryts/native` ↔ gpui：声明式树（create/append/setText）转发给 gpui
4. 渲染宿主 —— Rust gpui 独立进程或同一进程，接收指令驱动原生渲染

**关键技术路径**：优先"原生 EXE"（PerryTS 成熟后视作编译输出端）；架构必须兼容
"先 ship 再演进"；UI 层与逻辑层解耦，双进程 JSONL 单向命令流，为将来"内嵌 IR"
（从字符串源码换成静态编译产物）留后门。

**阶段规划**：P0 可视验证 → P1 交互闭环 → P2 打包分发 → P3 性能基准。

## 实测验证状态（2026-09-22 · 单 exe 全绿）

> 下表是 **Perry 后端**时期的验证记录。当日稍晚运行时层已替换为**内嵌 QuickJS**并
> 转正为默认后端，其验收见本文末尾「运行时层替换」一节——那里 supersede 了本表。

| 项 | 结论 | 证据 |
|---|---|---|
| P0 可视验证 | ✅ perry 原生编译，UI 与 node 模式逐字节一致 | build/demo-perry-native.png |
| P1 交互闭环 | ✅ 点击延迟 255ms→24.7ms（10ms 心跳修复 perry 500ms I/O 量子） | build/analyze-trace.mjs |
| P2 体积 | ✅ 17.1MB → 5.8MB（-66%），PERRY_WORKSPACE_ROOT + PERRY_SIZE_OPT=z | ui/build-perry.mjs |
| P2 单 exe | ✅ **正式版跑通**：host 16.4MB 单文件零子进程，UI 完整、交互全通 | build/shot-embedded*.png |
| P2 CLI | ✅ `node scripts/gpui-ts.mjs [entry.ts] [-o out.exe]` 一键 TS→EXE | scripts/gpui-ts.mjs → build/dist/main.exe 15.7MB |
| P3 性能基准 | ✅ 嵌入模式 click→前端派发 p50=6ms（4/6/6/5/6ms×5 次，判据 ≤50ms） | build/lat-run.log |
| P2 体积判据（修正） | ✅ 单 exe 16.45MB ≤ 双进程 host(10.37MB)+前端(6.05MB)=16.42MB —— 同体积换取单文件零子进程 | host/target{,-noembed}/release |

### 单 exe 收尾修复的两个 bug（2026-09-22 夜）

1. **计数器文本空白**：`text(count)` 传数字 signal，setText op 携带 JSON number，
   host `parse_op` 严格 `as_str()` 整条丢弃。修复：runtime.ts `String()` 收敛（setText/text），
   tree.rs 对 number 容错收字符串。
2. **嵌入模式 stdin 断链**（clicks=0、addTask 无效）：perry `perry_poll()` 覆盖
   microtasks+timers，但 stdlib pump（readline stdin 派发）走 `STDLIB_PUMP_FN`
   间接注册链——在 `/FORCE:MULTIPLE` 符号去重边界上注册/读取可能落在不同实例，
   嵌入模式下 pump 永不执行。修复：host 直接 `unsafe extern` 调用 stdlib 导出符号
   `js_stdlib_process_pending()`（10ms tick 与 perry_poll 并跑），绕开间接链。
   注：在 perry-src 插桩定位期间触发了 perry 的 "archive may be stale" 保护
   （source hash ≠ commit 时拒绝打包 staticlib），插桩已全部回滚，perry-src 保持干净。

## 双进程 ⇄ 单进程：两条 IPC 路线（已实测可互换）

- 双进程（现状，保留为对照模式）：host.exe 启动 perry-app.exe 子进程，JSONL over stdio
- 单进程（**默认，全绿**）：staticlib 链入 host，SetStdHandle 重定向到进程内匿名 pipe，
  **JSONL 协议零改动**，`perry_module_init()` + `perry_poll()` + `js_stdlib_process_pending()` 驱动
- 关键配方：① module_init 前预写一行 stdin probe（空 pipe 同步读阻塞）；②
  `/FORCE:MULTIPLE`（perry_runtime 内嵌 std 与宿主 std 符号重复）；③ 嵌入事件
  循环 API：perry_poll / perry_has_work / perry_next_wake_ms / perry_set_wake_callback；
  ④ **stdlib pump 必须显式调 `js_stdlib_process_pending`**（STDLIB_PUMP_FN 间接注册链
  在 /FORCE:MULTIPLE 下不可靠——timers 正常而 stdin 派发消失的怪象即源于此）；
  ⑤ perry-src 源码必须保持与 commit 一致（改动后 perry 拒绝打包 staticlib：
  "archive may be stale"，source hash ≠ 编译器 commit）

## 版本与构建纪律

- npx perry = 0.5.1520；perry-src 必须 `git checkout v0.5.1520` 对齐（否则 undefined symbol）
- auto-optimize 重编 ~7.5min（hash 缓存后秒用）；PERRY_NO_SIZE_OPT=1 可退回全量 stdlib
- PERRY_RS4GC=0（上游 .seh 汇编 bug）；PERRY_RUNTIME_DIR 指向 zstd 解压后的 .lib 目录

---

# 运行时层替换：Perry → 内嵌 QuickJS（2026-09-22 · 已转正为默认）

> 对应愿景里的「**成熟后可平滑替换运行时层**」。协议、前端源码、`tree.rs` 解析器
> 全部零改动——只换了「谁的 JS 引擎在跑前端」。

## 为什么换

Perry 路线的怪病都来自它的事件循环与链接方式，不是 TS 前端的问题：

| 问题 | 根因 | QuickJS 路线 |
|---|---|---|
| stdin 事件 ~500ms 量子聚簇 | 事件循环空闲时按 ~500ms 轮询 stdio | 自拥 10ms tick，直接 drain 队列 |
| 嵌入模式 stdin 静默失效 | `STDLIB_PUMP_FN` 间接注册链在 `/FORCE:MULTIPLE` 下注册/读取分家 | 无此间接层，宿主直接派发 |
| `/FORCE:MULTIPLE` 符号分家 | perry_runtime 内嵌第二份 Rust std | 不内嵌第二份 std，不再需要该开关 |
| archive-stale 保护 | 改 perry-src 即拒绝打包 staticlib | 无 perry-src 依赖 |
| `PERRY_RS4GC=0` | 上游 `.seh` 汇编 bug | 无 |

体积也顺带降了：QuickJS C 核远小于 perry runtime（5.9MB）。

## 架构（换层后）

```
ui/src/*.ts ──esbuild(IIFE, charset=ascii)──▶ ui/dist/main.js
                                                    │ include_str! 编译期内嵌
                                                    ▼
              host 内嵌 QuickJS（rquickjs）独立线程 ──JSONL──▶ tree.rs ──▶ GPUI
                       10ms 自驱 tick：promise jobs + timers + stdin 派发
```

- `process` 是宿主提供的 shim（QuickJS 无 `process` 全局）：`stdout/stderr.write`、
  `stdin.setEncoding/on`、`env`、`argv`、`exit`（真退出，`std::process::exit`）
- 计时器（`setInterval/setTimeout/clear*`）与 `console.*` 全部在 **JS bootstrap** 里定义，
  回调存 JS 全局（`__stdinCbs`/`__timers`）——避免 Rust 侧注册回调时捕获 `Ctx` 造成借用逃逸
- 线程模型：`Context` 是 `!Send`，引擎独享一个 OS 线程；宿主只能通过 pipe + 共享队列通信，
  Windows raw handle 跨线程以 `usize` 传递

## 验收（C1-C7 全绿）

| 项 | 判据 | 实测 |
|---|---|---|
| C1 零子进程 | 不 spawn node | `node.exe` 计数启动前后 4→4；`quickjs.rs` 零 spawn 调用 |
| C2 UI+交互 | 点击闭环 | 8 次点击 → 计数 0→**8** 精确匹配；mutation 批 33→100、累计 292→412 |
| C3 延迟 | click→前端派发 p50 ≤50ms | n=8 **min 2 / p50 8 / avg 7.5 / max 16 ms** |
| C4 体积 | — | **11.01MB**（perry 16.45MB，**-5.4MB / -33%**） |
| C5 冷启动 | — | 启动→UI hello **254ms**（引擎 boot 345.6µs） |
| C6 CLI | 一键出 exe | `node scripts/gpui-ts.mjs` → `build/dist/main.exe` 11.0MB |
| C7 文档 | — | 本节 + README |
| 抗丢帧 | 快速连点不丢 | 8 连点 @161ms 间隔（<perry 500ms 量子）**8/8 全中** |
| 单文件自洽 | 无 sidecar | 拷到空目录从任意 cwd 运行：hello + 239-op mount + 窗口正常 |

## 集成时踩到的坑（都已修）

1. **esbuild `charset` 必须设 `ascii`**（最隐蔽）。Latin-1 补充区码点（如标题里的
   `×` U+00D7）esbuild 会直接吐**单个 0xD7 字节**（非法 UTF-8），而 CJK 正常转义。
   QuickJS 解析器对 UTF-8 严格，报 `SyntaxError: unexpected token` 且**位置误导性地
   指向 `<input>:1:9`**（整条 IIFE 头部），极易误判为语法特性不支持。
   设 `charset:"ascii"` 后所有非 ASCII 一律 `\uXXXX`。
2. **bundle 用 `include_str!` 而非 `cargo:rustc-env`**：后者在 Windows 下多行值换行被
   破坏，且需处理转义；`include_str!` 由 rustc 记录依赖，bundle 变了自动重编。
3. **`build.rs` 的 `rerun-if-env-changed` 必须无条件声明**（在早退分支之前）。否则
   cargo 在 perry 模式下缓存 build.rs 输出，之后设 `QUICKJS_EMBED=1` 不重跑，
   表现为「以为切了 QuickJS，其实跑的还是 perry 嵌入模式」。
4. **`std::process::exit` 作闭包体时返回类型是 `!`**，不是 `IntoJs`；需显式标注 `-> ()`。
5. **不要在 Rust 侧注册 JS 回调**（`stdin.on`/`console`）：闭包捕获 `Ctx` 会触发
   `borrowed data escapes`。全部下沉到 JS bootstrap。
6. **源码里别写裸 NUL 字节**（曾用 `"\0__qjs_eof__\0"` 当 stdin EOF 哨兵）：文件虽合法
   UTF-8，但 grep/diff 等工具会当成二进制。改用 `"\u{0}..."` 转义。
7. `rquickjs` 细节：`Args::push_arg`（非 `push`）；`Rest`/`Args` 在 `rquickjs::function::`
   子模块；`Function::new` 的闭包返回类型必须实现 `IntoJs`。

### 排障工具：`qjsprobe`

上面第 1、2 类失败（UTF-8 / 语法）对 `node` 和 esbuild 自身都不可见，且 QuickJS 的
报错位置会误导。`host/examples/qjsprobe.rs` 直接在当前编译的 QuickJS 上诊断：

```bash
cd host
cargo run --release --example qjsprobe -- ../ui/dist/main.js
#   [utf8] valid UTF-8 (14573 bytes)
#   [qjs] OK — parsed and evaluated            → exit 0
#
#   或（坏样本）：
#   [utf8] INVALID at byte 9 (line ~1): "var t = \"\u{fffd}\";"
#          fix: esbuild option `charset: "ascii"`  → exit 1
```

它先以**字节**读入做严格 UTF-8 校验（报出精确 byte offset），再注入 `process`/定时器
stub 后 eval，从而把「解析失败」与「运行时缺全局」区分开。

## 后端切换

- **默认 quickjs**：`node scripts/gpui-ts.mjs`（或 `GPUI_TS_BACKEND`）
- perry 后端保留为对照：`node scripts/gpui-ts.mjs --backend perry`
- 亦可用环境变量直接驱动 host：`QUICKJS_EMBED=1 cargo build --release`（host 目录下）

