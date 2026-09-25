/**
 * Demo app — nativets × GPUI desktop dashboard.
 *
 * Authored in TSX. The build compiles JSX directly to the runtime's `h()`
 * factory (`--jsx=transform --jsx-factory=h --jsx-fragment=Fragment`), so JSX
 * is pure sugar over the same mutation protocol:
 *
 *     <div style={{ padding: 16 }}>hi {n}</div>
 *         ⟺   h("div", { style: { padding: 16 } }, "hi ", n)
 *
 * A capitalised tag is a component: `<Card title="…">…</Card>` becomes
 * `h(Card, { title }, …)`, with children delivered as `props.children`.
 *
 * Styling lives in exactly one place — the `style` prop. Intrinsic tags carry
 * no other presentation keys, so a component's own props (`bg`, `fg`, `color`,
 * `value`, …) can never collide with a style name, and it is always obvious
 * which attributes reach the host's style engine. The other two props an
 * intrinsic tag understands are `onClick` and `text`.
 *
 * The BOM card below exercises the browser-ish globals the host installs for
 * this backend (`window` metrics + `resize`, `navigator`, `location`,
 * `performance.now`, `crypto.randomUUID`, `requestAnimationFrame` and the
 * `alert`/`confirm` modals). All access goes through `./bom` so the same source
 * still runs on the Perry backend, which has no BOM.
 */

import {
    createSignal,
    h,
    text,
    For,
    onHostEvent,
    onPulse,
    setRootStyle,
    appendChild,
    stats,
    engineName,
} from "./io";
import type { El, Child, HostEvent, Style, Props } from "./io";
import { C } from "./theme";
import { CanvasView, Checkbox, DatePicker, Input, RadioGroup, ScrollArea, Select, Switch } from "./kit";
import type { Ctx } from "./canvas2d";
import {
    windowSize,
    dpr,
    userAgent,
    href,
    perfNow,
    uptimeLabel,
    newUuid,
    onResize,
    raf,
    askConfirm,
    notify,
} from "./bom";

// `h` is referenced by the JSX transform itself, not by hand-written calls —
// keep it imported even though no `h(` appears in this file.

// The palette lives in `theme.ts` so the kit's controls and these cards cannot
// drift apart.

// ---------------------------------------------------------------------------
// state
// ---------------------------------------------------------------------------

interface Task {
    id: number;
    title: string;
    done: boolean;
}

const [count, setCount] = createSignal(0);
const [nav, setNav] = createSignal("仪表盘");
const [clicks, setClicks] = createSignal(0);
const [batches, setBatches] = createSignal(0);
const [opsTotal, setOpsTotal] = createSignal(0);
const [clock, setClock] = createSignal("--:--:--");
const [tasks, setTasks] = createSignal<Task[]>([
    { id: 1, title: "Perry 编译 TS → 原生二进制", done: true },
    { id: 2, title: "GPUI retained tree 渲染", done: true },
    { id: 3, title: "mutation 协议 · 注入通道回环", done: false },
    { id: 4, title: "Solid 风格细粒度更新", done: false },
]);

let nextTaskId = 5;
const samples = [
    "验证 Windows Direct3D 后端",
    "对比 napi 桥与注入通道延迟",
    "补 TextInput 协议扩展",
    "写 README 架构图",
];

// --- form state (the kit controls below) ---

const [env, setEnv] = createSignal("生产");
const [verbose, setVerbose] = createSignal(true);
const [autoRefresh, setAutoRefresh] = createSignal(false);
const [region, setRegion] = createSignal("上海");
const [keyword, setKeyword] = createSignal("");
/** What the field reported on Enter — proves `change` is a separate event. */
const [committed, setCommitted] = createSignal("-");
const [fieldEvents, setFieldEvents] = createSignal(0);
/** Selected date from the native DatePicker (ISO "YYYY-MM-DD" or ""). */
const [pickedDate, setPickedDate] = createSignal("");

// --- scroll state ---

const [scrollPct, setScrollPct] = createSignal(0);
const [scrollTop, setScrollTop] = createSignal(0);
const [scrollContent, setScrollContent] = createSignal(0);
const [scrollEvents, setScrollEvents] = createSignal(0);

