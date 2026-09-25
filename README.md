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

## 前端写法：TSX

前端源码是 **TSX**（`.tsx`）。JSX 在构建期由 esbuild 以 **classic transform**
编译成运行时导出的 `h()` 工厂调用 —— JSX 纯粹是语法糖，运行时契约、mutation
协议、host 侧解析全部不变：

```tsx
<div style={{ padding: 16, background: C.card }}>hi {name}</div>
// ⟺  h("div", { style: { padding: 16, background: C.card } }, "hi ", name)
```

约定：

| 写法 | 含义 |
|---|---|
| `style={{ padding: 16, gap: 10 }}` | **样式的唯一通道**：对象原样进 host 的 setStyle op。没有平铺写法 —— 样式集中在一处，组件的自有 props 不会和样式键撞名 |
| `onClick={fn}` | 事件绑定（协议层目前只有 click） |
| `<Card title="…">…</Card>` | 大写标签＝组件：`h(Card, props, …)`，children 以 `props.children` 传入；组件拿到 props 原样自处（含 `style`） |
| `<></>` | Fragment：host 没有 fragment 概念，落成一个透明 flex-column 容器 |
| `{items.map(…)}` | 数组 children 会被展平（`h` 内的 `appendChildren`） |

内在标签（小写）只认 `style` / `onClick` / `text` 三个 prop，其余一律忽略。
`globals.d.ts` 里的 `JSX.IntrinsicProps` 没有索引签名，所以把样式键写到顶层
（`<div padding={16}>`）在编辑器里是**类型错误**而不是静默失效。

每个 `.tsx` 必须 `import { h } from "./runtime"`（用 `<></>` 时再加 `Fragment`）——
classic transform 直接引用这个标识符，不会自动注入。

**JSX 在各后端如何落地**：

| 后端 | 处理 |
|---|---|
| quickjs（默认）/ node-dev | esbuild 直接 bundle，`jsx: "transform"` + `jsxFactory: "h"` |
| perry（对照） | perry 解析器**没有 JSX 分支**（`perry check x.tsx` 直接报 *No TypeScript files found*），故先由 esbuild 预转成 `ui/build/gen/*.ts`（不 bundle、保留 ESM 结构与 import 路径）再交给 perry——见 `ui/pretranspile-tsx.mjs` |

`ui/tsconfig.json` + `ui/src/globals.d.ts` 只服务编辑器/`tsc`：前者声明
`jsx: "react"`（classic）+ `jsxFactory: h`，后者补上宿主注入的 `process` shim 与
全局 `JSX` 命名空间。两者都不参与构建。

## 目录结构

```
host/             Rust gpui 渲染宿主
  src/main.rs       窗口、transport(JSONL)、样式映射、元素构建、输入/滚动、绘制
  src/tree.rs       retained tree + mutation 应用
  src/draw.rs       canvas 显示列表词表（5 原语 + 颜色 + 滚动条几何，可 cargo test）
  src/quickjs.rs    内嵌 QuickJS 引擎（默认后端）：事件循环 + process shim
  src/bootstrap.js  注入前端的 JS 环境（BOM 本体，~560 行纯 JS）
  src/bom.rs        宿主侧的 BOM 事实：窗口度量/单调时钟/熵/弹窗应答
  src/embedded.rs   Perry staticlib 嵌入后端（对照模式）
  test/             bootstrap 的 node 单元测试（脱离 GUI 验 BOM 语义）
  build.rs          模式选择（QUICKJS_EMBED / perry staticlib 自动检测）
ui/               TSX 前端
  src/runtime.ts    Solid 风格微运行时（signals + h/Fragment + Show/For + transport）
  src/kit.tsx       组件库：TextField / Radio(Group) / Checkbox / Switch / Select /
                    ScrollArea / CanvasView
  src/canvas2d.ts   Canvas 2D 录制器（兼容 ctx 形状，录成显示列表）
  src/theme.ts      共用色板
  src/app.tsx       demo 应用（仪表盘，JSX 编写）
  src/bom.ts        BOM 访问门面（带 hasBom 探测，Perry 后端自动降级）
  src/globals.d.ts  手写环境声明（BOM + process shim + JSX 命名空间）
  tsconfig.json     JSX classic 变换配置（jsxFactory=h）；lib 只留 ES2020，故意不含 DOM
  build-qjs.mjs     TSX → 单 IIFE JS 打包（QuickJS 后端的编译步骤）
  build.mjs         node 开发模式打包（esbuild）
  build-perry.mjs   Perry 原生编译脚本（对照后端，含 TSX 预转）
  pretranspile-tsx.mjs  TSX → 中间 TS（perry 解析器无 JSX 分支）
scripts/
  gpui-ts.mjs       一键 CLI：TSX → 单文件 EXE（仓库内用）
  build-package.mjs 生成 npm 包的 runtime/types/vendor
  pack.mjs          esbuild + 追加到 host 副本（npm 包打包核心的仓库内单文件版）
packages/
  gpui-ts/          可发布的 npm 包（见「发布为 npm 库」）
    bin/ lib/        CLI 与打包核心
    runtime/ types/ 生成产物：ESM 运行时 + .d.ts
    vendor/win32-x64/gpui-ts-host.exe  预编译宿主（用户不需要编译环境）
build/            产物（dist/*.exe / 截图 / 协议流样本）
  winrect.py        查 GPUI 窗口客户区几何（验收坐标别靠肉眼量）
  shot.py           单进程「抓屏」（看新渲染长什么样）
  click_shot.py     单进程「点击 → 延时 → 抓屏」（抓 1s 级动画中段）
  type_shot.py      单进程「点击 → 键入 → 抓屏」（验输入框编辑，`\n` 表回车）
  wheel_shot.py     单进程「滚轮 → 抓屏」，用标准 ±120 delta
  scan-row.py       逐行采样像素（定位控件边界，别靠肉眼量）
  capture-ops.py    抓单进程前端真实 ops 流（供树重建/diff）
```

