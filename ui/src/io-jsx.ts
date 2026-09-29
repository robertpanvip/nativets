/**
 * io-jsx.ts — hyperscript + reactive layer: h/text/Show/For, signals
 * (createSignal/createEffect), and the root mount. Built on the protocol
 * core in ./io-core.
 */
import { Fragment, appendChild, clearEl, create, createEffect, sendHello, setEvents, setLineHandler, setStyle, setText, setValue, wireEvent } from "./io-core";
// scriptc 0.1.7: `export type X` is a TYPE-only export — pulling it through a
// value import list makes the runtime module check fail with SC4013
// ("does not provide an export named 'X'"). El/HostEvent/Props/Style and
// AnyComponent/Child are type-position-only here, so they ride `import type`.
import type { AnyComponent, Child, El, HostEvent, Props, Style } from "./io-core";

/**
 * Controlled native widgets (gpui-component migration): these tags are
 * rendered by the host as real components instead of plain divs:
 *
 *   - `checkbox` / `switch` — `value` is `"true"`/`"false"` (the same
 *     reactive `value` channel as `input`); toggling emits `change` whose
 *     payload carries the new state.
 *   - `button` — `text` is the label; emits `click`.
 *   - `select` — `value` is the selected option text; the host opens its own
 *     picker; picking emits `change` with the new option.
 *   - `date` — `value` is an ISO "YYYY-MM-DD"; picking emits `change`.
 *   - `progress` — display-only; `value` is 0..=100, `loading: 1` style
 *     flips the indeterminate animation.
 *   - `spinner` / `rating` — display widgets; rating emits `change` with the
 *     star count.
 *   - `slider` — `value` is the numeric thumb position; dragging emits
 *     `input` per tick and `change` on release.
 *
 * The frontend keeps ownership of the state (controlled, like `input`): the
 * event handler decides what the new state is, the setter re-runs the
 * `value` getter, and the host reflects it.
 */
export type NativeTag =
    | "checkbox"
    | "switch"
    | "button"
    | "select"
    | "date"
    | "progress"
    | "slider"
    | "spinner"
    | "rating";

/**
 * Hyperscript element factory — also the JSX factory (classic runtime):
 *
 *     <div style={{ padding: 16 }}>hi {name}</div>
 *         == h("div", { style: { padding: 16 } }, "hi ", name)
 *
 * style is the only presentation channel; components are called with their
 * props verbatim (children merged in).
 */
export function h(tag: string | AnyComponent, props: Props | null, ...children: Child[]): El {
    if (typeof tag === "function") {
        return callComponent(tag, props, children);
    }
    if (tag === Fragment) {
        const frag = create("div");
        setStyle(frag, { display: "contents" });
        appendChildren(frag, children);
        return frag;
    }
    const p: Props = props === null || props === undefined ? {} : props;
    // No String() on a union with object arms (SC1090) — pin the arm with a
    // cast, then convert the primitive.
    let textArg: string | undefined = undefined;
    if (p.text !== undefined && p.text !== null) textArg = String(p.text as string);
    // Native checkbox/switch carry their label through the create op's text
    // field — the host hands it to the gpui-component control (`label()`).
    if (textArg === undefined && p.label !== undefined && p.label !== null) {
        textArg = String(p.label as string);
    }
    let placeholderArg: string | undefined = undefined;
    if (p.placeholder !== undefined && p.placeholder !== null) placeholderArg = String(p.placeholder as string);
    // Native select: the option list rides the create op (fixed for the
    // node's lifetime — a changing list is a new select).
    let optionsArg: string[] | undefined = undefined;
    if (p.options !== undefined && p.options !== null) optionsArg = p.options;
    const e = create(tag as string, textArg, placeholderArg, optionsArg);
    if (p.style !== undefined && p.style !== null) setStyle(e, p.style);

    const kinds: string[] = [];
    if (p.onClick !== undefined) { wireEvent(e, "click", p.onClick); kinds.push("click"); }
    if (p.onMouseDown !== undefined) { wireEvent(e, "mousedown", p.onMouseDown); kinds.push("mousedown"); }
    if (p.onMouseUp !== undefined) { wireEvent(e, "mouseup", p.onMouseUp); kinds.push("mouseup"); }
    if (p.onDblClick !== undefined) { wireEvent(e, "dblclick", p.onDblClick); kinds.push("dblclick"); }
    if (p.onWheel !== undefined) { wireEvent(e, "wheel", p.onWheel); kinds.push("wheel"); }
    if (p.onInput !== undefined) { wireEvent(e, "input", p.onInput); kinds.push("input"); }
    if (p.onChange !== undefined) { wireEvent(e, "change", p.onChange); kinds.push("change"); }
    if (p.onScroll !== undefined) { wireEvent(e, "scroll", p.onScroll); kinds.push("scroll"); }
    if (p.onFocus !== undefined) { wireEvent(e, "focus", p.onFocus); kinds.push("focus"); }
    if (p.onBlur !== undefined) { wireEvent(e, "blur", p.onBlur); kinds.push("blur"); }
    if (kinds.length > 0) setEvents(e, kinds);

    if (p.value !== undefined && p.value !== null) applyValue(e, p.value);
    // Native checkbox/switch: `checked` getter drives the same setValue
    // channel, spelled "true"/"false" (see applyChecked).
    if (p.checked !== undefined && p.checked !== null) applyChecked(e, p.checked);
    appendChildren(e, children);
    return e;
}