// --- canvas state ---

/** Eight samples of "mutation batches per 5s" — regenerated on demand. */
const [bars, setBars] = createSignal<number[]>([38, 61, 27, 74, 52, 88, 43, 66]);
const [cmdCount, setCmdCount] = createSignal(0);
const [redraws, setRedraws] = createSignal(0);

// --- BOM state (see ./bom: all reads degrade when there is no BOM) ---

const FRAME_TOTAL = 60;
const [winLabel, setWinLabel] = createSignal(windowSize());
const [uuidText, setUuidText] = createSignal(newUuid());
const [frame, setFrame] = createSignal(0);
const [frameRunning, setFrameRunning] = createSignal(false);
/** Host uptime when the app mounted — one datapoint for the perf row. */
const mountMs = Math.round(perfNow());

// ---------------------------------------------------------------------------
// shared components
// ---------------------------------------------------------------------------

function Card(props: { title: string; children?: Child }): El {
    return (
        <div
            style={{
                flexDirection: "column",
                gap: 10,
                padding: 16,
                background: C.card,
                borderRadius: 12,
                borderWidth: 1,
                borderColor: C.border,
                // Cards keep their content height: they live inside the main
                // scroller, so growing would make them fight for the viewport
                // and shrinking would squash their content into the card above.
                // (The host also pins scroller children — see `build_node`.)
                shrink: 0,
            }}
        >
            {text(props.title, { fontSize: 12, color: C.textSecondary })}
            {props.children}
        </div>
    );
}

function StatCard(props: { label: string; value: () => string; color: string }): El {
    return (
        <div
            style={{
                flexDirection: "column",
                gap: 6,
                padding: 14,
                background: C.cardAlt,
                borderRadius: 10,
                borderWidth: 1,
                borderColor: C.border,
                grow: 1,
            }}
        >
            {text(props.label, { fontSize: 11, color: C.textMuted })}
            {text(props.value, { fontSize: 24, fontWeight: "bold", color: props.color })}
        </div>
    );
}

/** `bg` / `fg` are *component* props — they are not style keys, which is the
 *  whole point of keeping styling behind `style`. The native `button` tag is
 *  rendered by the host as a real gpui-component `Button`; `bg`/`fg` cross as
 *  style overrides on the same channel. */
function PillBtn(props: { bg: string; fg: string; onClick: () => void; children?: Child }): El {
    const style: Style = {
        background: props.bg,
        color: props.fg,
        fontSize: 14,
        fontWeight: "medium",
    };
    return (
        <button style={style} onClick={props.onClick}>
            {props.children}
        </button>
    );
}

/** Label + reactive value, the BOM card's unit of layout. */
function Field(props: { label: string; value: () => string }): El {
    return (
        <div style={{ flexDirection: "column", gap: 4, grow: 1 }}>
            {text(props.label, { fontSize: 11, color: C.textMuted })}
            {text(props.value, { fontSize: 13, color: C.textPrimary })}
        </div>
    );
}

/** `██░░` — a bar that can only change via a reactive text update. */
function bar(n: number, total: number): string {
    const filled = Math.round((n / total) * 20);
    return "█".repeat(filled) + "░".repeat(20 - filled);
}

/**
 * Progress-bar text for the rAF demo.
 *
 * `frame()` is bound to a local rather than inlined as `bar(frame(), …)`, per
 * this project's rule about calls in argument positions (README 坑 #3). Note
 * that binding did *not* turn out to be the reason the BOM card went missing on
 * the staticlib path — that was a stale archive link (坑 #7) — so this is
 * cheap insurance, not a fix.
 */
function progressText(): string {
    const n = frame();
    return bar(n, FRAME_TOTAL);
}

/** `第 22 / 60 帧` — the counter next to the bar. */
function frameLabel(): string {
    const n = frame();
    return "第 " + n + " / " + FRAME_TOTAL + " 帧";
}

/** Mint a fresh v4 UUID into the card. */
function newUuidClick(): void {
    setUuidText(newUuid());
}

// ---------------------------------------------------------------------------
// sections
// ---------------------------------------------------------------------------