> 抓滚轮不能用 `pyautogui.scroll()`：它在 Windows 上发的是裸 delta ±1，而 GPUI 按
> `delta/120 × 系统行数` 换算，于是每格只滚 0.025px，看起来像"滚轮没反应"。
> `wheel_shot.py` 直接发标准 120 的倍数。

## 运行

### 一键 CLI（gpui-ts：TSX → 独立 EXE）

```bash
node scripts/gpui-ts.mjs                        # 默认 ui/src/main.ts → build/dist/main.exe
node scripts/gpui-ts.mjs ui/src/main.ts -o out.exe
node scripts/gpui-ts.mjs --backend perry        # 切历史 Perry 后端（对照）
```

**默认（QuickJS）流程**：`esbuild` 把 TSX 打包成单个 IIFE JS（`ui/dist/main.js`，
JSX → `h()`、`charset=ascii` 保证全 ASCII 转义）→ `include_str!` 编译期内嵌进 host →
产出**单文件 EXE**，零 Node.js、零子进程、零 sidecar（拷到任意目录直接跑）。

**`--backend perry` 流程**：esbuild 先把 TSX 预转成 `ui/build/gen/*.ts`（perry 解析器
没有 JSX 分支）→ `perry compile --output-type staticlib`（size-optimized stdlib）→
`cargo build --release` → 单文件 EXE（见下方 Perry 小节）。

产物运行时可切传输（**同一个 exe 两种都支持**，见「内嵌 QuickJS 运行时层」）：

```bash
build/dist/main.exe                      # direct（默认）：宿主注入通信函数
build/dist/main.exe --transport=pipe     # pipe：stdio 管道（历史基线 / 排障）
GPUI_TS_TRANSPORT=pipe build/dist/main.exe   # 等价的环境变量写法
PERRY_UI_TRACE=1 build/dist/main.exe     # 打开前端 trace（写 stderr，不进协议流）
```

### 双进程开发模式

```bash
# 1. host（Rust，首次构建较久）
cd host && cargo build
# 2. 前端：QuickJS 后端只需打包 JS（快，适合迭代）
cd ui && npm install && npm run build:qjs
#    Perry 后端：npm run build:perry（需 PATH 有 zstd，msys2 自带）
#    node 开发模式：node build.mjs
#    TSX 类型检查：npm run typecheck（编辑器之外的同一套 JSX 属性约束）
# 3. 启动（在仓库根目录；host 自动选择前端）
host/target/release/gpui-perryts-host.exe
```

前端不依赖 GUI 的两项自检（改 BOM / 改类型后先跑这两个，比开窗口快得多）：

```bash
cd ui && npm run typecheck                  # 手写环境声明 + JSX 属性约束
node --test "host/test/*.test.mjs"          # BOM 语义（35 例，需 glob 不可传裸目录）
```

前端解析顺序：内嵌 QuickJS（默认）→ `build/perry-app.exe`（Perry 原生）→
`ui/dist/main.js`（node-dev）。也可显式指定：`gpui-perryts-host.exe ui/dist/main.js`。

后端由 `host/build.rs` 决定：设 `QUICKJS_EMBED=1` 走内嵌 QuickJS（不链 perry 库）；
否则按 `PERRY_STATIC_LIB_BASE` 解析 app archive（`scripts/gpui-ts.mjs --backend perry`
会传 `app-main`；未指定时取 `build/` 下 `app-main.lib` / `perry-app-static.lib`
里**较新**的一份，并把选择结果打成 cargo warning）来决定是否启用 Perry 嵌入模式。
`PERRY_NO_EMBED=1` 强制回退双进程。

