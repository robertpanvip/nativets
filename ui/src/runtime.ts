/**
 * nativets runtime — TypeScript side.
 *
 * A tiny Solid-style reactive runtime that emits element mutations to the
 * GPUI host, carrying the JSONL protocol over one of two interchangeable
 * transports (chosen by the host at runtime, see `writeLine` below):
 *   - injected host function (`__hostEmit`) when the engine is in-process
 *   - JSONL over stdio for out-of-process frontends
 *
 * Zero npm dependencies, conservative TS subset — Perry-compilable by design:
 *   - no try/catch  (PERRY_RS4GC refuses WinEH funclet pads on Windows, #7354)
 *   - no for...of   (lowers to iterator close = try/finally shape)
 *   - no Proxy / generators / structuredClone
 *
 *   createSignal / createEffect      fine-grained reactivity
 *   h(tag, props, ...children)       hyperscript (Solid-ish authoring)
 *   text(content, style)             reactive text node
 *   Show(cond, render) / For(items, render)   control flow
 *
 * Every state change becomes a minimal mutation batch, so only what actually
 * changed crosses the transport boundary.
 */

// ---------------------------------------------------------------------------
// transport
// ---------------------------------------------------------------------------

export const stats = {
    batchesSent: 0,
    opsSent: 0,
    eventsReceived: 0,
};

// 延迟追踪（PERRY_UI_TRACE=1 时启用，写 stderr，不干扰协议流）
const traceOn = typeof process !== "undefined" && process.env && process.env.PERRY_UI_TRACE === "1";
function traceLine(s: string): void {
    process.stderr.write(s + "\n");
}

let pendingOps: any[] = [];
let flushQueued = false;

function scheduleFlush(): void {
    if (!flushQueued) {
        flushQueued = true;
        Promise.resolve().then(flush);
    }
}

function flush(): void {
    flushQueued = false;
    if (pendingOps.length === 0) return;
    const ops = pendingOps;
    pendingOps = [];
    stats.batchesSent++;
    stats.opsSent += ops.length;
    if (traceOn) traceLine("FLUSH " + Date.now() + " n=" + ops.length);
    writeLine(JSON.stringify({ t: "batch", ops: ops }));
}

/**
 * 出站传输：宿主在**运行时**决定走哪条路，前端只做特性探测，因此同一个
 * bundle 同时适配两种传输（切传输不需要重新编译）。
 *
 *   __hostEmit 存在  → 宿主注入的原生函数（QuickJS `direct` 模式）：
 *                      batch 行直接进宿主的 ops 通道，没有管道、没有行缓冲
 *   __hostEmit 不存在 → JSONL over stdout（子进程 / Perry / `pipe` 模式）
 *
 * 用 `typeof` 探测而不是直接读：对未声明的全局名 `typeof` 永不抛错。
 */
const hostEmit: ((line: string) => void) | null = (function (): ((line: string) => void) | null {
    if (typeof globalThis === "undefined") return null;
    const g: any = globalThis;
    if (typeof g.__hostEmit === "function") return g.__hostEmit;
    return null;
})();

function writeLine(line: string): void {
    if (hostEmit !== null) {
        hostEmit(line);
        return;
    }
    process.stdout.write(line + "\n");
}

// --- inbound: host -> frontend (events) ---

const lineHandlers = new Map<string, (msg: any) => void>();

function handleLine(line: string): void {
    // NOTE: 无 try/catch —— Perry 的 RS4GC 在 Windows 上不支持 WinEH funclet
    // (#7354)。宿主只回传合法 JSONL，这里做廉价前置校验后直接解析。
    if (traceOn) traceLine("LINE_RECV " + Date.now() + " " + line.slice(0, 60));
    if (line.length < 2 || line.charAt(0) !== "{") return;
    const msg = JSON.parse(line);
    if (msg.t === "event") {
        if (traceOn) traceLine("EVT_RECV " + Date.now() + " feeder=" + (msg.ts === undefined ? -1 : msg.ts));
        // 先派发给具体元素的 handler，再通知全局监听（统计用）
        const fn = eventFns.get(msg.target + ":" + msg.kind);
        if (fn) fn();
        const g = lineHandlers.get("event");
        if (g) g(msg);
        return;
    }
    const h = lineHandlers.get(msg.t);
    if (h) h(msg);
}

