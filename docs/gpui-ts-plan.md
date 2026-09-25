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
| stdin 事件 ~500ms 量子聚簇 | 事件循环空闲时按 ~500ms 轮询 stdio | 自拥 tick + `unpark` 就地唤醒，不等 tick |
| 嵌入模式 stdin 静默失效 | `STDLIB_PUMP_FN` 间接注册链在 `/FORCE:MULTIPLE` 下注册/读取分家 | 无此间接层，宿主直接派发 |
| `/FORCE:MULTIPLE` 符号分家 | perry_runtime 内嵌第二份 Rust std | 不内嵌第二份 std，不再需要该开关 |
| archive-stale 保护 | 改 perry-src 即拒绝打包 staticlib | 无 perry-src 依赖 |
| `PERRY_RS4GC=0` | 上游 `.seh` 汇编 bug | 无 |
| **传输被迫走 stdio 管道** | 引擎与宿主是两个进程/两套运行时 | **进程内引擎 → 注入宿主函数 + 直灌队列**（见下） |

体积也顺带降了：QuickJS C 核远小于 perry runtime（5.9MB）。

## 架构（换层后）

```
ui/src/*.ts ──esbuild(IIFE, charset=ascii)──▶ ui/dist/main.js
                                                    │ include_str! 编译期内嵌
                                                    ▼
              host 内嵌 QuickJS（rquickjs）独立线程 ──▶ tree.rs ──▶ GPUI
                自驱 tick：microtask → timers → 事件派发
```

- `process` 是宿主提供的 shim（QuickJS 无 `process` 全局）：`stdout/stderr.write`、
  `stdin.setEncoding/on`、`env`、`argv`、`exit`（真退出，`std::process::exit`）
- 计时器（`setInterval/setTimeout/clear*`）与 `console.*` 全部在 **JS bootstrap** 里定义，
  回调存 JS 全局（`__stdinCbs`/`__timers`）——避免 Rust 侧注册回调时捕获 `Ctx` 造成借用逃逸
- 线程模型：`Context` 是 `!Send`，引擎独享一个 OS 线程；宿主只经共享队列 + 注入的宿主
  函数与它通信，Windows raw handle 跨线程以 `usize` 传递

### 传输层：把「进程内」这件事用起来（2026-09-22 晚）

换掉引擎后最该顺手改掉的是**传输层**：既然 QuickJS 在进程内，就没必要继续伪造一条
stdio 管道。协议与载体解耦成两条，运行时二选一（**同一个 bundle / 同一个 exe**，
前端用 `typeof __hostEmit === "function"` 探测载体，所以切换不需要重新编译）：

| | `direct`（默认） | `pipe`（历史基线） |
|---|---|---|
| 出站 ops | 注入宿主函数 `__hostEmit(line)` → `ingest_line` → ops 通道 | `process.stdout.write` → 管道 → 读线程逐行解析 |
| 入站事件 | `EventSink::push` 直灌队列 + `thread::unpark` | 写线程 → 管道 → 读线程 → 队列（引擎下个 tick 才取） |
| 额外线程 | 0 | 2 |
| 额外 syscall | 0 | 管道写+flush、阻塞读、行切分 |
| 引擎 boot | **129.6µs** | 1.27ms |
| 事件送达（宿主时钟） | **avg 0.10ms / max 1ms** | avg 0.10ms 但偶发 **191ms** 停顿 |
| 端到端派发 | **≤测量分辨率**（20 样本中 12 个为负，即落在 ±1ms 时钟偏差内） | p50 6ms，**醒目的 10ms 量化簇**（9/10/10/10…）+ 偶发 202/234ms |

两点值得记：

1. **延迟差异来自唤醒方式，不是带宽**。`pipe` 侧 click→写入管道只要 ~0ms，但事件要等
   引擎下一个 tick 去取队列，于是端到端出现 10ms 量化簇 —— 那正是 tick 周期。
   `direct` 用 `unpark` 就地唤醒，事件一落地就派发。tick 仍保留（驱动 timers）。
2. **微任务要分相位 drain**。前端用 `Promise.resolve().then(flush)` 批处理 ops，而
   `pump()` 原先是「先 drain 再派发事件」，于是点击处理器产生的 flush 要等到**下一个
   tick** 才跑，白加一个 TICK 的可见延迟。现在 timers 之后、事件派发之后再各 drain 一次。

传输选择：
```bash
build/dist/main.exe --transport=pipe          # 或 GPUI_TS_TRANSPORT=pipe
build/dist/main.exe --transport=direct        # 默认
```
未知取值只警告并回落到 `direct`，不会拒绝启动。宿主两条路径共用同一个
`main.rs::ingest_line`，所以载体不同不可能漂移出两套协议方言。

## JSX / TSX 前端（2026-09-22 深夜）

目标：「现在 TS 端还不是 TSX，我期望是 TSX」。JSX 只在**构建期**展开成运行时已有的
`h()` 工厂，所以协议层、host、tree.rs 全部零改动：

```
<div style={{ padding: 16 }}>hi {n}</div>
  --esbuild(jsx: transform, jsxFactory: h)-->  h("div", { style: { padding: 16 } }, "hi ", n)
```

### runtime.ts 为 JSX 补的三件事

| 新增 | 作用 |
|---|---|
| `Fragment`（字符串哨兵 `"#fragment"`） | `<></>` → `h(Fragment, null, …)`；host 没有 fragment 概念，落成透明 flex-column 容器 |
| `h()` 的**函数 tag 分支** | 大写标签＝组件：`<Card title/>` → `h(Card, props, …)` → `Card({…props, children})` |
| `appendChildren()`（递归展平数组） | `{items.map(…)}` 是把数组当**一个实参**交给工厂的，原来会被整块 append 给 host |