function Header(_props: Props): El {
    return (
        <div
            style={{
                flexDirection: "row",
                alignItems: "center",
                justifyContent: "between",
                padding: 14,
                paddingX: 18,
                background: C.card,
                borderRadius: 12,
                borderWidth: 1,
                borderColor: C.border,
            }}
        >
            <div style={{ flexDirection: "row", alignItems: "center", gap: 12 }}>
                <div
                    style={{
                        background: C.accent,
                        color: "#ffffff",
                        fontSize: 16,
                        fontWeight: "bold",
                        width: 34,
                        height: 34,
                        alignItems: "center",
                        justifyContent: "center",
                        borderRadius: 9,
                    }}
                >
                    P
                </div>
                <div style={{ flexDirection: "column", gap: 2 }}>
                    {text("PerryTS × GPUI", { fontSize: 17, fontWeight: "bold", color: C.textPrimary })}
                    {text("TypeScript 前端 · Perry 原生编译 · GPUI GPU 渲染", {
                        fontSize: 12,
                        color: C.textSecondary,
                    })}
                </div>
            </div>
            {text(clock, { fontSize: 15, fontWeight: "medium", color: C.accent })}
        </div>
    );
}

function NavItem(props: { label: string }): El {
    const active = nav() === props.label;
    const style: Style = {
        padding: 11,
        paddingX: 14,
        borderRadius: 9,
        fontSize: 14,
        background: active ? C.elevated : C.bg,
        color: active ? C.accent : C.textSecondary,
        borderWidth: 1,
        borderColor: active ? C.accent : C.border,
    };
    return (
        <div style={style} onClick={() => setNav(props.label)}>
            {props.label}
        </div>
    );
}

function Sidebar(_props: Props): El {
    const items = ["仪表盘", "任务", "指标", "日志", "设置"];
    return (
        <div style={{ flexDirection: "column", gap: 6, width: 178 }}>
            {For(
                () => items,
                (label) => <NavItem label={label} />
            )}
        </div>
    );
}

function CounterCard(_props: Props): El {
    return (
        <Card title="计数器 · 状态 → mutation → GPU 回环">
            <div style={{ flexDirection: "row", alignItems: "center", gap: 14 }}>
                <PillBtn bg={C.accentDim} fg={C.textPrimary} onClick={() => setCount(count() - 1)}>
                    -
                </PillBtn>
                {text(() => String(count()), {
                    fontSize: 34,
                    fontWeight: "bold",
                    color: C.textPrimary,
                    width: 90,
                    alignItems: "center",
                })}
                <PillBtn bg={C.accent} fg="#ffffff" onClick={() => setCount(count() + 1)}>
                    +
                </PillBtn>
                <PillBtn bg={C.bg} fg={C.textSecondary} onClick={resetCount}>
                    重置
                </PillBtn>
            </div>
            {text(() => (count() >= 10 ? "★ 状态更新正常，GPU 渲染流畅" : "点击按钮测试细粒度更新"), {
                fontSize: 12,
                color: C.textMuted,
            })}
        </Card>
    );
}

function TaskRow(props: { t: Task }): El {
    const t = props.t;
    return (
        <div
            style={{
                flexDirection: "row",
                alignItems: "center",
                justifyContent: "between",
                padding: 10,
                paddingX: 12,
                background: C.cardAlt,
                borderRadius: 8,
                borderWidth: 1,
                borderColor: C.border,
            }}
        >
            <div style={{ flexDirection: "row", alignItems: "center", gap: 10 }}>
                <div
                    style={{
                        color: t.done ? C.green : C.textMuted,
                        fontSize: 15,
                        fontWeight: "bold",
                        paddingX: 4,
                    }}
                    onClick={() => toggleTask(t.id)}
                >
                    {t.done ? "√" : "○"}
                </div>
                {text(t.title, { fontSize: 13, color: t.done ? C.textMuted : C.textPrimary })}
            </div>
            <div
                style={{ color: C.red, fontSize: 12, paddingX: 8, paddingY: 4, borderRadius: 6 }}
                onClick={() => removeTask(t.id)}
            >
                删除
            </div>
        </div>
    );
}