let stdinBuffer = "";
function pumpStdin(chunk: any): void {
    if (traceOn) traceLine("STDIN_CHUNK " + Date.now() + " len=" + String(chunk).length);
    const text = typeof chunk === "string" ? chunk : String(chunk);
    stdinBuffer += text;
    let idx = stdinBuffer.indexOf("\n");
    while (idx >= 0) {
        const line = stdinBuffer.slice(0, idx).trim();
        stdinBuffer = stdinBuffer.slice(idx + 1);
        if (line.length > 0) handleLine(line);
        idx = stdinBuffer.indexOf("\n");
    }
}

process.stdin.setEncoding("utf8");
process.stdin.on("data", pumpStdin);
process.stdin.on("end", () => {
    process.exit(0);
});

// ---------------------------------------------------------------------------
// elements & mutations
// ---------------------------------------------------------------------------

export interface El {
    id: number;
}

let nextId = 1;
const eventFns = new Map<string, () => void>();

function create(tag: string, text?: string): El {
    const id = nextId++;
    const op: any = { op: "create", id: id, tag: tag };
    if (text !== undefined) op.text = text;
    pendingOps.push(op);
    scheduleFlush();
    return { id: id };
}

function setText(e: El, text: string): void {
    // NOTE: String() 收敛 —— 数字 signal（如 count()）会把 number 推进 op，
    // host 端 parse_op 对 setText 严格取字符串，number op 会被整条丢弃
    // （计数器文本空白的根因）。协议层保持"文本必须是字符串"约定。
    pendingOps.push({ op: "setText", id: e.id, text: String(text) });
    scheduleFlush();
}

function setStyle(e: El, style: Record<string, any>): void {
    pendingOps.push({ op: "setStyle", id: e.id, style: style });
    scheduleFlush();
}

function setClick(e: El, fn: () => void): void {
    eventFns.set(e.id + ":click", fn);
    pendingOps.push({ op: "setEvents", id: e.id, events: ["click"] });
    scheduleFlush();
}

/** Mount `child` under `parent`. */
export function appendChild(parent: El, child: El): void {
    pendingOps.push({ op: "append", id: child.id, parent: parent.id });
    scheduleFlush();
}

/** Remove an element and its whole subtree on the host. */
export function remove(e: El): void {
    eventFns.delete(e.id + ":click");
    pendingOps.push({ op: "remove", id: e.id });
    scheduleFlush();
}

/** Remove all children of an element on the host. */
function clearEl(e: El): void {
    pendingOps.push({ op: "clear", id: e.id });
    scheduleFlush();
}

// ---------------------------------------------------------------------------
// reactivity (fine-grained, Solid-style)
// ---------------------------------------------------------------------------

let activeEffect: (() => void) | null = null;

export function createSignal<T>(initial: T): [() => T, (v: T) => void] {
    let value = initial;
    const subs = new Set<() => void>();
    const read = (): T => {
        if (activeEffect !== null) subs.add(activeEffect);
        return value;
    };
    const write = (v: T): void => {
        value = v;
        const snapshot: (() => void)[] = [];
        subs.forEach((s) => snapshot.push(s));
        for (let i = 0; i < snapshot.length; i++) snapshot[i]();
    };
    return [read, write];
}

export function createEffect(fn: () => void): void {
    const run = (): void => {
        activeEffect = run;
        fn();
        activeEffect = null;
    };
    run();
}

// ---------------------------------------------------------------------------
// authoring helpers
// ---------------------------------------------------------------------------

/**
 * Hyperscript element factory.
 * props: style keys directly on the object, plus optional `onClick` and
 * `text` (for leaf text nodes). children: El | string | number.
 */