`h()` 对元素节点的既有语义**完全没动**（`onClick` → setEvents、字符串 children →
子 text 节点），所以 JSX 版与手写 `h()` 版逐节点等价。

### 样式收敛到 `style`（2026-09-22 23:45）

最初把样式键平铺在 props 上（`<div padding={16} background={C.card}>`）。这不合适：
组件的自有 props 与样式键在**同一平面**，`<StatCard color={C.green}>` 里的 `color` 是
组件 prop、而 `<div color=…>` 里的 `color` 是样式，同名不同义；也看不出哪些属性真的
会进 host 的样式引擎。

改为样式走唯一的 `style` prop：

```tsx
<div style={{ padding: 16, background: C.card }}>…</div>
```

- `h()` 只读 `props.style` / `props.onClick` / `props.text`，其余内在标签属性**一律忽略**
  （故意不保留平铺写法——留两条路等于没收敛）。
- `globals.d.ts` 的 `JSX.IntrinsicProps` 刻意**不加索引签名**，于是
  `<div padding={16}>` 在编辑器里直接报类型错误，而不是静默变成无样式元素。
- 组件照旧拿到 props 原样（`style` 也在里面），自己决定怎么用；`PillBtn` 的
  `bg`/`fg` 因此可以放心叫这个名字而不会和样式键冲突。
- 顺手把 `Style` 类型导出，`text()` / `setRootStyle()` / 组件内部的样式对象都标注它。

协议与 host 零改动：`style` 对象仍然是 `setStyle` op 的载荷原样。

### `app.ts` → `app.tsx`

整份重写为 JSX 并顺手组件化（`Card` / `StatCard` / `PillBtn` / `NavItem` / `TaskRow` /
`Header` / `Sidebar` …）：既贴近 TSX 习惯，也把嵌套压浅一层——perry 对实参位置的多层调用
敏感（见「坑 #3」），组件化正好把调用链摊平。`App()` 里挂载用的 JSX 先绑局部变量再传参。

### perry 后端：多一步预转（perry 0.5.1520 解析器没有 JSX 分支）

`perry check src/app.tsx` 直接回 **"No TypeScript files found"**——入口只认 `.ts`，
解析器也没有 JSX 分支。于是 perry 路径加了一步 `ui/pretranspile-tsx.mjs`：

```bash
# esbuild：不 bundle、保留 ESM 结构与 import 说明符（"./runtime" 原样保留），
#          JSX → h()、擦类型、charset=ascii，产物落 ui/build/gen/*.ts
npx esbuild <src/*.ts|tsx 全量> --outdir=ui/build/gen --format=esm \
    --jsx=transform --jsx-factory=h --jsx-fragment=Fragment \
    --charset=ascii --out-extension:.js=.ts
# 再交给 perry（入口形态与以前完全一致）
npx perry compile build/gen/main.ts --output-type staticlib -o ../build/app-main
```

两个易错点：
1. **必须显式列出所有源文件**——`bundle: false` 时 esbuild 不跟随 import，只转点名的文件；
   目录扫描要排除 `*.d.ts`（纯声明，不应产出）。
2. 输出用 `outExtension: { ".js": ".ts" }` 改回 `.ts`，否则 perry 的入口校验不认。

另：`ui/pretranspile-tsx.mjs` 是**导出函数的模块**（`pretranspileTsx(uiDir, entryName)`），
由 `build-perry.mjs` / `scripts/gpui-ts.mjs` 调用；直接 `node pretranspile-tsx.mjs` 只会
import 一下然后静默退出，什么也不生成——排查时容易误判成"预转成功"。

### 编辑器支持（不参与构建）

`ui/tsconfig.json`（`jsx: "react"` classic + `jsxFactory: h` + `jsxFragmentFactory: Fragment`）
与 `ui/src/globals.d.ts`（宿主 `process` shim 的最小声明 + 全局 `JSX` 命名空间）只为编辑器 /
`tsc` 服务——esbuild、perry 都不读它们，项目也刻意不引 `@types/node`。

### 验收

| 项 | 结果 |
|---|---|
| QuickJS 后端 | `node scripts/gpui-ts.mjs` → **11.0MB** exe；UI 与改造前一致；点 `+` 计数 0→2；点「日志」导航高亮 + footer 同步为 `navigation: 日志` |
| Perry 后端 | `--backend perry`：esbuild 预转 → perry staticlib → **15.7MB** exe；UI 一致；连点 `+` 两下 → 计数 **2** |
| perry 静态检查 | `perry check build/gen` → All checks passed（3 files） |
| 体积 / 延迟 | 与改造前一致（JSX 是编译期糖，运行时零开销） |

> **截图读数教训**：Read 工具显示的图片是**缩放后**的，凭目测读坐标会严重错位（本次
> 把 1180×760 的窗口看成约 660 宽，第一次点击落到了导航栏上）。要拿真实坐标请用像素
> 扫描（`build/scan-row.py` / `build/find-block.py`），并用 `GetWindowRect` +
> `ClientToScreen` 把点击换算成**客户区相对坐标**（`build/winclick.py`）——这样窗口被
> 遮挡、被移动后点击依然落得准。

## 验收（C1-C7 全绿）

