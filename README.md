# gpui-perryts

用 **TypeScript（Solid 风格响应式）写界面，GPUI（Zed 同款 GPU 框架）做原生渲染** 的桌面端开发栈。

```
TypeScript/TSX 风格前端 ──esbuild──▶ 单个 IIFE JS ──内嵌──▶ QuickJS 引擎
        │                                                      │  JSONL mutation 协议 (stdio)
        ▼                                                      ▼
  Solid 风格 signals                          Rust host: retained tree ──▶ GPUI (GPU)
```

> 运行时层可插拔：**默认内嵌 QuickJS**（`rquickjs`，自拥 10ms 事件循环）；
> 历史 Perry 后端保留为对照模式（`--backend perry`）。协议与前端源码两者完全一致。

## 方案评估（为什么是这套架构）

| 路线 | 结论 |
|---|---|
| Perry 原生 UI（`perry/ui` → Win32 控件） | ❌ 没有 GPU 渲染，也不涉及 gpui |
| **内嵌 QuickJS 跑 TS bundle + gpui 渲染 host（stdio 协议）** | ✅ 当前默认：11.0MB 单文件、零子进程、click→派发 p50 8ms |
| Perry 编译 TS → 原生静态库 + gpui host | ✅ 已验证（16.4MB）；有 500ms I/O 量子、`/FORCE:MULTIPLE` 符号分家等怪病，已由 QuickJS 路线取代 |
| napi 进程内桥（gpuix 式） | ✅ 可行但引入 Node 运行时 + napi-rs 绑定开发量 |

**前端写法选型：SolidJS 风格细粒度响应式**（而非 React/Preact 的 VDOM reconciler）：

- 细粒度 signal 更新与 retained-tree mutation 是**一一映射**——不需要写 diff 算法，状态变化直接产生 `setText`/`setStyle` 最小操作；
- 无需 react-reconciler/scheduler 等 npm 依赖，运行时约 300 行纯 TS，完全落在 Perry 支持的 TS 子集内；
- React/Preact 路线依赖第三方 reconciler 在 Perry 下的兼容性（调度器、内部启发式），风险更高。

## 目录结构

```
host/             Rust gpui 渲染宿主
  src/main.rs       窗口、transport(JSONL)、样式映射、元素构建
  src/tree.rs       retained tree + mutation 应用
  src/quickjs.rs    内嵌 QuickJS 引擎（默认后端）：事件循环 + process shim
  src/embedded.rs   Perry staticlib 嵌入后端（对照模式）
  build.rs          模式选择（QUICKJS_EMBED / perry staticlib 自动检测）
ui/               TypeScript 前端
  src/runtime.ts    Solid 风格微运行时（signals + h + Show/For + stdio transport）
  src/app.ts        demo 应用（仪表盘）
  build-qjs.mjs     TS → 单 IIFE JS 打包（QuickJS 后端的编译步骤）
  build.mjs         node 开发模式打包（esbuild）
  build-perry.mjs   Perry 原生编译脚本（对照后端）
scripts/
  gpui-ts.mjs       一键 CLI：TS → 单文件 EXE
build/            产物（dist/*.exe / 截图 / 协议流样本）
```

## 运行

### 一键 CLI（gpui-ts：TS → 独立 EXE）

```bash
node scripts/gpui-ts.mjs                        # 默认 ui/src/main.ts → build/dist/main.exe
node scripts/gpui-ts.mjs app.ts -o out.exe
node scripts/gpui-ts.mjs --backend perry        # 切历史 Perry 后端（对照）
```

**默认（QuickJS）流程**：`esbuild` 把 TS 打包成单个 IIFE JS（`ui/dist/main.js`，
`charset=ascii` 保证全 ASCII 转义）→ `include_str!` 编译期内嵌进 host →
产出**单文件 EXE**，零 Node.js、零子进程、零 sidecar（拷到任意目录直接跑）。

**`--backend perry` 流程**：`perry compile --output-type staticlib`（size-optimized
stdlib）→ `cargo build --release` → 单文件 EXE（见下方 Perry 小节）。

### 双进程开发模式