> 别让这里的名字漂移：archive 名对不上时链接**照样成功**，只是跑上一版冻结的
> 源码（`/FORCE:MULTIPLE` 把错误完全吞掉）。`scripts/gpui-ts.mjs` 现在会在链接后
> 校验 exe 里含当前前端的字符串，见 `docs/gpui-ts-plan.md` 坑 #7。

## 发布为 npm 库：`gpui-ts`

仓库里的 host/前端能自举之后，它可以被打成一个**给别人用的 npm 包**：
使用者只要有 Node ≥ 18，**不需要 Rust、不需要 MSVC、不需要本仓库**。

```bash
npm i gpui-ts
npx gpui-ts build src/app.tsx -o myapp.exe     # 11.5 MB，单文件自包含
npx gpui-ts run   src/app.tsx                  # 打包到临时文件并直接运行
npx gpui-ts doctor                             # 报告本机这套安装能不能用
```

包内容（`packages/gpui-ts/`，11 个文件 / 压缩后 4.8 MB）：

| 目录 | 内容 |
| --- | --- |
| `bin/gpui-ts.mjs` | CLI：`build` / `run` / `doctor` |
| `lib/build.mjs` | 打包核心（esbuild 选项、`TAIL_MARKER`、`packExe`） |
| `runtime/index.js` | 运行时 + 组件库 + canvas2d + theme 的 ESM 打包（20 KB） |
| `types/*.d.ts` | 5 个声明文件，编辑器补全/跳转/签名提示 |
| `vendor/win32-x64/` | 预编译宿主二进制（11.5 MB） |

### 为什么用户不用装编译环境

宿主二进制**随包发布**，应用以「script 数据」的形式**追加在宿主副本尾部**：

```
app.tsx ──esbuild(IIFE)──► JS ──追加到宿主副本尾部──► myapp.exe
                                                          │
              宿主启动时从自己的文件里读回那段 JS ◄────────┘
```

Windows 的 PE 加载器会忽略 exe 末尾的多余字节，所以产物就是一个普通可执行文件，
只是顺便带着一段脚本；宿主按 `TAIL_MARKER`（`\n<<<GPUI_TS_BUNDLE_v1>>>\n`）切出来
交给内嵌 QuickJS。读取顺序是**尾部追加 → `QUICKJS_BUNDLE` env → 编译期内嵌的
demo**，所以同一个 host 既能跑发布出去的 app，也能跑仓库自带的示例。
宿主侧的实现在 `host/src/quickjs.rs::bundle_from_self`。

> marker 是跨构件契约：二进制和 JS 是分开发布的，`lib/build.mjs` 里的
> `TAIL_MARKER` 必须和 `quickjs.rs` 里的常量逐字一致。打包时会拒绝含有该 marker
> 的 bundle（否则宿主会在错误的偏移处切分，报一个看不懂的语法错）。

### 仓库内的构建与验证

```bash
node scripts/build-package.mjs     # 生成 runtime/ + types/ + vendor/（就地写入包目录）
cd packages/gpui-ts && npm pack    # 检查 tarball 内容（files 白名单会覆盖 .gitignore）
```

`runtime/` `types/` `vendor/` 都是**生成产物**，已在 `packages/gpui-ts/.gitignore` 里
忽略；但因为 `package.json` 的 `files` 白名单，`npm pack` 仍会打进去。

端到端验收（**在仓库之外**做，确保没有隐性依赖）：

```bash
mkdir /tmp/smoke && cd /tmp/smoke && npm init -y
npm i /path/to/gpui-ts-0.1.0.tgz
node node_modules/gpui-ts/bin/gpui-ts.mjs build src/main.tsx -o smoke.exe && ./smoke.exe
```

已验证：计数器点击回传（0→2）、下拉浮层展开/选中/回填、输入框编辑、
滚轮滚动、`For` 列表、`appendChild` 与「返回 UI」两种入口风格，两种都在
**隔离安装**下跑通（截图见 `build/smoke-*.png`）。

> 踩过的坑：`createRoot` 传给 `App` 的 `root` 是**元素句柄**（`{id:0}`），
> 不是函数——`root(<div/>)` 会报 `TypeError: not a function`。正确写法是
> `appendChild(root, <div/>)`，或者直接 `return` 那棵树。2026-09-23 起
> `App` **两种都支持**（返回值走和 JSX children 同一套归一化）。
> 另外 `root` 只传给**入口** `App`；作为 JSX 子节点调用的组件必须返回元素。

## 内嵌 QuickJS 运行时层（默认后端）

对应愿景的「成熟后可平滑替换运行时层」：**协议、前端源码、`tree.rs` 解析器零改动**，
只换掉「谁的 JS 引擎在跑前端」，顺手甩掉 Perry 路线的一串怪病。

```
ui/src/*.tsx ──esbuild(IIFE, jsx→h, charset=ascii)──▶ ui/dist/main.js ──include_str!──▶ 编进 exe
                                                                                    │
                    QuickJS 独立线程（自驱 tick：microtask → timers → 事件派发） ◀────┘
                                    │   ▲
                      ops 批 (__hostEmit)│   │ 事件 (EventSink::push + unpark)
                                    ▼   │
                              tree.rs 保留树 ──▶ GPUI
```