| 项 | 判据 | 实测 |
|---|---|---|
| C1 零子进程 | 不 spawn node | `node.exe` 计数启动前后 4→4；`quickjs.rs` 零 spawn 调用 |
| C2 UI+交互 | 点击闭环 | direct：30 次点击 → 计数 **30** 精确匹配（20ms 间隔）；pipe：15 次 → 15 |
| C3 延迟 | click→前端派发 p50 ≤50ms | direct **≤测量分辨率**；pipe p50 6ms（10ms 量化簇）；判据均满足 |
| C4 体积 | — | **11.03MB**（perry 16.45MB，**-5.4MB / -33%**） |
| C5 冷启动 | — | 启动→UI hello **193~235ms**（5 次；引擎 boot 130~257µs） |
| C6 CLI | 一键出 exe | `node scripts/gpui-ts.mjs` → `build/dist/main.exe` 11.0MB |
| C7 文档 | — | 本节 + README |
| 抗丢帧 | 快速连点不丢 | 30 连点 @20ms 间隔（<perry 500ms 量子）**30/30 全中**，协议告警 0 |
| 线程 | — | direct 38 / pipe 41（全进程含 GPUI 线程池，差额 = 预测的 2 条） |
| 单文件自洽 | 无 sidecar | 拷到空目录从任意 cwd 运行：hello + 239-op mount + 窗口正常 |

> 测量方法：宿主在 click handler 与出站投递处各打一个 epoch ms（同一时钟，无跨时钟偏差），
> 前端在 `EVT_RECV` 打 `Date.now()`。前者是严格的宿主侧度量，后者端到端但受
> host/JS 两次取时钟的影响，分辨率约 ±1ms —— 这也是 direct 下 20 个样本里有 12 个
> 出现负值的原因（值小到被时钟抖动淹没），并非丢帧。


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
8. **`pump()` 的相位顺序会直接变成可见延迟**：前端用 `Promise.resolve().then(flush)` 批处理
   ops，若 `pump()` 只在开头 drain 一次微任务，点击处理器产生的 flush 就要等到**下一个
   tick** 才发出 —— 每个交互白加一个 TICK（~10ms）。现在 timers 之后、事件派发之后再各
   drain 一次（微任务在空队列时开销为一次布尔返回）。
9. **别把 `process.stdout.write` 在 direct 模式下也删掉**：前端只在探测到 `__hostEmit`
   时才改走注入路径，其余（含用户自己的 `console`/打印代码）仍可能写 stdout。direct 模式
   把 stdout 改道到诊断 stderr 并加 `[stdout]` 前缀，既保留可观测性又保证它进不了协议通道。

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

正交的一个开关是**传输**（仅 quickjs 后端有，运行期生效、不需要重编译）：

| 取值 | 行为 |
|---|---|
| `direct`（默认） | 宿主注入 `__hostEmit`；事件直灌队列 + `unpark` |
| `pipe` | stdio 匿名管道 + 读写线程（历史基线，便于对照/远程排障） |

选择方式：`--transport=pipe` 或 `GPUI_TS_TRANSPORT=pipe`；未知取值警告后回落 `direct`。


## BOM 层：宿主注入的浏览器环境（2026-09-23 凌晨）

### 定位

前端要写的是 TSX 而不是「被宿主函数包围的裸 JS」，缺的是 `setTimeout`、
`requestAnimationFrame`、`window.innerWidth`、`crypto.randomUUID()` 这些**环境**。

明确不做的事：**不模拟 DOM**。全部 API 挂同一个全局（`window === self === globalThis`，
Web Worker 心智模型），**`document` 故意不存在**——渲染入口只留 `h()` + mutation 协议
一条，不给自己留第二条路。

### 文件分工

| 文件 | 职责 | 为什么单独存在 |
|---|---|---|
| `host/src/bootstrap.js` | JS 环境本体 ~560 行，`include_str!` | 从 `quickjs.rs` 的内联 raw string 提出来的：**能编辑、能被 node 测试直接加载** |
| `host/src/bom.rs` | 窗口度量/单调时钟/熵/弹窗应答 | 只有这 4 件事 JS 自己拿不到 |
| `ui/src/bom.ts` | 前端唯一访问口 + `hasBom` 探测 | Perry 后端无 BOM，降级收敛在一处 |
| `ui/src/globals.d.ts` | 手写环境声明 | 与 `bootstrap.js` 严格对齐，见下 |

### 三个非显然的决定

1. **`rAF` 由引擎 tick 驱动，不是定时器**。`__hostTick` 里顺手调 `__rafMaybeFrame`，
   所以空闲应用不产生帧预算、引擎该 park 就 park；默认 20ms 节奏 = 稳定 50Hz。

2. **`confirm` 真的阻塞 JS 线程**。走独立 `mpsc` 通道等宿主应答——结果**不能**回引擎
   队列，那个线程正卡在这儿等。窗口已销毁时返回默认值（confirm→0 / alert→1），
   绝不永久阻塞。实测：弹窗期间 1 秒定时器**一格不走**。

3. **弹窗由宿主画，不进协议**。模态是宿主的事（像系统消息框），协议里没有它。
   `HostView::build_dialog` 用 `.absolute().inset().occlude()` 铺满 + 遮罩 `rgba(0,0,0,0.6)`。

### 类型层面的取舍

`tsconfig.json` 只留 `"lib": ["ES2020"]`，**故意不引 `lib.dom`**。引了会把 `document`、
`localStorage`、`fetch` 全声明成可用，而宿主一个都没提供 → 运行时才炸。改成按实际能力
手写 `globals.d.ts` 后，**用错东西是编译错误**：

```ts
document.title      // TS2584: Cannot find name 'document'（可预期的失败）
localStorage.setItem // TS2304
window.innerWidth    // OK
```

前端从 0 用不到的东西，写错时立刻知道，而不是在 QuickJS 里看 `undefined`。