```bash
# 1. host（Rust，首次构建较久）
cd host && cargo build
# 2. 前端：QuickJS 后端只需打包 JS（快，适合迭代）
cd ui && npm install && npm run build:qjs
#    Perry 后端：npm run build:perry（需 PATH 有 zstd，msys2 自带）
#    node 开发模式：node build.mjs
# 3. 启动（在仓库根目录；host 自动选择前端）
host/target/release/gpui-perryts-host.exe
```

前端解析顺序：内嵌 QuickJS（默认）→ `build/perry-app.exe`（Perry 原生）→
`ui/dist/main.js`（node-dev）。也可显式指定：`gpui-perryts-host.exe ui/dist/main.js`。

后端由 `host/build.rs` 决定：设 `QUICKJS_EMBED=1` 走内嵌 QuickJS（不链 perry 库）；
否则检测 `build/perry-app-static.lib` 决定是否启用 Perry 嵌入模式
（`PERRY_NO_EMBED=1` 强制回退双进程，`PERRY_STATIC_LIB_BASE` 可指定 app staticlib）。

## 内嵌 QuickJS 运行时层（默认后端）

对应愿景的「成熟后可平滑替换运行时层」：**协议、前端源码、`tree.rs` 解析器零改动**，
只换掉「谁的 JS 引擎在跑前端」，顺手甩掉 Perry 路线的一串怪病。

```
ui/src/*.ts ──esbuild(IIFE)──▶ ui/dist/main.js ──include_str!──▶ 编进 exe
                                                                     │
                        QuickJS 独立线程（10ms 自驱 tick） ──JSONL──▶ tree.rs ──▶ GPUI
```

- `process` 由宿主 shim 提供（QuickJS 无 `process` 全局）：`stdout/stderr.write`、
  `stdin.setEncoding/on`、`env`、`argv`、`exit`
- 计时器与 `console.*` 在 JS bootstrap 里定义，回调存 JS 全局 —— 避免 Rust 侧注册
  回调时捕获 `Ctx` 触发借用逃逸
- 线程模型：`Context` 是 `!Send`，引擎独享一个 OS 线程，宿主只通过 pipe + 共享队列通信

实测（详见 `docs/gpui-ts-plan.md`）：

| 项 | 实测 |
|---|---|
| 体积 | **11.01MB**（Perry 单 exe 16.45MB，**-33%**） |
| 冷启动 | 启动 → UI hello **254ms**（引擎 boot 345.6µs） |
| 交互延迟 | click → 前端派发 **p50 8ms** / avg 7.5ms / max 16ms（判据 ≤50ms） |
| 抗丢帧 | 8 连点 @161ms 间隔（<Perry 500ms 量子）**8/8 全中**，计数精确 |
| 零子进程 | `node.exe` 计数启动前后不变；`quickjs.rs` 零 `spawn` 调用 |
| 单文件自洽 | 拷到空目录、任意 cwd 运行：hello + 239-op mount + 窗口正常 |

集成时最隐蔽的坑：esbuild 的 `charset` 默认不转义 Latin-1 补充区码点（如标题里的
`×` U+00D7），会输出**单个 0xD7 字节**（非法 UTF-8），而 QuickJS 解析器严格 —— 报错
位置还会误导性地指向 `<input>:1:9`。必须设 `charset: "ascii"`。

遇到「bundle 在 node 里好好的，QuickJS 却报语法错」时，用这个诊断一步定位：

```bash
cd host && cargo run --release --example qjsprobe -- ../ui/dist/main.js
# [utf8] valid UTF-8 / INVALID at byte N ...   ← 先查 UTF-8
# [qjs] OK — parsed and evaluated / FAIL — <真实 JS 错误信息>
```



## Mutation 协议（v1）

前端 → host（stdout，JSONL）：

```jsonc
{"t":"hello","proto":1,"title":"App"}
{"t":"batch","ops":[
  {"op":"create","id":1,"tag":"div","text":"P"},
  {"op":"setStyle","id":1,"style":{"background":"#4f8cff","width":34}},
  {"op":"setEvents","id":1,"events":["click"]},
  {"op":"append","id":1,"parent":0},
  {"op":"setText","id":2,"text":"Count: 1"},
  {"op":"remove","id":3},
  {"op":"clear","id":4},
  {"op":"setTitle","title":"新标题"}
]}
```