- `process` 由宿主 shim 提供（QuickJS 无 `process` 全局）：`stdout/stderr.write`、
  `stdin.setEncoding/on`、`env`、`argv`、`exit`
- 计时器与 `console.*` 在 JS bootstrap 里定义，回调存 JS 全局 —— 避免 Rust 侧注册
  回调时捕获 `Ctx` 触发借用逃逸
- 线程模型：`Context` 是 `!Send`，引擎独享一个 OS 线程；宿主只经共享队列 + 注入的
  宿主函数与它通信

### 两条传输，同一个协议（运行时切换，**不需要重新编译**）

引擎既然在**进程内**，就没必要假装有一条 stdio 管道。于是协议与载体解耦：

| | `direct`（默认） | `pipe`（历史基线） |
|---|---|---|
| 出站 ops | 注入宿主函数 `__hostEmit(line)` → 直接进 ops 通道 | `process.stdout.write` → 管道 → 读线程逐行解析 |
| 入站事件 | `EventSink::push` 直灌队列 + `unpark` 引擎线程 | 写线程 → 管道 → 读线程 → 队列（引擎下个 tick 才取） |
| 额外线程 | 0 | 2（stdin reader + stderr tee） |
| 额外系统调用 | 0 | 写+刷管道、阻塞读、行切分 |
| 引擎 boot | **130µs** | 1.27ms |
| 事件送达 | **avg 0.10ms / max 1ms** | avg 0.10ms，但偶发 191ms 停顿 |
| 端到端派发 | **≤测量分辨率（12/20 样本为负，即 <±1ms 时钟偏差）** | p50 6ms，**明显 10ms 量化簇** + 偶发 200ms+ |

延迟差异的来源是**唤醒方式**：`pipe` 只能等引擎下一个 10ms tick 去取队列（所以出现
9/10/10/10ms 的量化簇）；`direct` 用 `unpark` 就地唤醒，事件一落地就派发。两侧都以
同一个 bundle 跑（前端用 `typeof __hostEmit === "function"` 探测载体），所以切换只是
一个启动参数：

```bash
build/dist/main.exe                          # direct（默认）
build/dist/main.exe --transport=pipe         # 或 GPUI_TS_TRANSPORT=pipe
```

实测（详见 `docs/gpui-ts-plan.md`）：

| 项 | 实测 |
|---|---|
| 体积 | **11.03MB**（Perry 单 exe 16.45MB，**-33%**） |
| 冷启动 | 启动 → UI hello **193~235ms**（5 次；引擎 boot 130~257µs） |
| 交互延迟 | click → 前端派发 **≤测量分辨率**（direct）／ p50 6ms + 10ms 量化簇（pipe）（判据 ≤50ms） |
| 抗丢帧 | 30 连点 @20ms 间隔 **30/30 全中**，计数精确等于点击数，零协议告警 |
| 零子进程 | `node.exe` 计数启动前后不变；`quickjs.rs` 零 `spawn` 调用 |
| 线程数 | direct 38 / pipe 41（全进程，含 GPUI 线程池；本栈差额 = 预测的 2 条） |
| 单文件自洽 | 拷到空目录、任意 cwd 运行：hello + 359-op mount + 窗口正常 |

集成时最隐蔽的坑：esbuild 的 `charset` 默认不转义 Latin-1 补充区码点（如标题里的
`×` U+00D7），会输出**单个 0xD7 字节**（非法 UTF-8），而 QuickJS 解析器严格 —— 报错
位置还会误导性地指向 `<input>:1:9`。必须设 `charset: "ascii"`。

遇到「bundle 在 node 里好好的，QuickJS 却报语法错」时，用这个诊断一步定位：

```bash
cd host && cargo run --release --example qjsprobe -- ../ui/dist/main.js
# [utf8] valid UTF-8 / INVALID at byte N ...   ← 先查 UTF-8
# [qjs] OK — parsed and evaluated / FAIL — <真实 JS 错误信息>
```

### 宿主排障开关

协议是单一事实来源，所以「前端发了什么」和「宿主收到什么」必须能分别观测：

```bash
GPUI_TS_LOG_OPS=1  ./build/dist/main.exe   # 每批 op 数 + 丢弃数 + 累计（stderr）
GPUI_TS_DUMP_TREE=1 ./build/dist/main.exe  # 大批次后 dump 保留树（≥32 op 才打）
```

**一个数字就够定位整类问题**：`batch ops=359 dropped=0`。
同一份前端在三种拓扑下 mount 批次必须都是 **359**（QuickJS / Perry staticlib /
Perry 双进程）。三者出现分歧时，先怀疑构建产物而不是代码逻辑——`--backend perry`
现在会在链接后校验 exe 内容（见坑 #7）。历史上正是"恒为 239"这条线索暴露了链错
archive：四次改动、四个相同的数字，说明跑的从来不是改后的源码。