### 踩到的坑

1. **`setInterval(fn, 0)` 退化成一次性 timeout**（测试抓到的真 bug）。`__hostTick` 原本
   用 `t.interval > 0` 判断是否重复，0ms 间隔永不重复。改为注册时打 `repeat` 标志 +
   独立 `every` 延时，并加 `__delay(ms)` 归一化。**这条是靠 node 测试发现，不是靠看屏幕。**
2. **`__bomLine` 在重构中被漏掉**：`quickjs.rs` 引用了它，`bootstrap.js` 里没有。
   补上（把短键实时负载解析进 `__bomApply`）并在 `pump()` 里按前缀
   `{"t":"bom"` 拦截——它必须**不进**前端的 stdin 流，否则污染协议。
3. **`__bomApply` 启动期会早于 `dispatchEvent` 定义**（有个 `try` 恰好吞掉了）。
   加了显式的 `typeof globalThis.dispatchEvent === "function"` 守卫——**这是真守卫，
   不是防御性习惯**。
4. **vm 里的跨 realm 内在对象**：测试用 `vm` 加载 `bootstrap.js` 时，`ctx.Uint8Array`、
   `ctx.Date`、`ctx.TypeError` 都不是构造器（realm 内在对象挂在 realm 全局上，不在
   sandbox 对象上）。解法是加 `run(code) = vm.runInContext` 辅助函数，并断言错误**消息**
   （`/could not be cloned/`）而不是构造器。
5. **`node --test host/test` 直接 `MODULE_NOT_FOUND`**：这个 node 把裸目录当模块解析。
   必须传 glob：`node --test "host/test/*.test.mjs"`。
6. **Perry 模式下 7 个 dead-code 警告**（`perf_now_ms`/`entropy_hex`/`DialogKind::*`/
   `bom::show`/`WindowMetrics::*`/`spawn_dialog_host`）：BOM 只服务 QuickJS 前端，属预期。
   用带理由的 `#[cfg_attr(not(quickjs), allow(dead_code))]` 消掉，**两个模式现在都是零警告**。

### 验收（2026-09-23 · 双后端）

宿主侧 `sync_metrics` 在每次 `render` 里推几何（`window.viewport_size()` 逻辑像素 +
`window.scale_factor()` + `cx.primary_display().bounds()`），前端只用实时 getter 读。

| 项 | 判据 | 实测 |
|---|---|---|
| 窗口度量 | 与真实客户区一致 | `GetClientRect` = **1180×760**，BOM 卡片读出 **1180×760** |
| resize 推送 | 改窗口即更新 | 缩到客户区 940×600 → 卡片变 **940 × 600**；宿主日志 `{"t":"bom","w":940,"h":600,…}` |
| alert 模态 | 宿主绘制 + 阻塞 | 遮罩+居中弹窗+「确定」；**时钟冻结在 00:27:13 横跨 52s**，应答后恢复 00:30:39→00:30:44 |
| confirm 模态 | 双分支 | 标题「确认」+「取消/确定」；取消 → 计数仍 **4**；确定 → **0** |
| rAF | 与 tick 同拍 | 600ms 抓拍命中**第 22 / 60 帧**，按钮切「… 动画中」 |
| crypto | 合法 v4 | 两次点击得 `7fbf8b3f-0bfd-4903-b927-…` / `38745ff3-6efe-4a51-ae65-…`（版本位 4、变体位 a/b） |
| 无 BOM 降级 | Perry 下不崩且文案一致 | 卡片渲染 `无 BOM（Perry 后端）` / `-` / `perry-native（无 BOM）`；「重置」走 `!hasBom → true` 分支（计数 4 → 0，宿主无 dialog 请求）；`app.tsx` 零改动 |
| 语义测试 | 脱离 GUI | `node --test "host/test/*.test.mjs"` → **35/35 pass** |
| 类型 | 严格 | `npm run typecheck` **exit 0** |

排障技巧：UI 状态出现「我没动它怎么变了」时，看 **`点击事件回传` 计数**——它等于宿主
收到的 click 数。对不上就说明有误触（本次 UUID 变化即由此定位到：聚焦窗口时的随手点击
落在了「新 UUID」上，而不是信号自发生成）。

> 回归心智：**别用肉眼鉴定信号值是否稳定**。本次两张 UUID 截图都是引擎被 `alert`
> 冻住之后拍的，等于拿冻结帧证明「它没变」。要证明稳定，必须在引擎活着时取样，或者
> 拿时钟一起拍——时钟不动就说明这一帧是死的。

## 排查记录（坑 #7 / #8）：一次「假成功」的链接（2026-09-23 凌晨）

### 症状

Perry 后端里 **BOM 卡片整块不渲染**（布局从计数器直接跳到任务列表），而 QuickJS
后端一切正常。宿主的 `parse_op` 丢弃告警**一条都没有**。

### 走错的四步（都值得记下来）

1. **怀疑 perry codegen 吞掉了组件**。证据看起来很硬：用
   `build/capture-ops.py` 抓 `perry-app.exe`（executable 构建）的真实 ops 流，
   重建整棵树 —— **0 个孤儿节点**，`#64` 下三行 + 标题全都在。同一份源码下
   executable 发 **359** ops、QuickJS bundle 也发 **359**，只有嵌入构建是 **239**。
2. **怀疑是 `bar(frame(), FRAME_TOTAL)` 命中坑 #3**（实参位置多层调用）。绑局部变量
   重构 → op 数**仍是精确的 239**。假设排除。
3. **怀疑是位置/参数元数问题**：把 `BomCard` 和 `TasksCard` 在 `MainPanel` 里对调 →
   第 3 位正常渲染、`BomCard` 换到第 4 位**依然消失**。说明与位置无关。