function TasksCard(_props: Props): El {
    return (
        <Card title="任务列表 · For 动态渲染 + 事件回传">
            <div style={{ flexDirection: "column", gap: 8 }}>
                {For(
                    tasks,
                    (t) => <TaskRow t={t} />
                )}
            </div>
            <div style={{ flexDirection: "row", gap: 10, alignItems: "center" }}>
                <PillBtn bg={C.elevated} fg={C.accent} onClick={addTask}>
                    ＋ 快速添加
                </PillBtn>
                {text(() => "已完成 " + doneCount() + " / " + tasks().length, {
                    fontSize: 12,
                    color: C.textMuted,
                })}
            </div>
        </Card>
    );
}

/**
 * Everything the host's BOM provides, on display.
 *
 * Note the shape of the frame demo: `requestAnimationFrame` is paced by the
 * engine tick (see `bootstrap.js`), so it runs at a stable 50Hz and stops the
 * moment the last callback is not re-registered.
 */
const ROW: Style = { flexDirection: "row", gap: 18 };

/**
 * Row 1 — window metrics, all read live from the BOM getters.
 */
function BomRowMetrics(_props: Props): El {
    return (
        <div style={ROW}>
            <Field label="window 尺寸（随 resize 更新）" value={winLabel} />
            <Field label="devicePixelRatio" value={dpr} />
            <Field label="performance.now() @挂载" value={() => uptimeLabel(mountMs)} />
        </div>
    );
}

/** Row 2 — environment identity: where this bundle thinks it is running. */
function BomRowEnv(_props: Props): El {
    return (
        <div style={ROW}>
            <Field label="location.href" value={href} />
            <Field label="navigator.userAgent" value={userAgent} />
            <Field label="crypto.randomUUID()" value={uuidText} />
        </div>
    );
}

/**
 * Row 3 — the interactive parts: a finite rAF burst, a fresh UUID, the host
 * alert, and a text progress bar driven by the frame signal.
 *
 * Split out of `BomCard` on purpose. As one function `BomCard`'s body was a
 * single ~30-deep nested `h()` expression, and perry's `--output-type
 * staticlib` backend silently dropped the whole component (0 of its ops
 * emitted, no error anywhere). Small components keep every expression shallow.
 */
function BomRowActions(_props: Props): El {
    return (
        <div style={{ flexDirection: "row", alignItems: "center", gap: 12 }}>
            <PillBtn bg={C.elevated} fg={C.accent} onClick={runFrames}>
                {text(() => (frameRunning() ? "… 动画中" : "▶ 帧动画 rAF"))}
            </PillBtn>
            <PillBtn bg={C.bg} fg={C.textSecondary} onClick={newUuidClick}>
                新 UUID
            </PillBtn>
            <PillBtn bg={C.bg} fg={C.textSecondary} onClick={demoAlert}>
                alert 弹窗
            </PillBtn>
            {text(progressText, { fontSize: 13, color: C.accent })}
            {text(frameLabel, { fontSize: 11, color: C.textMuted })}
        </div>
    );
}

function BomCard(_props: Props): El {
    return (
        <Card title="BOM · 宿主注入的浏览器环境">
            <BomRowMetrics />
            <BomRowEnv />
            <BomRowActions />
        </Card>
    );
}

// ---------------------------------------------------------------------------
// form controls card — input / radio / checkbox / switch / select
// ---------------------------------------------------------------------------

/** Every control on one line. Reading four signals here is all it takes for the
 *  text to keep itself current. */
function formSummary(): string {
    const mode = verbose() ? "详细" : "精简";
    const refresh = autoRefresh() ? "自动刷新" : "手动刷新";
    return env() + " · " + region() + " · " + mode + " · " + refresh;
}

function committedLabel(): string {
    const v = committed();
    return "input 事件 " + fieldEvents() + " 次 · Enter 提交值：" + (v === "" ? "(空)" : v);
}