前端侧对应物：卡片上的 `累计 mutation` / `mutation 批`（前端自报发送量）。两者应当
一致；`dropped > 0` 才是宿主丢 op，否则别去改 `tree.rs`。

### BOM：宿主注入的浏览器环境

前端要能像写 Web 代码那样写 TSX，就得有 `setTimeout`、`requestAnimationFrame`、
`window.innerWidth` 这些东西。它们在 QuickJS 里**一个都没有**，所以由宿主补齐。

关键定位：这是一个**JS 运行环境**，不是 DOM 模拟。全部 API 挂在同一个全局上
（`window === self === globalThis`，和 Web Worker 同一心智模型），**没有 DOM 树**，
`document` 是**故意缺席**的——渲染入口只有 `h()` + mutation 协议这一条路，
不给自己留第二条。

| 区块 | 提供的东西 |
|---|---|
| 调度 | `setTimeout`/`setInterval`/`clearTimeout`/`clearInterval`、`queueMicrotask`、`requestAnimationFrame`/`cancelAnimationFrame`、`requestIdleCallback` |
| 窗口 | `innerWidth`/`innerHeight`/`outerWidth`/`outerHeight`/`devicePixelRatio`（实时 getter）、`resize` 事件 |
| 环境 | `screen`、`location`（`nativets://app/index.tsx`）、`navigator.userAgent`、`performance.now()` |
| 事件 | `Event`/`CustomEvent`、`addEventListener`/`removeEventListener`/`dispatchEvent` |
| 工具 | `crypto.getRandomValues`/`randomUUID()`（v4）、`structuredClone`（环安全） |
| 弹窗 | `alert`/`confirm`（宿主绘制的 GPUI 模态）、`prompt`（显式未实现） |
| 兼容 | `process` shim（QuickJS 无此全局）、6 级 `console.*`、`console.assert` |

落地在四处，各管一摊：

| 文件 | 职责 |
|---|---|
| `host/src/bootstrap.js` | JS 环境本体（~560 行），`include_str!` 进 exe；纯 JS，可脱离 GUI 测试 |
| `host/src/bom.rs` | 只有 JS 拿不到的 4 件事：窗口度量（原子共享）、单调时钟、熵、弹窗应答 |
| `ui/src/bom.ts` | 前端唯一访问口，带 `hasBom` 探测；Perry 后端自动降级 |
| `ui/src/globals.d.ts` | 手写声明，与 `bootstrap.js` 严格对齐 |

三个刻意的设计选择：

- **`requestAnimationFrame` 由引擎 tick 驱动，不是定时器**。空闲应用不产生任何帧预算，
  引擎该 park 就 park；默认 20ms 节奏 = 稳定 50Hz。
- **`confirm` 真的阻塞 JS 线程**（浏览器语义），走独立 `mpsc` 通道——结果**不能**回引擎
  队列，因为那个线程正卡在那里等。实测：弹窗期间 1 秒定时器一格不走，应答后立刻恢复。
  窗口已销毁时返回默认值，绝不永久阻塞。
- **弹窗由宿主绘制，不走协议**。模态是宿主自己的事（像系统消息框），协议里没有它。

`tsconfig.json` 只留 `"lib": ["ES2020"]`，**故意不引 `lib.dom`** —— 那会把 `document`、
`localStorage`、`fetch` 都声明成「可用」，而宿主根本没提供。BOM 由 `globals.d.ts`
按实际能力手写，所以用错东西是**编译错误而不是运行时 `undefined is not a function`**。

BOM 的语义可以完全脱离 GUI 验证（35 个用例，覆盖定时器/帧节奏/环克隆/熵/弹窗）：

```bash
node --test "host/test/*.test.mjs"
```

> 注意：必须传 glob。这个 node 会把裸目录当模块解析，`node --test host/test` 直接
> `MODULE_NOT_FOUND`。

绘制词表（颜色解析、显示列表、滚动条几何）同样不需要窗口：

```bash
cd host && cargo test --release      # 11 个用例，draw.rs
```



## Mutation 协议（v1 + 表单/绘制扩展）

协议本身与载体无关：同一份 JSONL 既走 stdio 管道（`pipe`），也走宿主注入的函数
（`direct`，ops 直进通道 / 事件直灌队列）。宿主侧两条路径共用同一个
`main.rs::ingest_line` 解析入口，因此两种载体不可能漂移出两套方言。

前端 → host（JSONL）：