4. **怀疑是超大深层嵌套表达式压垮 codegen**：拆成
   `BomRowMetrics/BomRowEnv/BomRowActions` 三个小组件 → **还是 239**。

四个假设全部落空，但每一步都留下同一个数字：**239**。这本身就是最强的线索——
四份不同的源码产出完全相同的 op 数，说明**跑的根本不是这些源码**。

> 回归心智：**当 N 次改动都给出同一个数字时，先怀疑自己改的东西没有进产物，
> 而不是继续找代码原因。** 我在这上面绕了三轮。

### 真正的根因

`host/build.rs` 写死了 archive 名：

```rust
let app_src = build_dir.join("perry-app-static.lib");   // ← 永远这一份
```

而 `scripts/gpui-ts.mjs --backend perry` 产出的是 `build/app-main.lib`（基名由入口
文件推出来），并且它把 `PERRY_STATIC_LIB_BASE=app-main` 传给 cargo —— **build.rs 只
声明了 `rerun-if-env-changed` 却从不读这个变量**。于是：

- 链接目标 = `build/perry-app-static.lib`，一份 **09-22 08:08 的冻结产物**（BOM 卡片
  诞生之前）；
- `rerun-if-changed` 也指向旧名，所以新 archive 变了也不会触发重链；
- `/FORCE:MULTIPLE` 把「链错库」变成完全无声的：exe 正常生成、应用正常运行。

**一锤定音的证据是字符串**：`build/app-main.lib` 含
`BOM · 宿主注入的浏览器环境` 等字面量，`build/dist/main.exe` 里**一个都没有**，
但有旧版的 `Perry 编译 TS → 原生二进制`。

```bash
grep -a -c "BOM · 宿主注入的浏览器环境" build/app-main.lib    # 1
grep -a -c "BOM · 宿主注入的浏览器环境" build/dist/main.exe   # 0   ← 链错了
```

### 修复

1. `build.rs` 按 `PERRY_STATIC_LIB_BASE` 解析 archive 与 linkdeps；未指定时取
   `app-main.lib` / `perry-app-static.lib` 中**较新**的一份，并打 cargo warning
   把选择结果说出来（手工 `cargo build` 时不再靠猜）。`rerun-if-changed` 随之指向
   真实文件——否则 archive 更新仍不会触发重链。
2. `scripts/gpui-ts.mjs` 在 `emit()` 之后做**嵌入校验**：从 gen 产物里抽带 CJK 的
   字符串字面量（均匀取 5 条），断言 exe 二进制里都能命中，否则以非零码退出并说明
   怎么查。这就是上面那条手工证据的自动化版本。
3. 顺手把 `build/` 下三份化石 archive 移到 `build/trash/fossil-archives/`，
   避免"取较新"再选到它们。

修复后：`app-main.lib` 被真正链接，`dist/main.exe` **17.0MB**（此前 15.7MB ——
连 runtime 都来自旧 linkdeps），mount ops **359**，BOM 卡片正常渲染。

### 附带发现：跨模块 `hasBom` 不可靠（坑 #8）

修复链接后，Perry 下 BOM 卡片渲染出来了，但 `performance.now() @挂载` 显示 `0 ms`
而不是 `-`。逐项对照：

- `windowSize()` / `dpr()` / `newUuid()` 都在**模块初始化期**求值 → 全是对的降级值；
- 「重置」按钮走 `askConfirm()` → `!hasBom → return true`，计数 4 → 0（宿主日志里
  没有任何 dialog 请求）→ **点击期 `hasBom` 也是 false**；
- 唯一异常是 `app.tsx` 里那个**渲染期**读的 `() => (hasBom ? mountMs + " ms" : "-")`
  → 走了 true 分支。

即：`bom.ts` 内部处处正确，唯独**跨模块 import 这个 `const`** 在 staticlib codegen 下
被读成 true。收敛办法是**不跨模块引用布尔值**：把分支全部留在 `bom.ts`，对外只暴露
`uptimeLabel(ms)`（格式化）与 `raf(): boolean`（返回值）。改完后三项都显示 `-`。

> 判据选择：`app.tsx` 里另一个 ternary（`frameRunning() ? "… 动画中" : "▶ 帧动画 rAF"`）
> 显示的是**后**一个分支，说明 perry 的 ternary 本身没编错 —— 问题只出在
> 被 import 的常量绑定上。用"另一个同构表达式正常"来收窄假设，比继续读 codegen 便宜。

---

# 交互组件与绘制层（2026-09-23 · input / radio / checkbox / switch / select / 滚动 / Canvas）

前面几步把协议从「只有 `div` + `click`」推到了「带状态控件 + 绘制原语 + 滚动容器」。
这一节记的是设计边界和**真正花时间的几个坑**——都不在设计里，全在实现里。

## 目标与边界

保持三条既有边界不动：

1. **宿主只管像素/字体/布局**：caret 位置、滚动偏移、滚动条几何全在宿主算；
   前端说不出「光标在第 3 个字符后面画在哪」。
2. **协议只增不改**：`setValue` / `setCanvas` 与 `setText` / `setStyle` 并列，
   `create` 多一个可选 `placeholder`。旧前端（只发旧 op）行为完全不变。
3. **前端描述，宿主搬运**：canvas 是**显示列表**（纯 JSON），不是命令回调；
   因此它能进日志、能被 diff、双后端行为一致。

## 交付物