function FormCard(_props: Props): El {
    return (
        <Card title="表单控件 · input / radio / checkbox / switch / select">
            <div style={{ flexDirection: "row", gap: 24, alignItems: "start" }}>
                <div style={{ flexDirection: "column", gap: 12, width: 300 }}>
                    <Input
                        label="关键字（回车提交，Ctrl+V 粘贴）"
                        value={keyword}
                        placeholder="输入后回车…"
                        onInput={(v: string) => {
                            setKeyword(v);
                            setFieldEvents(fieldEvents() + 1);
                        }}
                        onChange={(v: string) => setCommitted(v)}
                    />
                    <DatePicker
                        value={pickedDate}
                        placeholder="选择日期…"
                        onChange={(v: string) => {
                            setPickedDate(v);
                            console.log("[app] date picked:", v);
                        }}
                    />
                    <div style={{ flexDirection: "row", alignItems: "center", gap: 14 }}>
                        <Select options={REGIONS} value={region} onChange={setRegion} />
                        <Switch
                            checked={autoRefresh}
                            onToggle={() => setAutoRefresh(!autoRefresh())}
                            label="自动刷新"
                        />
                    </div>
                </div>
                <div style={{ flexDirection: "column", gap: 12, grow: 1 }}>
                    <RadioGroup options={ENVS} value={env} onChange={setEnv} />
                    <Checkbox
                        label="输出详细日志"
                        checked={verbose}
                        onToggle={() => setVerbose(!verbose())}
                    />
                    {text(() => "当前：" + formSummary(), { fontSize: 12, color: C.textSecondary })}
                    {text(committedLabel, { fontSize: 12, color: C.textMuted })}
                </div>
            </div>
        </Card>
    );
}

const ENVS = ["开发", "预发", "生产"];
const REGIONS = ["上海", "北京", "深圳", "杭州"];

// ---------------------------------------------------------------------------
// canvas card
// ---------------------------------------------------------------------------

const CHART_W = 430;
const CHART_H = 150;

/**
 * Drawn with `ui/src/canvas2d.ts`: every call below becomes one command in the
 * node's display list (`fillRect` → `rect`, the path API → `poly`, …). Nothing
 * here knows about GPUI — and the same function would work on a backend that had
 * a real canvas, which is the point of keeping the API familiar.
 */
function drawChart(ctx: Ctx, w: number, h: number, values: number[], tone: number): void {
    const pad = 26;
    const plotW = w - pad * 2;
    const plotH = h - pad - 24;
    const accent = tone === 0 ? C.accent : C.green;

    let max = 1;
    for (let i = 0; i < values.length; i++) {
        if (values[i] > max) max = values[i];
    }
    const gap = 9;
    const bw = (plotW - gap * (values.length - 1)) / values.length;

    // baseline
    ctx.fillStyle = C.border;
    ctx.fillRect(pad, pad + plotH, plotW, 1, 0);

    const cx: number[] = [];
    const cy: number[] = [];
    for (let i = 0; i < values.length; i++) {
        const bh = Math.round((values[i] / max) * plotH);
        const x = pad + i * (bw + gap);
        const y = pad + plotH - bh;
        const isLast = i === values.length - 1;
        ctx.fillStyle = isLast ? C.cyan : accent;
        ctx.fillRect(x, y, bw, bh, 3);
        ctx.fontSize = 10;
        ctx.fillStyle = C.textMuted;
        ctx.text(String(values[i]), x + 1, y - 13, undefined);
        cx.push(x + bw / 2);
        cy.push(y);
    }

    // trend line through the bar tops — the path API half of the recorder
    if (cx.length > 0) {
        ctx.beginPath();
        ctx.moveTo(cx[0], cy[0]);
        for (let i = 1; i < cx.length; i++) {
            ctx.lineTo(cx[i], cy[i]);
        }
        ctx.stroke({ stroke: C.violet, line: 2 });
        for (let i = 0; i < cx.length; i++) {
            ctx.circle(cx[i], cy[i], 2.5, { fill: C.violet });
        }
    }

    ctx.fontSize = 11;
    ctx.fillStyle = C.textSecondary;
    ctx.text("mutation 批 · 每 5s 采样", pad, 6, undefined);
    ctx.fillStyle = C.textMuted;
    ctx.text("sample 1-8", pad + plotW - 60, 6, undefined);
}