```jsonc
{"t":"hello","proto":1,"title":"App"}
{"t":"batch","ops":[
  {"op":"create","id":1,"tag":"div","text":"P"},
  {"op":"create","id":5,"tag":"input","placeholder":"输入后回车…"},
  {"op":"create","id":6,"tag":"canvas"},
  {"op":"setStyle","id":1,"style":{"background":"#4f8cff","width":34}},
  {"op":"setEvents","id":5,"events":["input","change","focus","blur"]},
  {"op":"setValue","id":5,"value":"hello"},           // 字段值（≠ setText 的内容语义）
  {"op":"setCanvas","id":6,"cmds":[{"k":"rect",/*…*/}]}, // canvas 显示列表
  {"op":"append","id":1,"parent":0},
  {"op":"setText","id":2,"text":"Count: 1"},
  {"op":"remove","id":3},
  {"op":"clear","id":4},
  {"op":"setTitle","title":"新标题"}
]}
```

host → 前端（JSONL）：`{"t":"event","target":1,"kind":"click","value":"…"}`

| kind | 载荷 | 说明 |
| --- | --- | --- |
| `click` / `focus` / `blur` | — | focus/blur 由宿主每帧轮询焦点句柄发出（GPUI 无对应回调） |
| `input` | `value` | 每次真实编辑后的**完整值**（值未变的 Enter 不发，否则前端会当成多敲了一键） |
| `change` | `value` | 回车提交 |
| `scroll` | `top` / `max` / `viewport` / `content` | 均 px；`top` 是**已滚距离**，与 `max` 同坐标系，前端直接 `top / max` 画进度 |

`setEvents` 是前端在声明「我关心哪些事件」——宿主只回传被声明的种类，其余静默丢弃。

**v1 只增不改**：`setValue` / `setCanvas` 与 `setText` / `setStyle` 并列，
`create` 多一个可选 `placeholder`；只发旧 op 的前端行为完全不变。

样式键：`flexDirection / gap / padding(+X/Y/L/R/T/B) / width / height / min* / max* /
grow / shrink / alignItems / justifyContent / alignSelf / overflow(scroll|hidden) /
background / color / fontSize / fontWeight / borderRadius / borderWidth / borderColor /
opacity / position(absolute|relative|fixed) / top / left / right / bottom / cursor`。
颜色支持 `#rgb` / `#rrggbb` / `#rrggbbaa` / `rgb()` / `rgba()`。复合键有固定应用顺序
（`padding` 先于 `paddingX`）。

两个键的语义值得单独写下来，它们都是被滚动/浮层逼出来的：

- **`grow: n` 等价 CSS `flex: n 1 0%`**，而不是只设 `flex-grow: n`：宿主同时设
  `flex-basis: 0` 和 `min-width/height: 0`。差别在滚动容器上是决定性的——只设
  `flex-grow` 时子项以**内容高度**为基准，会把父级撑开而不是产生"可滚动区域"；
  而 GPUI 的 flex 子项最小尺寸默认是 0（CSS 是 `min-height: auto`），少了
  `min-*: 0` 就完全不能收缩。
- **`elevate: n`（n > 0）不是样式而是绘制顺序**：宿主把节点包进
  `deferred(child).with_priority(n)`，等价于 z-index。下拉面板能盖住后面的兄弟节点
  全靠它——没有它，面板会被后画的卡片盖在身上。

## Perry 后端（对照模式，附八个坑与绕过方案）

> 默认后端已换成内嵌 QuickJS；本节记录 Perry 路线（`--backend perry`）的配方与坑，
> 保留作为对照与回退路径。

perry 0.5.1520 在 Windows 上有八个坑，均已绕过（坑 1-2 的一键脚本 `ui/build-perry.mjs`）：

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
5. **perry-src 工作树不干净 → auto-opt 拒绝打包 runtime** 并警告
   "archive may be stale"。此时 `/FORCE:MULTIPLE` 会让 host 链到新旧两份实例，
   符号注册/读取分家（如 `STDLIB_PUMP_FN` 永远为 null），症状是"部分子系统静默失效"。
   → 给 perry-src 打临时补丁必须 `git checkout` 回滚后再出正式产物。
6. **perry 完全不解析 JSX**：`perry check x.tsx` 直接答 "No TypeScript files found"，
   parser 里根本没有 JSX 分支。→ 所有源文件先过 esbuild 预转成 `.ts`
   （`ui/pretranspile-tsx.mjs`），perry 只见 `.ts`。
7. **链错 app archive：链接成功、内容陈旧**（最接近"假成功"的一个）。app archive 的
   基名有两个来源——`scripts/gpui-ts.mjs` 用入口名推出 `app-main.lib`，
   而 `ui/build-perry.mjs --staticlib` 写 `perry-app-static.lib`。上一版 `build.rs`
   把这名字**写死**成后者，于是 CLI 每轮新编的 archive 被完全忽略：宿主链的是上一次
   遗留的那份，且 `/FORCE:MULTIPLE` 让错误彻底无声——exe 正常生成、应用正常运行，
   只是**静止在旧版源码**上（表现为"改了代码界面一点没变"、BOM 卡片整块消失、
   mount ops 恒为 239 而不是 359）。
   → `build.rs` 按 `PERRY_STATIC_LIB_BASE` 解析（未指定时取候选里较新的一份，
   并打出 cargo warning）；`rerun-if-changed` 也跟着指向真实文件，否则 archive 更新
   也不会触发重链；`scripts/gpui-ts.mjs` 链接后校验 exe 含当前前端的字符串
   （`PERRY_SKIP_EMBED_VERIFY=1` 可跳过）。