| 文件 | 内容 |
| --- | --- |
| `host/src/draw.rs`（新） | 5 原语 `rect/ellipse/line/poly/text`；`parse_color`（hex/rgb/rgba/裸 hex）、`with_alpha`、`parse_cmds`（8192 上限 + skipped 计数）、`scrollbar_thumb`。不依赖 GPUI 类型 |
| `host/src/tree.rs` | `Op::SetValue` / `Op::SetCanvas`；`Node.value/placeholder/caret/canvas`；`is_input` / `is_canvas` / `display_text`；`Tree::ids()` / `node_mut()` |
| `host/src/main.rs` | `build_input` / `build_canvas` / `build_plain` 三分派；焦点句柄与滚动句柄表；`input_key`（编辑/键盘映射）；`sync_focus` / `sync_scroll` 轮询；`paint_cmds` / `paint_field_text` / `scrollbar_overlay` |
| `ui/src/kit.tsx`（新） | `TextField` / `Radio` / `RadioGroup` / `Checkbox` / `Switch` / `Select` / `ScrollArea` / `CanvasView` |
| `ui/src/canvas2d.ts`（新） | 形状兼容 `CanvasRenderingContext2D` 的录制器 |
| `ui/src/theme.ts`（新） | 共用色板 |
| `ui/src/runtime.ts` | `setValue` / `setCanvas` / `setStyle` 导出；事件绑定按 `id:kind` 键控；`childRegistry` + 递归 `forget()`（`For` 重建不再泄漏处理器） |

## 坑 #A：`grow` 只设了 `flex-grow`，高度链整条被撑开

`grow: 1` 原先映射成 `d.flex_grow(x)`。在 `flex-col` 父级里这**只**给 grow 因子，
`flex-basis` 仍是 `auto`（内容高度），于是子项以内容高度为基准、把父级撑开。

症状极隐蔽：窗口看起来正常（`root` 是 `size_full` + 裁剪），但
**主面板滚区的 viewport 等于 content（1198 = 1198，max = 0）**——永远滚不动，
而且 `Footer` 被推到窗口外看不见。

修复：`grow: n` 定义成 CSS `flex: n 1 0%`（同时 `flex-shrink: 1` / `flex-basis: 0` /
`min-w/h: 0`）。修好后同一条滚区变成 `viewport=584 / content=1498 / max=914`。

> 判据：不要靠截图判断"布局对不对"，看 `max_offset`。`viewport == content` 就是
> 高度链没约束，与 CSS 里忘了 `height` 是同一件事。

## 坑 #B：GPUI 的 flex 子项最小尺寸默认是 0（CSS 是 `min-height: auto`）

修完 #A，滚区有了 914px 可滚空间——但 `content` 还是等于 `viewport`。
根因是**滚动容器的子项被等比压缩**：CSS 里 flex 项的 `min-height: auto` 会阻止
"压缩到内容高度以下"，GPUI/taffy 这里是 0，于是 7 张卡片被压进 584px，`content`
自然等于 `viewport`。

修复分两处：

- 宿主：`build_node` 多一个 `no_shrink` 参数，滚动容器的**直接子项**在未显式声明
  `shrink` / `grow` / `flex` 时被钉成 `flex_shrink: 0`；
- 前端：卡片显式写 `shrink: 0`（它同时告诉读者"这张卡的高度就是内容高度"）。

## 坑 #C：`Card` 自带 `grow: 1`，放进滚区就成了抢占者

`Card` 是早期为「几张卡平分有限空间」写的，带 `grow: 1`。
它进了滚动容器之后，在 `flex-col` 里每张卡都要分一杯羹 →
所有卡片等分视口高度、内容溢出卡片边框、视觉上**互相重叠**。

修复：`Card` 的 `grow: 1` 改成 `shrink: 0`。

> 教训：`grow` 是"填满剩余空间"，只在**有界且不滚**的容器里才等于"平分"。
> 内容进入滚区的那一刻，`grow` 就从"排布手段"变成"bug 来源"。

## 坑 #D：`pyautogui.scroll()` 发的不是标准 wheel delta

验收时滚轮"完全没反应"。加日志看到事件**确实到达了**（`root` 上的
`on_scroll_wheel` 有输出），但 delta 是：

```
Lines(Point { x: 0.0, y: -0.025000002 })
```

GPUI 的换算是 `wparam.signed_hiword() / WHEEL_DELTA * 系统设置行数`（120 × 3）。
-0.025 反推出 `hiword = -1` —— pyautogui 在 Windows 上直接发裸 delta ±1，
不是 ±120。于是每格滚 0.025px，看起来就是"滚不动"。

换成 `mouse_event(MOUSEEVENTF_WHEEL, 0, 0, ±120, 0)` 后 delta 变成 `Lines(3.0)`，
滚动立刻正常。**这是测试工具的问题，不是应用的**，但代价是白排查了一轮——
所以写进了 `build/wheel_shot.py` 的文档字符串里。

## 坑 #E：`top` 与 `max` 不在同一个坐标系

GPUI 的 `offset()` 是**平移量**（≤ 0），而 `max_offset()` 是**距离**（≥ 0）。
一开始直接把 `offset.y` 当 `top` 报给前端，于是日志里是
`{"top":-207.0,"max":338.0}` —— 前端要画进度条得先猜符号。

修复：协议里 `top` 定义为**已滚距离**（`-offset.y`），与 `max` 同坐标系；
滚动条绘制（`scrollbar_thumb` 里算的是 `offset / max_offset` 的进度）也随之统一。

## 坑 #F：键盘事件的前提是窗口真的在前台 + 日志真的落地

第一次输入测试完全没进字（输入框仍显示 placeholder），但**边框变蓝了**——
说明 GPUI 内部的 `window.focus(&fh)` 生效了。诊断路径：

