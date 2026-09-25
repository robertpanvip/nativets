# gpui-ts

用 TypeScript / TSX 写桌面应用，产出一个**原生单文件 exe**。

没有 Rust 工具链，没有 MSVC，没有 Electron，没有浏览器内核。装上这个包，
`npx gpui-ts build src/app.tsx` 就够了。

```bash
npm i gpui-ts
npx gpui-ts build src/app.tsx -o myapp.exe
./myapp.exe
```

- **不需要编译环境** — 宿主二进制随包预编译发布（`vendor/win32-x64/`），
  用户的机器上只要有 Node ≥ 18 即可。
- **单文件产物** — 大约 11.5 MB，一个 exe，拷到任何目录双击就能跑。
  不需要 `node_modules`，不需要一起拷别的文件。
- **原生窗口** — 由 Rust 侧的 [GPUI](https://gpui.rs) 渲染（GPU 加速，
  无 WebView）；窗口是真的系统窗口，任务栏、DPI、缩放都正常。
- **带类型的 API** — 包内附 `.d.ts`，编辑器里补全、跳转、签名提示齐全。

界面层是一个很薄的、类 DOM 的抽象：你写 `h("div", …)` 或 JSX，运行时把它
编译成一组增量指令发给宿主；宿主据此建树、布局、绘制。**没有 DOM，也没有
CSS 引擎**——所以样式是一个受支持的子集，见下文「样式子集」。

---

## 快速开始

`src/app.tsx`：

```tsx
import { createRoot, h, createSignal, text } from "gpui-ts";

function App() {
    const [n, setN] = createSignal(0);

    // Return the UI, or mount it yourself with `appendChild(root, …)`.
    return (
        <div style={{ flexDirection: "column", gap: 12, padding: 16 }}>
            <button style={{ padding: 8, borderRadius: 6 }} onClick={() => setN(n() + 1)}>
                {text("＋ 加一")}
            </button>
            {text(() => "当前 " + n(), { fontSize: 20 })}
        </div>
    );
}

createRoot({ title: "My App" }, App);
```

```bash
npx gpui-ts build src/app.tsx -o myapp.exe
npx gpui-ts run   src/app.tsx      # 编译到临时文件并直接运行
npx gpui-ts doctor                 # 检查本机这套安装是否可用
```

`App` 可以**返回**要显示的 UI（`El`、字符串、数字，或它们的数组），也可以自己
挂到传进来的 `root` 上：`appendChild(root, <div/>)`。两种写法都行。

> **`root` 只传给入口 `App`。** 作为 JSX 子节点使用的组件必须**返回**元素：
>
> ```tsx
> function Badge(props: { n: number }) {         // ✅ 返回元素
>     return <div>{text("n=" + props.n)}</div>;
> }
> function Bad(props: {}) {                      // ❌ 拿不到 root，
>     appendChild(root, <div/>);                 //    root 在这层不存在
> }
> ```
>
> 组件按**一个 props 对象**调用（`h(Badge, { n: 1 })` → `Badge({ n: 1 })`），
> 子节点在 `props.children` 上。

**入口文件必须引入 `h`**：JSX 走的是经典转换（`jsxFactory: "h"`），
`<div/>` 会被编译成 `h("div", …)`。用 `<>…</>` 片段时还要引入 `Fragment`，
`<For …/>` 这种「多参数函数」不要当组件用，直接调用即可：
`{For(() => rows, (r) => text(r))}`。

### CLI

| 命令 | 作用 |
| --- | --- |
| `gpui-ts build <entry.tsx> [-o <out.exe>]` | 打包成单个自包含 exe，默认输出到 `<entry>.exe` |
| `gpui-ts run <entry.tsx>` | 先 build 到临时文件，再运行，退出后清理 |
| `gpui-ts doctor` | 打印 Node 版本、当前平台、宿主二进制路径与体积 |
| `gpui-ts --help` / `--version` | 用法与版本 |

`build` 失败时退出码为 2 并打印原因（入口不存在、当前平台没有预编译宿主、esbuild 缺失等）。

---

## 渲染模型：一个应用就是一棵元素树

`createRoot(spec, App)` 调用 `App(root)`，你在里面往 `root` 上挂一棵树（或者
直接返回那棵树）。
之后**只有三种方式**能改变界面：

1. **信号** — `createSignal` 的值变了，所有读过它的地方自动更新：
   `text(() => …)`、`Show(() => …)`、`For(() => …)`、`createEffect(() => …)`。
2. **事件** — 宿主把交互回传成事件（`onClick`、`onInput`、`onScroll`、…），
   你的回调里改信号。
3. **直接操作** — `setStyle` / `setValue` / `setCanvas` / `appendChild` / `remove`
   （很少需要，组件库和 `createEffect` 已经覆盖大多数场景）。

`App` 的函数体只执行一次——**它不是渲染函数**。别在函数体里读信号来"取当前值
并渲染"；体内的信号读取不会被追踪。要随信号变化的东西，要么写进
`() => …` 形式的参数里，要么放进 `createEffect`。

### 元素与组件

`h(tag, props, ...children)` 是唯一的工厂函数，`tag` 可以是：

- 内置标签：`div`、`span`、`input`、`button`、`canvas`
- 你自己的函数组件：`h(Card, { title: "…" }, child)`——**组件按一个 props 对象调用**：

```tsx
function Card(props: { title: string; children?: any }) {
    return (
        <div style={{ flexDirection: "column", gap: 8 }}>
            {text(props.title)}
            {props.children}
        </div>
    );
}
```

也就是说 `function Card(title: string, children)` 是错的（会渲染成
`[object Object]`）；框架传进来的是单个对象，子节点在 `props.children` 上。

---

## API

包导出四个模块的内容（`runtime` / `kit` / `canvas2d` / `theme`），
`import { … } from "gpui-ts"` 一律可用。

### runtime

| 名称 | 说明 |
| --- | --- |
| `createRoot(spec, App)` | 启动应用；`spec.title` 设置窗口标题 |
| `h` / `Fragment` / `text` | JSX 工厂、片段标记、文本节点 |
| `createSignal<T>(v)` | `[get, set]`；`get()` 读值并注册依赖 |
| `createEffect(fn)` | 副作用；`fn` 里读到的信号变化时重跑 |
| `Show(cond, render)` | 条件渲染，`cond` 是 `() => boolean` |
| `For(items, render)` | 列表渲染，`items` 是 `() => T[]` |
| `setStyle` / `setValue` / `setCanvas` | 增量修改已有的元素 |
| `appendChild` / `remove` / `setRootStyle` | 树结构操作 |
| `onHostEvent(fn)` | 订阅宿主事件（`kind` 字符串），用于埋点/日志 |
| `stats` | 内部计数器（op 数、事件数等），调试用 |

### kit（组件）

| 组件 | 关键 props |
| --- | --- |
| `TextField` | `value: () => string`、`onInput(v)`、`onChange?(v)`、`placeholder`、`label`、`width` |
| `Select` | `options: string[]`、`value: () => string`、`onChange(v)`、`width` |
| `RadioGroup` | 选项数组 + `value: () => string` + `onChange` |
| `Radio` | `label`、`selected: () => boolean`、`onSelect()` |
| `Checkbox` | `label`、`checked: () => boolean`、`onToggle()` |
| `Switch` | `checked: () => boolean`、`onToggle()`、`label?` |
| `ScrollArea` | `height?`、`grow?`、`onScroll?(ev)`、`children`、`style?` |
| `CanvasView` | `width`、`height`、`draw(ctx)`、`style?`、`onPresent?` |

受控组件的约定是「**值归你、光标归宿主**」：`value` 传一个 getter，用户输入时
`onInput` 拿到新字符串，你更新信号，值再流回组件。这样宿主不需要猜你的状态模型。
输入框的回声（值没变）不会移动光标；只有从外部改值才会把光标收敛到末尾。

### canvas2d

`CanvasView` 内部就是一个「录制器 → 显示列表」：`draw(ctx)` 里调的每个方法都
只记录一条指令，最后一次性发给宿主，由宿主 GPU 绘制。

```tsx
<CanvasView
    width={420}
    height={180}
    draw={(ctx) => {
        ctx.fillStyle = "#4c8dff";
        for (let i = 0; i < bars().length; i++) {
            ctx.fillRect(i * 40, 180 - bars()[i], 30, bars()[i], 4);
        }
        ctx.fillStyle = "#e6e8ee";
        ctx.text("seed " + seed(), 8, 6);
    }}
/>
```

`Ctx` 支持：`fillRect` / `strokeRect`（可选圆角）、`line`、`circle`、`ellipse`、
`poly`、`text`，以及路径 API `beginPath` / `moveTo` / `lineTo` / `arc` /
`closePath` / `fill` / `stroke`。绘制状态有 `fillStyle`、`strokeStyle`、
`lineWidth`、`fontSize`、`fontWeight`、`globalAlpha`（0..1，乘到该指令每个颜色上）。

**这是 Canvas 2D 的一个子集，不是实现**。已知偏离：

- 贝塞尔曲线（`bezierCurveTo` / `quadraticCurveTo`）没有；`arc` 在前端被采样成
  折线（约 48 段/整圆），所以弧形是折线而非真曲线。
- 没有 `save` / `restore` / `transform` / 裁剪 / 渐变 / 图案 / 阴影。
- 没有 `measureText`；`text()` 的 `y` 是**行盒顶部**，不是基线（和 `fillText` 不同）。
- 每帧指令数上限 8192；超出的会被丢弃并在宿主日志里计数（`skipped N malformed cmds`）。
- `ctx.text(text, x, y)` 的参数顺序与 `fillText` 一致——`text` 是方法名，别把参数写反。

### theme

`C` 是一个调色板对象（背景、卡片、边框、主/次文本、强调色、成功/警告/危险等）。
直接用，也可以整包替换成自己的主题。

---

## 样式子集

样式是普通对象，键名接近 CSS 但由宿主解释：

- **布局**：`flexDirection`、`gap`、`padding` / `paddingX` / `paddingY` /
  `paddingTop` 等、`margin*`、`width` / `height`、`minWidth` / `maxHeight` 等。
  数值按 px 处理，也接受 `"100%"` / `"full"`（= 撑满父级）。
- **`grow`** = CSS 的 `flex: n 1 0%`——「分掉**剩余**空间」。整条链上父容器
  必须是有界高度，`grow` 才有意义。
- **`shrink`** = `flex-shrink`；**`display: "contents"`** 表示这个节点透明（不
  产生元素，子节点直接加入父级流），用于 `Show` / 片段这类包裹节点。
- **视觉**：`background`、`borderWidth` / `borderColor` / `borderRadius`、
  `color`、`fontSize`、`fontWeight`、`textAlign`、`overflow`。
- **`position: "absolute"`** 配合 `top` / `right` / `bottom` / `left` 脱离文档流。
- **`elevate: n`** 不是 CSS，语义是 z-index：`n > 0` 时该节点被延迟绘制，
  能盖住它之后的兄弟节点。下拉面板这类浮层必须设它，否则会被下面的内容盖住。

浏览器 CSS 的很多行为在这里并不存在，常见的三个坑：

1. 只设 `grow: 1` 不等于 `flex: 1`——要「平分」得让父级有确定高度。
2. 宿主里 flex 子项的最小尺寸默认是 **0**（CSS 是 `min-height: auto`），
   所以内容不会自动撑开容器，需要显式给高度或用 `ScrollArea`。
3. `grow` 只在**有界且不滚动**的容器里才等于「平分」；滚动容器里请用
   `ScrollArea grow` 并给外层一个确定高度。

---

## 它是怎么工作的

```
app.tsx ──esbuild(IIFE)──► 一段 JS ──追加到宿主二进制尾部──► myapp.exe
                                                                  │
                    宿主启动，从自己的文件里读回那段 JS ◄──────────┘
                                                                  │
              QuickJS 线程求值 ──JSONL 指令──► GPUI 建树/布局/绘制
                          ▲                        │
                          └────── 事件回传 ────────┘
```

宿主二进制随包发布；你的应用以「script 数据」的形式**附在它尾部**——
Windows 的 PE 加载器会忽略可执行文件末尾的多余字节，所以产物就是一个普通
exe，只是顺便带着一段脚本。宿主启动时按标记（`<<<GPUI_TS_BUNDLE_v1>>>`）
把它切出来，交给内嵌的 QuickJS 执行。

这也是为什么产物能这么小、为什么用户不需要任何编译环境。

---

## 当前限制

- **平台**：目前只发布 `win32-x64` 的宿主。其他平台 `gpui-ts doctor` 会告诉你
  缺什么；从源码构建宿主后放进 `vendor/<platform>-<arch>/` 即可。
- **JS 子集**：应用代码最终由 QuickJS 执行，不要依赖 V8 特有的东西。
  `async` / `await` 可用；Node 内置模块（`fs`、`path` 等）**不可用**——
  `process` 是一个很小的 shim（`stdout` / `stderr` / `stdin` / `env` / `argv` /
  `exit`，以及 `setTimeout` 等定时器）。
- **没有 DOM / CSS 引擎**：见上文样式子集与已知偏离。
- **没有 devtools**：调试靠 `process.stdout.write` 和宿主日志
  （设 `PERRY_UI_TRACE=1` 可看到事件与指令的时间线）。

## License

MIT