8. **跨模块 import 的 `hasBom` 在 staticlib codegen 下不可靠**：`bom.ts` 里
   `export const hasBom = typeof window !== "undefined"`，在 `app.tsx` 里写
   `hasBom ? A : B` 会读成 **true**（渲染期），而同一次运行中 `bom.ts` 内部以及
   点击期的读取都正确返回 false。→ **分支一律留在 `bom.ts` 内**，对外只暴露
   格式化结果（如 `uptimeLabel()`）或返回值（如 `raf(): boolean`），
   app 代码不引用这个布尔值。

验证方式：分别捕获 node 与 perry 前端的 stdout ops 流，时钟值归一化后
`diff` 完全一致（`build/stream-node.txt` / `build/stream-perry.txt`）。

> 注：`PERRY_RS4GC=0` 是绕过而非修复；perry 上游修复 `.seh` 汇编 bug 后可去掉该
> 环境变量，恢复默认 GC 策略。

与 QuickJS 后端的另一个差异：**Perry 后端没有 BOM**（没有 `window`/`setTimeout`/
`crypto`/`requestAnimationFrame`，弹窗更无从谈起）。前端所有 BOM 访问都收敛在
`ui/src/bom.ts` 的 `hasBom = typeof window !== "undefined"` 探测之后，取不到就降级
成占位文本——同一份 `app.tsx` 因此能原样跑在两个后端上，BOM 卡片在 Perry 下显示
「无 BOM（Perry 后端）」而不是崩掉。

## 组件库与绘制层

前端只有 `h()` + mutation 协议，所以"组件"天然分两层：一层是 TS 侧的**描述**，
一层是宿主侧的**像素**。

### `ui/src/kit.tsx`：可直接用的控件

| 组件 | 生成什么 | 宿主额外负责什么 |
| --- | --- | --- |
| `TextField` | `tag:"input"` + `placeholder` + 受控 `value` | 真实文本编辑、光标绘制、焦点/键盘、`input`/`change` 回传 |
| `Radio` / `RadioGroup` | `div` + 圆点，选中态靠样式切换 | 无（纯样式） |
| `Checkbox` / `Switch` | `div` + `☑` / 滑块（`justifyContent` 在 start/end 间切） | 无（纯样式） |
| `Select` | 当前位置 + `position:absolute` 面板 + `elevate:20` | `deferred` 抬升绘制层级 |
| `ScrollArea` | `overflow:"scroll"` + `height` 或 `grow` | 滚动句柄、裁剪、滚动条绘制、`scroll` 事件 |
| `CanvasView` | `tag:"canvas"`，绘制函数在 `createEffect` 里录显示列表 | 遍历显示列表做 GPU 绘制 |

`ui/src/theme.ts` 是共用色板；`ui/src/runtime.ts` 是 `h()` / signal / 事件绑定层
（`value` 传 getter 时自动用 `createEffect` 重推，`onInput`/`onChange`/`onScroll`/
`onFocus`/`onBlur` 各自声明 `setEvents`）。

### `ui/src/canvas2d.ts`：Canvas 2D 的可用子集

`createCanvas(w, h)` 返回一个**形状兼容** `CanvasRenderingContext2D` 的录制器：
`fillStyle/strokeStyle/lineWidth/fontSize/fontWeight/globalAlpha` 加
`fillRect/strokeRect/line/lineTo/arc/closePath/fill/stroke/circle/ellipse/poly/
text/beginPath/moveTo`。`reset()` 开始一帧，`present()` 把这一帧录到的内容一次性
`setCanvas` 发出去。

与浏览器 Canvas 的**有意偏离**（源码注释里也列了一遍）：

- `text` 的 `y` 是**行盒顶部**而不是基线；没有 `measureText`，要自己估宽度；
- 没有 `clearRect` / `save`/`restore` / `transform` / 渐变 / 图片 / 阴影 / 裁剪路径；
- `arc` 采样成折线（落到 `poly` 原语），不是真正的圆弧。

### `host/src/draw.rs`：绘制词表（可单测）

5 个原语 `rect / ellipse / line / poly / text`。`parse_cmds` 解析显示列表
（8192 条上限，超出计入 `skipped` 并打日志），`parse_color` 认 CSS 那几种写法，
`with_alpha` 等价 `globalAlpha`，`scrollbar_thumb(viewport, content, offset, track,
min_thumb)` 返回滑块的位置与高度（最小 24px；内容不溢出时返回 `None`）。
**这一层不依赖 GPUI 类型**，所以 `cargo test` 能无窗口跑——11 个单测覆盖颜色各形态、
alpha 只作用于 A 通道、畸形条目计入 skipped、上限截断、滑块进度映射与钳制。