1. `GetForegroundWindow() == hwnd` → 窗口确实是前台；
2. 加一行 `input_key` 入口日志 → 键**根本没到**；
3. 换成 `setsid` 后台启动 + 显式重定向 stderr → 键到了。

留下两条纪律：

- 宿主诊断一律走 `log!`（写原始 stderr 句柄），启动时用
  `(setsid ./app >run.log 2>&1 &)`，**别指望默认继承的 stderr**；
- `build/dist/*.exe` 被运行中的窗口占用时 `copyfile` 会 `EBUSY`，重建前先
  `taskkill /F /IM main.exe`。

## 顺手修的两个真 bug

1. **Enter 在值未变时多发一个 `input`**：`(value, caret) == before && !commit`
   这个条件让 `commit` 路径绕过了早退，于是先发 `input` 再发 `change`，前端会把
   它当"又敲了一键"。改成「值变了才发 `input`，`commit` 独立发 `change`」。
2. **`For` 重建时的处理器泄漏**：`remove` / `clearEl` 原先只注销自身的事件处理器，
   子树里的全留着。引入 `childRegistry` + 递归 `forget(e)`。

## 验收（2026-09-23 · QuickJS 默认后端，真机截图）

| # | 项目 | 证据 |
| --- | --- | --- |
| 1 | 首屏 7 张卡片、无重叠、Footer 在位 | `build/kit-A-top.png` |
| 2 | 下拉浮层**盖住**下方卡片（`deferred` 抬升） | `build/kit-B-crop.png` |
| 3 | 选中回填 + 摘要行联动（生产 · 北京 · 详细 · 手动刷新） | `build/kit-E-crop.png` |
| 4 | 输入框真实编辑：`gpui-kit` + 光标在末位 | `build/kit-E-crop.png` |
| 5 | 回车提交 → `change`（且无冗余 `input`） | 日志 `ev change id=13 …"value":"gpui-kit"` |
| 6 | 主面板滚轮滚动 | `viewport=584 / content=1498 / max=914` |
| 7 | Canvas 柱状图 + 趋势线 + 数值标签 | `build/kit-F-canvas.png` |
| 8 | `换一组数据` → 重绘（柱值变、计数 1→2） | `build/kit-G-crop.png` |
| 9 | `切换配色` → 重绘（蓝→绿，计数 →3） | `build/kit-H-crop.png` |
| 10 | 内层滚区 + `scroll` 事件（`top` 正值） | `build/kit-J-crop.png`：`滚动 20% · top 69px · 内容高 506px` |
| 11 | `cargo test`（draw.rs） | 11 passed |
| 12 | `node --test "host/test/*.test.mjs"` | 35 passed |
| 13 | `npm run typecheck` / `npx perry check src/main.ts` | 双双通过 |

体积：单文件 exe **11.5 MB**（组件与绘制层是纯 Rust/TS 增量，没有引入依赖）。

## 已知缺口（同时写进 README）

- 没有 IME 组合输入（中文只能 `Ctrl+V` 粘贴）；
- 点击 input 不按点击位置定位光标（监听器拿不到 bounds），一律落到末尾；
- 没有选区模型（`Ctrl+A` 只是把光标移到末尾）；
- 滚动只有纵向；
- canvas 无 `clearRect` / `transform` / 渐变 / 图片 / 阴影 / 裁剪。

---

# 库化：把工具打成 npm 包 `gpui-ts`（2026-09-23）

## 目标与边界

**目标**：使用者 `npm i gpui-ts` → `npx gpui-ts build src/app.tsx` → 得到单文件
原生 exe。**机器上不需要 Rust / MSVC / 本仓库**，只要 Node ≥ 18。

**非目标**：本次不做多平台宿主、不做 HMR/devtools、不改协议、不改 Perry 后端。

## 机制：尾部追加（tail-append）

宿主二进制随包发布，应用以「script 数据」形式附加在宿主**副本**尾部：

```
app.tsx ──esbuild(IIFE)──► JS ──TAIL_MARKER + JS──► 宿主副本 ──► myapp.exe
                                                                    │
                宿主启动：从 current_exe() 读回最后一个 marker 之后 ◄─┘
```

- Windows PE 加载器忽略 exe 尾部多余字节 → 产物就是普通可执行文件。
- 宿主读取顺序：**尾部追加 → `QUICKJS_BUNDLE` env → `include_str!` 内嵌 demo**。
  尾部优先，因为发布版宿主内嵌的是本仓库的 demo，否则用户 app 会被 demo 顶掉。
- `bundle_from_self()` 只回扫尾部 **8 MB**（`TAIL_SCAN_BYTES`），并在末尾把
  NUL 填充裁掉（`rposition(|b| *b != 0)`）——PE 段对齐可能补零。

### 坑 #G：marker 是跨构件契约，必须在两侧逐字一致

`lib/build.mjs::TAIL_MARKER` ↔ `host/src/quickjs.rs::TAIL_MARKER`。二进制与 JS
是分开发布的，没有编译期检查。另外 `packExe()` **拒绝** bundle 内含 marker——
宿主取的是**最后一个** marker，若 JS 里恰好有一段（比如用户打印了这个常量），
切分点会错位，症状是莫名其妙的语法错误而非"找不到 bundle"。

### 坑 #H：`root` 不是函数（本次由隔离安装暴露）

`createRoot(spec, app)` 传给 `app` 的 `root` 是**元素句柄** `{id: 0}`，
不是回调。写成 `root(<div/>)` 在 QuickJS 里报
`TypeError: not a function | at App (<input>:453:965)` —— 报错位置落在**整个
`root(...)` 表达式的尾部**（因为 JSX 全在一行），列号指向参数末尾而不是 `root`，
极容易误判成"某个组件内部炸了"。