host → 前端（stdin）：`{"t":"event","target":1,"kind":"click"}`

样式键：`flexDirection / gap / padding(+X/Y/L/R/T/B) / width / height / min* / max* /
grow / shrink / alignItems / justifyContent / overflow(scroll) / background / color /
fontSize / fontWeight / borderRadius / borderWidth / borderColor / opacity`。
颜色支持 `#rrggbb` 与 `#rrggbbaa`。复合键有固定应用顺序（`padding` 先于 `paddingX`）。

## Perry 后端（对照模式，附四个坑与绕过方案）

> 默认后端已换成内嵌 QuickJS；本节记录 Perry 路线（`--backend perry`）的配方与坑，
> 保留作为对照与回退路径。

perry 0.5.1520 在 Windows 上有四个坑，均已绕过（坑 1-2 的一键脚本 `ui/build-perry.mjs`）：

1. **RS4GC stackmap × 自带汇编器**：RS4GC 开启时生成的 `.s` 含
   `.seh_startepilogue/.seh_endepilogue` 指令，perry 自带汇编器不认识
   （与上游 #7354 RS4GC×WinEH funclet 冲突同源，hello-world 也复现）。
   → 设 `PERRY_RS4GC=0` 关闭 RS4GC，codegen 3/3 全部通过
   （本项目代码无 try/catch、无 for...of、无 Proxy，`perry check` 全绿）。
2. **`perry_runtime.lib` "缺失"**：链接期报找不到，实际库就在包里，只是以
   zstd 压缩发布（`@perryts/perry-win32-x64/lib/*.lib.zst`）。
   → 解压后设 `PERRY_RUNTIME_DIR` 指向该目录。
3. **嵌套调用参数绑定异常**：`appendChild(e, create("text", String(c)))` 这类
   实参位置多层调用，perry 原生二进制里外层拿到错误值（实测 `append` op 的
   `id` 变 undefined，node 同代码正常；两层调用 `f(g(x))` 无此问题，最小复现
   `build/nest-test.ts`）。→ 一律先绑定局部变量再传参。host 收到缺 `id` 的
   append 会静默丢弃该 op，表现为文本节点不挂树、UI 缺块（曾导致 perry 窗口
   侧边栏导航、+/- 按钮文字全部消失，与 node 模式 UI 不一致）。
4. **事件循环空闲时 ~500ms 量子轮询 stdio**：JS 侧 `process.stdin` 事件派发
   聚簇在 ~500ms 边界（实测送达延迟 avg 255ms / p50 312ms / max 504ms，
   node 为 0.1ms），点击回传明显"丢帧"。→ runtime 里加 10ms 心跳定时器
   强制事件循环高频运转，实测降至 avg 24.7ms / p50 17ms / max 94ms
   （`build/feeder.mjs` + `build/analyze-trace.mjs` 可复现测量；
   `PERRY_UI_TRACE=1` 输出逐段延迟到 stderr）。

验证方式：分别捕获 node 与 perry 前端的 stdout ops 流，时钟值归一化后
`diff` 完全一致（`build/stream-node.txt` / `build/stream-perry.txt`）。

> 注：`PERRY_RS4GC=0` 是绕过而非修复；perry 上游修复 `.seh` 汇编 bug 后可去掉该
> 环境变量，恢复默认 GC 策略。

## 复用的已验证组件

- `gpui-pre 0.3.3` / `gpui-pre-platform 0.3.3`：Zed gpui 的 crates.io 快照（zed@5b055fa）。
  （2026-09-22 起不再使用 `gpui-pre-windows` 本地补丁——其唯一作用是去掉 `u_strlen`
  对系统 icuuc.dll 的依赖（Win10 1803 以下兼容），对体积零影响；如需兼容老系统，
  从 rebased-rs 项目恢复补丁目录并还原 host/Cargo.toml 的 `[patch.crates-io]` 条目）；
- MSVC 工具链环境变量与构建参数沿用本机 rebased-rs 项目验证过的配置。

## 后续路线