/** Deterministic pseudo-random data: a fixed seed makes a redraw reproducible,
 *  which is what lets the ops stream be diffed between runs (and backends). */
let seed = 7;
function nextBars(): number[] {
    const out: number[] = [];
    for (let i = 0; i < 8; i++) {
        seed = (seed * 1103515245 + 12345) % 2147483648;
        out.push(24 + (seed % 72));
    }
    return out;
}

function drawStats(): string {
    return "重绘 " + redraws() + " 次 · 显示列表 " + cmdCount() + " 条";
}

function CanvasCard(_props: Props): El {
    return (
        <Card title="Canvas 2D · 前端录制显示列表 → 宿主 GPU 绘制">
            <div style={{ flexDirection: "row", gap: 16, alignItems: "start" }}>
                <CanvasView
                    width={CHART_W}
                    height={CHART_H}
                    style={{
                        background: C.cardAlt,
                        borderRadius: 10,
                        borderWidth: 1,
                        borderColor: C.border,
                    }}
                    draw={(ctx: Ctx) => {
                        // Bind first (README 坑 #3: nested calls in argument
                        // positions misbehave under perry).
                        const data = bars();
                        const tone = chartTone();
                        drawChart(ctx, CHART_W, CHART_H, data, tone);
                    }}
                    onPresent={(n: number, r: number) => {
                        // Both args come from the canvas's own effect; reading a
                        // signal here that the effect writes would self-trigger.
                        setCmdCount(n);
                        setRedraws(r);
                    }}
                />
                <div style={{ flexDirection: "column", gap: 8, grow: 1 }}>
                    <PillBtn bg={C.elevated} fg={C.accent} onClick={reshuffleBars}>
                        ⟳ 换一组数据
                    </PillBtn>
                    <PillBtn
                        bg={C.bg}
                        fg={C.textSecondary}
                        onClick={() => setChartTone(chartTone() === 0 ? 1 : 0)}
                    >
                        切换配色
                    </PillBtn>
                    {text(drawStats, { fontSize: 11, color: C.textMuted })}
                    {text("改数据或换配色都会自动重绘：绘制函数读到的 signal 就是它的依赖。", {
                        fontSize: 11,
                        color: C.textMuted,
                    })}
                </div>
            </div>
        </Card>
    );
}

const [chartTone, setChartTone] = createSignal(0);

function reshuffleBars(): void {
    setBars(nextBars());
}

// ---------------------------------------------------------------------------
// scroll card
// ---------------------------------------------------------------------------

const LOG_ROWS = [
    "INFO  构建产物 17.0MB，静态库嵌入校验通过",
    "INFO  宿主注入 BOM：window / navigator / crypto",
    "DEBUG 首批 mutation 359 ops，dropped=0",
    "INFO  scroll 事件由宿主在 render 轮询后回传",
    "WARN  perry 后端无 BOM，BOM 卡片自动降级",
    "DEBUG 点击 → 派发 p50 ≈ 8ms（direct 传输）",
    "INFO  Canvas 显示列表：rect / ellipse / line / poly / text",
    "INFO  TextField 的 caret 由宿主维护，值由前端持有",
    "DEBUG 滚动条是宿主绘制的 overlay，不占布局宽度",
    "INFO  焦点事件 focus / blur 用于前端自绘焦点环",
    "WARN  长列表请用 ScrollArea：宿主只排布可见帧",
    "DEBUG select 面板用 position:absolute + elevate",
    "INFO  协议仍是一份 JSONL，两种载体共用同一解析入口",
    "INFO  到底了 —— 这条消息只为了把内容撑出滚动条",
];

function LogRow(props: { row: string; index: number }): El {
    const level = props.row.slice(0, 5).trim();
    const color =
        level === "WARN" ? C.amber : level === "DEBUG" ? C.textMuted : level === "ERROR" ? C.red : C.green;
    return (
        <div
            style={{
                flexDirection: "row",
                gap: 10,
                padding: 7,
                paddingX: 9,
                background: C.card,
                borderRadius: 7,
                borderWidth: 1,
                borderColor: C.border,
            }}
        >
            {text(String(props.index + 1).padStart(2, "0"), { fontSize: 11, color: C.textMuted })}
            {text(level, { fontSize: 11, fontWeight: "bold", color: color })}
            {text(props.row.slice(5).trim(), { fontSize: 12, color: C.textSecondary })}
        </div>
    );
}