### 职责边界（为什么 caret 与滚动条在宿主）

- **值是前端的，光标是宿主的**：`setValue` 回传同一个值（受控组件的回声）时宿主
  **不动光标**；只有外部真的改了值才把光标收敛到末尾；
- **caret 用字符索引**而非字节索引（CJK 下只有字符索引和用户心里数的数一致），
  画光标时用 `ShapedLine::split_at` 求 advance width，越过右边界时把文本整体左移；
- **滚动条几何在宿主算**：它是 overlay，不占布局宽度，滑块位置每次 paint 重算；
  而且它是滚动容器的 **absolute 兄弟节点**而不是子节点——作为子节点会跟着内容滚走；
- **滚动容器是两层结构**：wrapper 持有 box 类样式（`width`/`height`/`grow`/
  `position`…），内层持有流（`flexDirection`/`gap`/`padding`/`overflow`）与滚动句柄。
  wrapper 同时是 `relative` 的定位基准，好让滚动条贴边。

### 已知缺口

- **没有 IME 组合输入**：中文只能 `Ctrl+V` 粘贴（`Ctrl+V` 是 `input_key` 里唯一的
  多字节入口）；
- **点击 input 不按点击位置定位光标**（监听器拿不到元素 bounds），一律落到末尾；
- **没有选区模型**，`Ctrl+A` 只是把光标移到末尾；
- 滚动只做**纵向**，没有横向；
- canvas 无 `clearRect` / `transform` / 渐变 / 图片。

## 复用的已验证组件

- `gpui-pre 0.3.3` / `gpui-pre-platform 0.3.3`：Zed gpui 的 crates.io 快照（zed@5b055fa）。
  （2026-09-22 起不再使用 `gpui-pre-windows` 本地补丁——其唯一作用是去掉 `u_strlen`
  对系统 icuuc.dll 的依赖（Win10 1803 以下兼容），对体积零影响；如需兼容老系统，
  从 rebased-rs 项目恢复补丁目录并还原 host/Cargo.toml 的 `[patch.crates-io]` 条目）；
- MSVC 工具链环境变量与构建参数沿用本机 rebased-rs 项目验证过的配置。

## 后续路线

- IME 组合输入（中文直输，替代"只能 Ctrl+V"）、选区模型、点击定位光标
- 横向滚动、图片节点、动画（gpui animate）
- 运行时层已用内嵌 QuickJS 替换 Perry（体积 -33%、自拥 10ms 事件循环）；后续可考虑
  评估 **PrimJS** 等更小的引擎，或按需切换 ES 版本/预编译字节码（QuickJS bytecode）
  省掉启动期 parse
- 库化收尾：`gpui-ts` 目前只发 `win32-x64` 宿主（macOS/Linux 需从源码构建后放进
  `vendor/<platform>-<arch>/`）；包内后端固定 QuickJS，Perry 仅作仓库内对照；
  尚无 devtools/HMR（调试靠 `PERRY_UI_TRACE=1` 与宿主日志）
- Perry 后端仅为对照保留；上游修复 `.seh` 汇编 bug 后可去掉 `PERRY_RS4GC=0`

已完成（2026-09-23）：`input` / radio / checkbox / switch / select / 滚动容器 /
canvas 显示列表，见上节「组件库与绘制层」；并打成了可发布的 npm 包 `gpui-ts`
（含 `.d.ts` 与预编译宿主，使用者无需任何编译环境），见「发布为 npm 库」。

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

- `node ui/pretranspile-tsx.mjs` 等价步骤已内建：`npx perry compile build/gen/main.ts
  --output-type staticlib -o ../build/app-main.lib`（`build/gen` = esbuild 预转的 TSX；
  基名由入口推出来，`scripts/gpui-ts.mjs` 通过 `PERRY_STATIC_LIB_BASE` 告诉 build.rs）
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
  > `/FORCE:MULTIPLE` 也意味着**链错 archive 不会报错**：见「坑 #7」。`build.rs`
  > 现在按 `PERRY_STATIC_LIB_BASE` 解析，并且 `scripts/gpui-ts.mjs` 会在链接后
  > 校验 exe 里含当前前端的字符串。
- stdio 即协议：`console.log`/`process.stdin` 全走进程 std handles，pipe 重定向后
  JSONL 协议**零改动**，gpui host 把「子进程 stdio」换成「进程内 pipe」即可

原型验证结果（build/embed-proto/run.log）：init ops 到达、event 消费
（microtasks=1）、心跳 timer 在 `next_wake_ms≈9` 可见、perry_has_work() 持续为真。