- TextInput 协议扩展（gpui 侧 text input 元素 + 键盘事件回传）
- 滚动/悬停事件、图片节点、动画（gpui animate）
- 运行时层已用内嵌 QuickJS 替换 Perry（体积 -33%、自拥 10ms 事件循环）；后续可考虑
  评估 **PrimJS** 等更小的引擎，或按需切换 ES 版本/预编译字节码（QuickJS bytecode）
  省掉启动期 parse
- Perry 后端仅为对照保留；上游修复 `.seh` 汇编 bug 后可去掉 `PERRY_RS4GC=0`

## 体积优化：size-optimized stdlib（17.1MB → 5.8MB，-66%）

`perry compile` 默认链接预编译的**全量** stdlib（sqlite/crypto/tokio/http 全在里面，
linker dead-strip 掉不进 exe，但 .lib 解析和最后链接的 object 全量参与，exe 17.1MB）。

perry 内置 auto-optimize 机制（`--output-type` 任意）：设置 `PERRY_WORKSPACE_ROOT`
指向 perry 源码 checkout 后，编译器按应用实际 imports 计算所需 cargo features，
用 `--no-default-features --features <所需>` 重编 runtime+stdlib，panic=abort 自动生效。
本应用（process.stdin/stdout + setInterval）只需 `features=async-runtime` 一个。

```bash
# 一次性准备：checkout 与 npx perry 版本一致的源码（必须，否则 undefined symbol）
git clone --depth 1 https://github.com/PerryTS/perry.git E:/AI-workspace/perry-src
cd E:/AI-workspace/perry-src
git fetch --depth 1 origin tag v0.5.1520 && git checkout v0.5.1520   # 与 perry 0.5.1520 对齐

# 编译（build-perry.mjs 已内置；首次约 7 分钟 cargo 重编，之后 hash 缓存秒用）
npm run build:perry            # PERRY_NO_SIZE_OPT=1 可跳过
```

实测：`Binary size: 5.8MB`（PERRY_SIZE_OPT=z）。重编产物在
`perry-src/target/perry-auto-<hash>/x86_64-pc-windows-msvc/release/`。
可选：`PERRY_SIZE_LTO=fat` fat LTO 再压一档（编译更慢）。

## 单一 EXE：staticlib 嵌入 host（Perry 后端，已验证）

> 这是 Perry 后端的单 exe 方案；默认后端（内嵌 QuickJS）不涉及本节内容。

两个进程可以合一个：perry 支持 `--output-type staticlib`（上游 #1088），
TS 前端编译成静态库，链入 gpui host。

- `npx perry compile src/main.ts --output-type staticlib -o ../build/perry-app-static.lib`
  → **947KB** 纯 app 对象；入口 `void perry_module_init()`（无 main，无内嵌事件循环）
- host 侧调用序列（已验证，见 `build/embed-proto/`）：
  1. 建 3 组匿名 pipe（in/out/err），在**任何 std 使用前** `SetStdHandle` 重定向
  2. `perry_module_init()` —— 100ms 返回，顶层语句跑完，init ops（JSONL）写进 out pipe
     *注意：init 前先往 stdin pipe 预写一行，否则 perry stdin 子系统同步读空 pipe 阻塞*
  3. 循环：喂 event 行 → `perry_poll()`（排空 microtasks+stdlib pump）；
     `perry_has_work()` / `perry_next_wake_ms()` 驱动调度；`perry_set_wake_callback()` 可免轮询
- 链接：`perry_app_static.lib + perry_runtime.lib + perry_stdlib.lib` + windows_link.rs
  同款系统库清单 + `/FORCE:MULTIPLE`（perry_runtime 内嵌一份 Rust std，与宿主 std
  在 `rust_eh_personality` 等符号上重复定义，功能等价取其一）
- stdio 即协议：`console.log`/`process.stdin` 全走进程 std handles，pipe 重定向后
  JSONL 协议**零改动**，gpui host 把「子进程 stdio」换成「进程内 pipe」即可

原型验证结果（build/embed-proto/run.log）：init ops 到达、event 消费
（microtasks=1）、心跳 timer 在 `next_wake_ms≈9` 可见、perry_has_work() 持续为真。