/** Host-side numbers, recomputed on every `scroll` event. */
function scrollLabel(): string {
    return "滚动 " + scrollPct() + "% · top " + scrollTop() + "px · 内容高 " + scrollContent() + "px";
}

function onLogScroll(ev: HostEvent): void {
    const top = ev.top === undefined ? 0 : ev.top;
    const max = ev.max === undefined ? 0 : ev.max;
    const content = ev.content === undefined ? 0 : ev.content;
    setScrollTop(Math.round(top));
    setScrollContent(Math.round(content));
    setScrollPct(max > 0 ? Math.round((top / max) * 100) : 0);
    setScrollEvents(scrollEvents() + 1);
}

function ScrollCard(_props: Props): El {
    return (
        <Card title="滚动区 · 宿主滚动 + 宿主绘制滚动条 + scroll 事件">
            <div style={{ flexDirection: "column", gap: 8 }}>
                <div style={{ flexDirection: "row", justifyContent: "between", alignItems: "center" }}>
                    {text(scrollLabel, { fontSize: 12, color: C.textSecondary })}
                    {text(() => "scroll 事件 " + scrollEvents() + " 次", { fontSize: 11, color: C.textMuted })}
                </div>
                <ScrollArea height={168} onScroll={onLogScroll}>
                    {For(
                        () => LOG_ROWS,
                        (row, i) => <LogRow row={row} index={i} />
                    )}
                </ScrollArea>
            </div>
        </Card>
    );
}

function StatsRow(_props: Props): El {
    return (
        <div style={{ flexDirection: "row", gap: 12 }}>
            <StatCard label="点击事件回传" value={() => String(clicks())} color={C.green} />
            <StatCard label="mutation 批" value={() => String(batches())} color={C.accent} />
            <StatCard label="累计 mutation" value={() => String(opsTotal())} color={C.amber} />
        </div>
    );
}

function MainPanel(_props: Props): El {
    return (
        <div style={{ flexDirection: "column", gap: 14, grow: 1, height: "100%", minWidth: 0 }}>
            {/* The whole dashboard body scrolls, not just the cards below the
                fold: a scroll container needs a bounded height (the same rule as
                CSS) and `height: "100%"` inside the sized main panel is what
                gives it one. Scrolling everything keeps the viewport tall enough
                to be worth a scrollbar. */}
            <ScrollArea
                grow
                style={{
                    gap: 14,
                    padding: 0,
                    paddingRight: 8,
                    background: C.bg,
                    borderRadius: 0,
                    borderWidth: 0,
                }}
            >
                <FormCard />
                <StatsRow />
                <CounterCard />
                <BomCard />
                <CanvasCard />
                <ScrollCard />
                <TasksCard />
            </ScrollArea>
        </div>
    );
}

function Footer(_props: Props): El {
    return (
        <div
            style={{
                flexDirection: "row",
                justifyContent: "between",
                alignItems: "center",
                padding: 10,
                paddingX: 16,
                background: C.cardAlt,
                borderRadius: 10,
                borderWidth: 1,
                borderColor: C.border,
            }}
        >
            {text(() => "frontend: " + mode + " · protocol 1", { fontSize: 12, color: C.textMuted })}
            {text(() => "navigation: " + nav(), { fontSize: 12, color: C.textSecondary })}
        </div>
    );
}

// ---------------------------------------------------------------------------
// actions
// ---------------------------------------------------------------------------

function toggleTask(id: number): void {
    // NOTE: 先绑定局部再调用 —— 避免 f(g(h())) 深嵌套（perry 下行为异常）
    const cur = tasks();
    setTasks(cur.map((t) => (t.id === id ? { id: t.id, title: t.title, done: !t.done } : t)));
}

function removeTask(id: number): void {
    const cur = tasks();
    setTasks(cur.filter((t) => t.id !== id));
}