/**
 * Bind a field value: a function is treated as a getter and re-pushed
 * whenever a signal it reads changes. Values ride the same `setValue`
 * channel spelled as strings (numbers are stringified at the call site or
 * by the kit wrappers — the wire has no number arm).
 *
 * scriptc 0.1.7: the runtime cannot tell a ()=>string slot from a ()=>number
 * slot apart — calling through the wrong arm traps (SC4013). With the getter
 * domain pinned to ONE arm (()=>string, see Props.value), a single cast is
 * exact; `typeof` narrows the primitive arms before they convert.
 *
 * The casts MUST live inside their branches. `const x = value as string`
 * written ahead of the checks lowers to an eager union_narrow that throws
 * ("a '() => string' value is not representable in the target union") the
 * moment a getter reaches this function — the union model has no
 * cast-then-test, only test-then-cast (same shape as `text()` below).
 */
function applyValue(e: El, value: string | number | (() => string)): void {
    if (typeof value === "string") {
        setValue(e, String(value as string));
        return;
    }
    if (typeof value === "number") {
        setValue(e, String(value as number));
        return;
    }
    // Getter arm — reached only when value is neither string nor number, so
    // the function-arm narrow cannot see a mismatched tag.
    const getter = value as (() => string);
    createEffect(() => {
        setValue(e, String(getter()));
    });
}

/**
 * Reactive checked state for the native `checkbox`/`switch` tags: the wire
 * carries the same `setValue` op as text fields, spelled `"true"`/`"false"`.
 */
function applyChecked(e: El, checked: () => boolean): void {
    createEffect(() => {
        setValue(e, checked() ? "true" : "false");
    });
}

/**
 * Mount a string child as a text node.
 *
 * NOTE: 不能写成 appendChild(e, create(...)) —— perry 0.5.1520 下该形态生成的
 * append op 会丢 id（node 正常）。先绑定局部变量。
 */
function mountText(parent: El, content: string): void {
    const t = create("text", content);
    appendChild(parent, t);
}

/**
 * Mount JSX children in order. Arrays are flattened at runtime; each leaf
 * arm is pinned with a cast before use (a `String()` on the whole `Child`
 * union — object arms included — is refused: SC1090).
 */
function appendChildren(parent: El, children: Child[]): void {
    for (let ci = 0; ci < children.length; ci++) {
        const c = children[ci];
        if (c === null || c === undefined || c === false || c === true) continue;
        if (typeof c === "string") {
            mountText(parent, c as string);
            continue;
        }
        if (typeof c === "number") {
            mountText(parent, String(c as number));
            continue;
        }
        if (isChildArray(c)) {
            appendChildren(parent, c as Child[]);
            continue;
        }
        appendChild(parent, c as El);
    }
}

/** Runtime shape test for the recursive Child union. scriptc：数组值被
 * 断言进记录臂（`(c as El).id`）会在运行时触发 SC4013，必须用
 * `Array.isArray`（探针验证可编译且语义正确）。 */
function isChildArray(c: Child | El | string | number): boolean {
    return Array.isArray(c);
}

/** Accepted text content: a literal, or a string getter (re-run on every
 * change). Number getters must wrap with `String()` at the call site —
 * scriptc refuses to call a union of functions whose return types differ
 * (SC2011), so the getter arm must stay homogeneous. */
export type TextSource = string | number | (() => string);

/** A text element whose content can be reactive. */
export function text(content: TextSource, style?: Style): El {
    // 数字 signal 同样收敛为 string（见 setText 注释）。scriptc 不认
    // `typeof x === "function"` 收敛，所以按 string/number 逐分支钉死：
    // 两个分支都不中即是 getter，钉进局部变量后再进 effect。
    let initial = "";
    let getter: (() => string) | (() => number) | null = null;
    if (typeof content === "string") {
        initial = content as string;
    } else if (typeof content === "number") {
        initial = String(content as number);
    } else {
        getter = content as (() => string);
    }
    const e = create("text", initial);
    if (style) setStyle(e, style);
    if (getter !== null) {
        createEffect(() => {
            setText(e, String(getter()));
        });
    }
    return e;
}