**修法**：`createRoot` 现在同时支持两种风格——`app` 可以自己
`appendChild(root, …)`（demo 的写法，返回 `void`），也可以直接 `return` 那棵树。
返回值走和 JSX children 完全相同的归一化（`appendChildren(root, [out])`），
所以 `El` / 字符串 / 数字 / 数组都行。签名是 `(root: El) => Child | void`。

> 顺带暴露的第二个约束：**`root` 只传给入口 `App`**。作为 JSX 子节点使用的组件
> 会经由 `callComponent` 收到 props 对象（没有 `id`），拿 `root` 的形参在这层
> 是 `{}`，`appendChild` 会生成 `parent: undefined` 的 op 被宿主丢弃。已写进包 README。

### 坑 #I：`<For items={…} render={…} />` 是错的

`For(items, render)` 是**两参数函数**，不是 props 组件。`h(For, {…})` 会走
`callComponent` → `For({items, render})`，于是 `items` 收到整个 props 对象、
`render` 为 `undefined`。正确写法是直接调用：`{For(() => rows, (r) => text(r))}`。
同理 `Show(cond, render)`。

## 交付物

| 路径 | 内容 |
| --- | --- |
| `packages/gpui-ts/package.json` | name/bin/exports/types/`files` 白名单；依赖仅 `esbuild ^0.25` |
| `packages/gpui-ts/bin/gpui-ts.mjs` | `build` / `run` / `doctor` / `--help` / `--version` |
| `packages/gpui-ts/lib/build.mjs` | `TAIL_MARKER`、`ESBUILD_OPTS`、`hostPathFor`、`bundleApp`、`packExe` |
| `packages/gpui-ts/index.ts` | 直接 `export *` 复用 `ui/src/{runtime,kit,canvas2d,theme}`（**不 fork**） |
| `packages/gpui-ts/tsconfig.types.json` | `rootDir=../../ui/src`、`emitDeclarationOnly`、`jsxFactory: "h"` |
| `packages/gpui-ts/README.md` | 用户视角：安装、CLI、API、样式子集、已知限制 |
| `packages/gpui-ts/.gitignore` | 忽略生成产物（`files` 白名单仍会打进 tarball） |
| `scripts/build-package.mjs` | 生成 `runtime/index.js` + `types/*.d.ts` + `vendor/win32-x64/` |

生成逻辑：esbuild 打 `index.ts`（`format: esm`，复用与 CLI 同一份 `ESBUILD_OPTS`）
→ 20 KB ESM；`tsc -p tsconfig.types.json` → 5 个 `.d.ts`；拷贝宿主 exe → vendor。
**删掉** `app.d.ts` / `main.d.ts` / `bom.d.ts`——demo 的内部声明不该是公开 API。

## 验收（全部通过）

| # | 项目 | 证据 |
| --- | --- | --- |
| 1 | `npm pack` 内容正确（11 文件 / 4.8 MB 压缩 / 含 12 MB 宿主） | `files` 白名单覆盖 `.gitignore`，已实测 |
| 2 | 仓库外隔离安装（`npm i ./gpui-ts-0.1.0.tgz`） | `/e/AI-workspace/gpui-ts-smoke`，独立 `node_modules` |
| 3 | `gpui-ts doctor` | `node v22.22.2 (win32-x64)` + host 11.5 MB + `shipped win32-x64` |
| 4 | `gpui-ts build src/main.tsx -o smoke.exe` | `smoke.exe (11.5 MB, self-contained)` |
| 5 | 运行：无异常、窗口标题生效 | `[qjs] app bundle evaluated` / `title":"gpui-ts smoke"` |
| 6 | 全组件渲染（TextField/Select/Switch/ScrollArea/`For`） | `build/smoke-user.png` |
| 7 | 计数器点击回传 | `build/smoke-counter-crop.png`：计数 0 → 2 |
| 8 | 下拉浮层展开 + 左对齐 + 盖住下方内容 | `build/smoke-select-crop.png` |
| 9 | 选中回填 + 跨信号联动 | `build/smoke-pick-crop.png`：`@ 深圳 · live` |
| 10 | 输入框真实编辑（带光标） | `build/smoke-type-crop.png`：`hello gpui-ts @ 深圳 · live` |
| 11 | 滚轮滚动 | `build/smoke-scroll-crop.png`：01 移出、06 露出 |
| 12 | 「返回 UI」入口风格 | 同上（`App` 用 `return`） |
| 13 | `appendChild` 入口风格未被破坏 | `build/smoke-append-crop.png`：计数 7 → 8 |
| 14 | `npm run typecheck` / `npx perry check src/main.ts` | 双双通过 |
| 15 | `node --test "host/test/*.test.mjs"` | 35 passed |
| 16 | `cargo test`（draw.rs） | 11 passed |

体积：单文件 exe **11.5 MB**（与库化前一致，尾部追加只多了 JS 本身）。

## 已知缺口

- 只发 `win32-x64` 宿主；其他平台 `doctor` 会明确报告缺什么，可从源码构建后放进
  `vendor/<platform>-<arch>/`；
- 包内后端固定 QuickJS，Perry 仍是仓库内对照路径（CLI 未暴露 `--backend`）；
- 无 HMR / devtools；调试靠 `PERRY_UI_TRACE=1` 与宿主日志；
- 应用代码跑在 QuickJS 上，Node 内置模块不可用（`process` 是宿主 shim）。
