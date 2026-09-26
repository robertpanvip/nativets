/**
 * Application state — every `createSignal` and shared data constant lives here so
 * the dashboard cards (`cards/*.tsx`) can import exactly what they need without
 * forming an import cycle with the composition root (`app.tsx`).
 *
 * Pure data only: cards and `primitives.tsx` read/write these; no rendering or
 * DOM logic belongs here.
 */

import { createSignal } from "./io";
import { engineName } from "./io";
import { windowSize, newUuid, perfNow } from "./bom";

export interface Task {
    id: number;
    title: string;
    done: boolean;
}

// --- counters / stats ---
export const [count, setCount] = createSignal(0);
export const [nav, setNav] = createSignal("仪表盘");
export const [clicks, setClicks] = createSignal(0);
export const [batches, setBatches] = createSignal(0);
export const [opsTotal, setOpsTotal] = createSignal(0);
export const [clock, setClock] = createSignal("--:--:--");

// --- tasks ---
export const [tasks, setTasks] = createSignal<Task[]>([
    { id: 1, title: "Perry 编译 TS → 原生二进制", done: true },
    { id: 2, title: "GPUI retained tree 渲染", done: true },
    { id: 3, title: "mutation 协议 · 注入通道回环", done: false },
    { id: 4, title: "Solid 风格细粒度更新", done: false },
]);
export const samples = [
    "验证 Windows Direct3D 后端",
    "对比 napi 桥与注入通道延迟",
    "补 TextInput 协议扩展",
    "写 README 架构图",
];

// --- form state (kit controls) ---
export const [env, setEnv] = createSignal("生产");
export const [verbose, setVerbose] = createSignal(true);
export const [autoRefresh, setAutoRefresh] = createSignal(false);
export const [region, setRegion] = createSignal("上海");
export const [keyword, setKeyword] = createSignal("");
/** What the field reported on Enter — proves `change` is a separate event. */
export const [committed, setCommitted] = createSignal("-");
export const [fieldEvents, setFieldEvents] = createSignal(0);
/** Selected date from the native DatePicker (ISO "YYYY-MM-DD" or ""). */
export const [pickedDate, setPickedDate] = createSignal("");

// --- scroll state ---
export const [scrollPct, setScrollPct] = createSignal(0);
export const [scrollTop, setScrollTop] = createSignal(0);
export const [scrollContent, setScrollContent] = createSignal(0);
export const [scrollEvents, setScrollEvents] = createSignal(0);
/** Outer scroller movements — set by the host's `scroll` event on the main
 *  ScrollArea. A nested-scroll regression counter. */
export const [outerScrolls, setOuterScrolls] = createSignal(0);

// --- canvas state ---
export const [bars, setBars] = createSignal<number[]>([38, 61, 27, 74, 52, 88, 43, 66]);
export const [cmdCount, setCmdCount] = createSignal(0);
export const [redraws, setRedraws] = createSignal(0);

// --- BOM state (all reads degrade when there is no BOM) ---
export const FRAME_TOTAL = 60;
export const [winLabel, setWinLabel] = createSignal(windowSize());
export const [uuidText, setUuidText] = createSignal(newUuid());
export const [frame, setFrame] = createSignal(0);
export const [frameRunning, setFrameRunning] = createSignal(false);
/** Host uptime when the app mounted — one datapoint for the perf row. */
export const mountMs = Math.round(perfNow());

// --- widget card state (Progress / Slider / Rating / Spinner) ---
export const [progress, setProgress] = createSignal(64);
export const [sliderVal, setSliderVal] = createSignal(30);
export const [sliderLog, setSliderLog] = createSignal("");
export const [stars, setStars] = createSignal(3);
export const [spinnerRun, setSpinnerRun] = createSignal(true);

// --- extended component card state (16 native widgets) ---
export const [taText, setTaText] = createSignal("多行文本内容\n第二行");
export const [comboVal, setComboVal] = createSignal("北京");
export const [colorVal, setColorVal] = createSignal("#3366cc");
export const [radioIdx, setRadioIdx] = createSignal(1);
export const [tabIdx, setTabIdx] = createSignal(0);
export const [page, setPage] = createSignal(1);
export const [crumbIdx, setCrumbIdx] = createSignal(0);
export const [collOpen, setCollOpen] = createSignal(false);
export const [alertVisible, setAlertVisible] = createSignal(true);
export const [extLog, setExtLog] = createSignal("");

// --- delegation card state ---
export const [delegLog, setDelegLog] = createSignal("");

// --- canvas chart state ---
export const [chartTone, setChartTone] = createSignal(0);

// --- shared option lists ---
export const ENVS = ["开发", "预发", "生产"];
export const REGIONS = ["上海", "北京", "深圳", "杭州"];

// --- scroll card log lines ---
export const LOG_ROWS = [
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

/** Which runtime is hosting this bundle (reported in the status bar). */
export const mode = engineName();