/** Conditional rendering: mounts `render()` while `cond()` is true. */
export function Show(cond: () => boolean, render: () => El): El {
    const container = create("div");
    // `display: contents` -> the host renders no box for this node.
    setStyle(container, { display: "contents" });
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

/**
 * Invoke a JSX component with its props plus the JSX children.
 *
 * No dynamic key-copy any more: `Record<string, any>` has no static
 * representation (SC2011), and the copy was never load-bearing — the JSX
 * transform hands `h()` a FRESH object literal per element, so writing
 * `children` in place is safe. (A hand-written `h(Component, sharedBag, …)`
 * would alias the bag; no call site does that.)
 */
function callComponent(fn: AnyComponent, props: Props | null, children: Child[]): El {
    const p: Props = props === null || props === undefined ? {} : props;
    if (children.length === 1) {
        p.children = children[0];
    } else if (children.length > 1) {
        p.children = children;
    }
    return fn(p);
}

/** Styles applied to the implicit root container (id 0). */
export function setRootStyle(style: Style): void {
    setStyle({ id: 0 }, style);
}

/**
 * Register a callback for host->frontend UI events. Runs once per event,
 * after the element (and, for `click`, ancestor) handlers — regardless of
 * `stopPropagation`. Diagnostics hook, not a delegation mechanism.
 */
export function onHostEvent(fn: (kind: string) => void): void {
    setLineHandler("event", function (msg: HostEvent): void {
        fn(msg.kind);
    });
}

// ---------------------------------------------------------------------------
// platform hooks — the ENTIRE contract the core needs from a host
//
// Two facts, both irreducible: *who am I* (status bar) and *heartbeat me*
// (carrying a timestamp, so time needs no second seam). Everything else —
// timers, Date, process, BOM — stays behind the platform's own module, which
// is how app/kit/canvas2d compile unchanged on all three backends.
// ---------------------------------------------------------------------------

/**
 * Engine label for the status bar. The QuickJS wiring in runtime.ts
 * overwrites it with the old `process.env` probe result; the scriptc entry
 * sets its own. The old IIFE in app.tsx probed `process` directly — SC1090
 * refuses that under scriptc, so the probe moved with the platform.
 */
let engineLabel: string = "perry-native";

export function setEngineLabel(label: string): void {
    engineLabel = label;
}

/** Status-bar engine name (never probed at the call site). */
export function engineName(): string {
    return engineLabel;
}

/**
 * Periodic-task registry. `setInterval` is refused by scriptc (SC4005) and
 * conceptually wrong there anyway — the host driver thread IS the heartbeat,
 * and a compiled-to-native graph has no timer queue to put one in. App code
 * registers via onPulse(); the platform drains the registry at its own rate:
 * QuickJS from a real interval (runtime.ts), scriptc from `gpts_tick`.
 *
 * The callback receives the platform's wall clock in ms (local midnight
 * based on dynamic engines, uptime on scriptc) — one seam, not two.
 */
class PulseEntry {
    fn: ((nowMs: number) => void) | null = null;
}

const pulses: PulseEntry[] = [];

/** Register a callback fired on every platform heartbeat. */
export function onPulse(fn: (nowMs: number) => void): void {
    const entry = new PulseEntry();
    entry.fn = fn;
    pulses.push(entry);
}

/** Fire every registered pulse callback in registration order. */
export function pumpPulses(nowMs: number): void {
    for (let i = 0; i < pulses.length; i++) {
        const p = pulses[i];
        if (p.fn !== null) {
            const f = p.fn;
            f(nowMs);
        }
    }
}

// ---------------------------------------------------------------------------
// root
// ---------------------------------------------------------------------------

/**
 * `app` may either append to `root` itself (`appendChild(root, ...)`, what
 * the demo does) or simply *return* the UI — both are supported, because
 * the second is the natural shape for a TSX entry point. A returned value
 * is mounted with the same normalisation JSX children get.
 */
export function createRoot(
    spec: { title?: string },
    app: (root: El) => Child | void
): void {
    const title = spec.title || "App";
    sendHello(title);
    const root: El = { id: 0 };
    const out: Child | void = app(root);
    if (out !== undefined && out !== null) {
        const mounted: Child[] = [out];
        appendChildren(root, mounted);
    }
}