export function h(tag: string, props: Record<string, any> | null, ...children: any[]): El {
    const e = create(
        tag,
        props && props.text !== undefined ? String(props.text) : undefined
    );
    if (props) {
        const rest: Record<string, any> = {};
        let hasStyle = false;
        const keys = Object.keys(props);
        for (let ki = 0; ki < keys.length; ki++) {
            const k = keys[ki];
            if (k === "onClick") {
                setClick(e, props[k]);
            } else if (k === "text") {
                // already handled via create
            } else {
                rest[k] = props[k];
                hasStyle = true;
            }
        }
        if (hasStyle) setStyle(e, rest);
    }
    for (let ci = 0; ci < children.length; ci++) {
        const c = children[ci];
        if (c === null || c === undefined || c === false) continue;
        if (typeof c === "string" || typeof c === "number") {
            // NOTE: 不能写成 appendChild(e, create(...)) —— perry 0.5.1520 下
            // 该形态生成的 append op 会丢 id（node 正常）。先绑定局部变量。
            const t = create("text", String(c));
            appendChild(e, t);
        } else {
            appendChild(e, c as El);
        }
    }
    return e;
}

/** A text element whose content can be reactive. */
export function text(content: (() => string) | string, style?: Record<string, any>): El {
    // 数字 signal 同样收敛为 string（见 setText 注释），否则 setText op
    // 携带 JSON number，host parse_op 丢弃 → 文本空白（计数器 bug 根因）。
    const e = create("text", typeof content === "function" ? "" : String(content));
    if (style) setStyle(e, style);
    if (typeof content === "function") {
        createEffect(() => {
            setText(e, String(content()));
        });
    }
    return e;
}

/** Conditional rendering: mounts `render()` while `cond()` is true. */
export function Show(cond: () => boolean, render: () => El): El {
    const container = create("div");
    setStyle(container, { flexDirection: "column" });
    createEffect(() => {
        clearEl(container);
        if (cond()) appendChild(container, render());
    });
    return container;
}

/** List rendering: rebuilds children whenever `items()` changes. */
export function For<T>(items: () => T[], render: (item: T, index: number) => El): El {
    const container = create("div");
    // host 默认 flex_row，容器显式声明 column（列表语义）
    setStyle(container, { flexDirection: "column" });
    createEffect(() => {
        clearEl(container);
        const arr = items();
        for (let i = 0; i < arr.length; i++) {
            appendChild(container, render(arr[i], i));
        }
    });
    return container;
}

/** Styles applied to the implicit root container (id 0). */
export function setRootStyle(style: Record<string, any>): void {
    setStyle({ id: 0 }, style);
}

/** Register a callback for host->frontend UI events. */
export function onHostEvent(fn: (kind: string) => void): void {
    lineHandlers.set("event", (msg: any) => {
        stats.eventsReceived++;
        fn(msg.kind);
    });
}

// ---------------------------------------------------------------------------
// I/O 心跳（perry 原生运行时必需）
// ---------------------------------------------------------------------------
// perry 0.5.1520 的 JS 事件循环在空闲时以 ~500ms 量子轮询 stdio I/O，
// 实测 stdin 事件送达延迟 avg 255ms / max 504ms（node 为 0.1ms），
// 表现为点击回传明显丢帧。10ms 心跳强制事件循环高频运转，I/O 轮询
// 随之加密。node 下该定时器无副作用（空转开销可忽略）。
let heartbeatTicks = 0;
setInterval(function () {
    heartbeatTicks = heartbeatTicks + 1;
}, 10);

// ---------------------------------------------------------------------------
// root
// ---------------------------------------------------------------------------

export function createRoot(
    spec: { title?: string },
    app: (root: El) => void
): void {
    writeLine(
        JSON.stringify({
            t: "hello",
            proto: 1,
            title: spec.title || "App",
        })
    );
    const root: El = { id: 0 };
    app(root);
}