function addTask(): void {
    const title = samples[(nextTaskId - 5) % samples.length];
    const cur = tasks();
    setTasks(cur.concat([{ id: nextTaskId++, title: title, done: false }]));
}

function doneCount(): number {
    return tasks().filter((t) => t.done).length;
}

/**
 * Run a finite burst of animation frames.
 *
 * Deliberately *finite*: a permanent rAF loop would keep one mutation batch per
 * frame crossing the transport forever, which would drown the mutation stats
 * below and keep the engine thread from ever parking.
 */
function runFrames(): void {
    if (frameRunning()) return;
    setFrameRunning(true);
    setFrame(0);
    const step = (): void => {
        const n = frame() + 1;
        setFrame(n);
        if (n < FRAME_TOTAL) raf(step);
        else setFrameRunning(false);
    };
    // `raf` reports whether it actually scheduled anything: without a BOM
    // nothing will ever tick, so do not leave the button in "animating".
    if (!raf(step)) setFrameRunning(false);
}

function demoAlert(): void {
    notify("这是宿主绘制的 GPUI 模态弹窗（alert）—— 前端协议里没有它。");
}

/** `confirm` blocks the engine thread until answered, exactly like a browser. */
function resetCount(): void {
    if (askConfirm("确定要把计数器重置为 0 吗？")) setCount(0);
}

/**
 * Which runtime is hosting this bundle? Reported in the status bar.
 *
 * The label is injected by the platform entry (runtime.ts probes the QuickJS
 * process shim; the scriptc entry sets "scriptc-aot"). Probing `process` here
 * is refused by scriptc's checker (SC1090 — no typeof on a statically-typed
 * value), so the probe lives in the dynamic-engine-only module.
 */
const mode: string = engineName();

// ---------------------------------------------------------------------------
// wiring
// ---------------------------------------------------------------------------

/** Two-digit pad. (scriptc has no padStart; `slice(-2)` is unproven there.) */
function pad2(n: number): string {
    let s: string = String(n);
    if (n < 10) s = "0" + s;
    return s;
}

/** Last second rendered in the clock, so a 4 Hz pulse repaints at 1 Hz. */
let lastClockSec: number = -1;

/** Repaint the clock from the timestamp the platform handed the heartbeat. */
function tickClock(nowMs: number): void {
    const total: number = Math.floor(nowMs / 1000);
    if (total === lastClockSec) return;
    lastClockSec = total;
    const hh: number = Math.floor(total / 3600) % 24;
    const mm: number = Math.floor(total / 60) % 60;
    const ss: number = total % 60;
    setClock(pad2(hh) + ":" + pad2(mm) + ":" + pad2(ss));
}

function wireStats(): void {
    onHostEvent(() => setClicks(clicks() + 1));
    // The host pushes geometry whenever it changes; re-read rather than trusting
    // the event payload, so there is one source of truth for the getters.
    onResize(() => setWinLabel(windowSize()));
    // Stats and clock refresh off the platform heartbeat — `setInterval` is
    // refused by scriptc (SC4005) and the host driver thread already ticks.
    // QuickJS drives this from runtime.ts (250 ms, local wall clock); scriptc
    // from gpts_tick (uptime, since a compiled graph has no clock of its own).
    onPulse((nowMs: number) => {
        setBatches(stats.batchesSent);
        setOpsTotal(stats.opsSent);
        tickClock(nowMs);
    });
}

// ---------------------------------------------------------------------------
// app entry
// ---------------------------------------------------------------------------

export function App(root: El): void {
    setRootStyle({
        flexDirection: "column",
        gap: 12,
        padding: 16,
        width: "100%",
        height: "100%",
        background: C.bg,
        color: C.textPrimary,
        fontSize: 14,
    });
    wireStats();

    // NOTE: 绑定局部变量再传参（perry 0.5.1520 下实参位置的多层调用会错绑）
    const shell = (
        <div style={{ flexDirection: "row", gap: 12, grow: 1 }}>
            <Sidebar />
            <MainPanel />
        </div>
    );

    appendChild(root, <Header />);
    appendChild(root, shell);
    appendChild(root, <Footer />);
}
