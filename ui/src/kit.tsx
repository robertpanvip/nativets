/**
 * Component kit — the interactive controls, in the same style as everything
 * else: plain elements plus fine-grained reactivity.
 *
 * Two rules shape this file:
 *
 * 1. **A component body runs once.** There is no re-render pass, so anything
 *    that changes over time is either a reactive `text()` / `Show()` / `For()`,
 *    or an element whose style an effect re-writes (`createEffect(() =>
 *    setStyle(el, …))`). That is the whole "how do I restyle on state change"
 *    story, and it needs no diffing.
 * 2. **State lives where it belongs.** A radio's selection, a dropdown's open
 *    flag and a canvas's data are frontend state; a text field's *caret* is not
 *    (only the host knows what the user typed) and neither is a scroll offset
 *    (the host owns the layout). The controls below therefore look redundant
 *    only if you expect them to own everything.
 *
 * The host side of the three non-obvious ones:
 *   `TextField`  → `tag: "input"`, host paints the text + caret and returns
 *                  `input` / `change` / `focus` / `blur` events
 *   `ScrollArea` → `overflow: "scroll"`, host adds a scroll handle and paints
 *                  the scrollbar over the content, then reports `scroll`
 *   `CanvasView` → `tag: "canvas"`, host walks the display list
 */

import { createEffect, createSignal, For, h, mergeStyleInto, setStyle, Show, text } from "./io";
import type { Child, El, HostEvent, Style } from "./io";

/**
 * scriptc workaround (same shape as the io registries): a closure may not
 * CALL a callable stored in a plain props record (`props.onInput(x)` is
 * dynamic — SC2011/SC1090), but a function-typed FIELD on a class instance
 * is whitelisted. So a component copies an incoming callback into a ref
 * object, and every call site binds it to a local first.
 */
class StrFnRef { fn: ((v: string) => void) | null = null; }
class CtxFnRef { fn: ((ctx: Ctx) => void) | null = null; }
class PresentFnRef { fn: ((cmds: number, redraws: number) => void) | null = null; }
import { createCanvas } from "./canvas2d";
import type { Ctx } from "./canvas2d";
import { C } from "./theme";

// `h` is referenced by the JSX transform itself, not by hand-written calls.

// ---------------------------------------------------------------------------
// small shared pieces
// ---------------------------------------------------------------------------

const ROW: Style = { flexDirection: "row", alignItems: "center", gap: 8 };

function ControlLabel(label: string): El {
    return text(label, { fontSize: 11, color: C.textMuted });
}

// ---------------------------------------------------------------------------
// Input / TextField
// ---------------------------------------------------------------------------

/** Border color is the only thing focus changes, and it is driven by the host's
 *  own `focus`/`blur` events — the frontend has no other way to know. */
function fieldStyle(focused: boolean): Style {
    return {
        height: 34,
        width: "full",
        paddingX: 10,
        paddingY: 6,
        borderRadius: 8,
        fontSize: 13,
        background: C.bg,
        color: C.textPrimary,
        borderWidth: 1,
        borderColor: focused ? C.accent : C.border,
    };
}

export function Input(props: {
    value: () => string;
    onInput: (v: string) => void;
    onChange?: (v: string) => void;
    placeholder?: string;
    label?: string;
    width?: number;
}): El {
    const [focused, setFocused] = createSignal(false);
    // Copy the incoming callbacks into class refs before any closure captures
    // them (scriptc: closures may not call record function fields).
    const inRef = new StrFnRef();
    inRef.fn = props.onInput;
    const chRef = new StrFnRef();
    if (props.onChange !== undefined) chRef.fn = props.onChange;
    const box = (
        <input
            style={fieldStyle(false)}
            placeholder={props.placeholder === undefined ? "" : props.placeholder}
            value={props.value}
            onInput={(ev: HostEvent) => {
                const f = inRef.fn;
                if (f !== null) f(String(ev.value));
            }}
            onChange={(ev: HostEvent) => {
                const f = chRef.fn;
                if (f !== null) f(String(ev.value));
            }}
            onFocus={() => setFocused(true)}
            onBlur={() => setFocused(false)}
        />
    );
    createEffect(() => setStyle(box, fieldStyle(focused())));
    if (props.label === undefined) return box;
    const col = (
        <div style={{ flexDirection: "column", gap: 5, width: props.width === undefined ? "full" : props.width }}>
            {ControlLabel(props.label)}
            {box}
        </div>
    );
    return col;
}

// `TextField` is kept as a deprecated alias so existing call sites keep working.
export const TextField = Input;

// ---------------------------------------------------------------------------
// Button (native)
// ---------------------------------------------------------------------------
// Native button: the host renders this tag as a real gpui-component `Button`
// (see main.rs `build_native`). The label is emitted as a text child, which the
// host gathers and passes to the component — JSX `<button>label</button>`
// mounts a child `text` node, so the tag's own text field stays empty.

export function Button(props: {
    onClick: () => void;
    label?: string;
    children?: Child;
    style?: Style;
}): El {
    const style: Style = {
        height: 32,
        paddingX: 14,
        borderRadius: 8,
        fontSize: 13,
        background: C.accent,
        color: "#ffffff",
    };
    if (props.style !== undefined) mergeStyleInto(style, props.style);
    const clickRef = new CtxFnRef();
    clickRef.fn = () => props.onClick();
    const inner = props.label !== undefined ? text(props.label, { fontSize: 13 }) : props.children;
    return (
        <button
            style={style}
            onClick={(ev: HostEvent) => {
                const f = clickRef.fn;
                if (f !== null) f(undefined as unknown as Ctx);
            }}
        >
            {inner}
        </button>
    );
}

// ---------------------------------------------------------------------------
// DatePicker (native)
// ---------------------------------------------------------------------------
// Native date picker: the host renders this tag as a real gpui-component
// `time::DatePicker` (see main.rs `build_native`). Controlled like `input` —
// `value` is the selected date as an ISO string ("YYYY-MM-DD" or ""), picking
// emits `change` with the new date, and the app's `onChange` decides what to
// keep and pushes it back via `setValue`.

export function DatePicker(props: {
    value: () => string;
    onChange: (v: string) => void;
    placeholder?: string;
    style?: Style;
}): El {
    const chgRef = new StrFnRef();
    chgRef.fn = props.onChange;
    return h("date", {
        style: props.style,
        value: props.value,
        placeholder: props.placeholder,
        onChange: (ev: HostEvent) => {
            const f = chgRef.fn;
            if (f !== null) f(String(ev.value));
        },
    });
}


// ---------------------------------------------------------------------------
// Radio / Checkbox / Switch
// ---------------------------------------------------------------------------

function ringStyle(selected: boolean): Style {
    return {
        width: 16,
        height: 16,
        borderRadius: "full",
        background: C.cardAlt,
        borderWidth: 1,
        borderColor: selected ? C.accent : C.border,
        alignItems: "center",
        justifyContent: "center",
    };
}

export function Radio(props: { label: string; selected: () => boolean; onSelect: () => void }): El {
    const dot = <div style={{ width: 8, height: 8, borderRadius: "full", background: C.accent }} />;
    // `Show` mounts/unmounts the dot; the ring's own color is re-styled by the
    // effect below. Two mechanisms, one state — no reconciler needed.
    const ring = <div style={ringStyle(false)}>{Show(props.selected, () => dot)}</div>;
    createEffect(() => setStyle(ring, ringStyle(props.selected())));
    const row = (
        <div style={ROW} onClick={props.onSelect}>
            {ring}
            {text(props.label, { fontSize: 13, color: C.textPrimary })}
        </div>
    );
    return row;
}

export function RadioGroup(props: {
    options: string[];
    value: () => string;
    onChange: (v: string) => void;
    direction?: "row" | "column";
}): El {
    const column = props.direction === "column";
    const selRef = new StrFnRef();
    selRef.fn = props.onChange;
    // `For` builds its own container (a column, the list default), so the
    // direction is applied to *that* element — an extra wrapper would just be a
    // second box with the same job.
    const list = For(
        () => props.options,
        (opt) => <Radio label={opt} selected={() => props.value() === opt} onSelect={() => {
            const f = selRef.fn;
            if (f !== null) f(opt);
        }} />
    );
    setStyle(list, {
        flexDirection: column ? "column" : "row",
        gap: column ? 8 : 16,
        alignItems: column ? "start" : "center",
    });
    return list;
}

/**
 * Native checkbox: the host renders this tag as a real gpui-component
 * `Checkbox` (see main.rs `build_native`). Controlled like `input` — the
 * `checked` getter drives a `setValue` op ("true"/"false"), and the host's
 * toggle emits `change` whose payload carries the new state; `onToggle`
 * decides what to keep. The `label` is host-rendered text next to the box.
 */
export function Checkbox(props: { label: string; checked: () => boolean; onToggle: () => void }): El {
    const chgRef = new StrFnRef();
    chgRef.fn = (_v: string) => props.onToggle();
    return h("checkbox", {
        label: props.label,
        checked: props.checked,
        onChange: (ev: HostEvent) => {
            const f = chgRef.fn;
            if (f !== null) f(String(ev.value));
        },
    });
}

/** Native switch — same controlled contract as `Checkbox` (see above). */
export function Switch(props: { checked: () => boolean; onToggle: () => void; label?: string }): El {
    const chgRef = new StrFnRef();
    chgRef.fn = (_v: string) => props.onToggle();
    return h("switch", {
        label: props.label,
        checked: props.checked,
        onChange: (ev: HostEvent) => {
            const f = chgRef.fn;
            if (f !== null) f(String(ev.value));
        },
    });
}

// ---------------------------------------------------------------------------
// Select (dropdown)
// ---------------------------------------------------------------------------
// Native select: the host renders a real gpui-component `Select`-style
// picker (see main.rs `build_native`). Controlled like `input` — `value` is
// the selected option text, picking emits `change` with the new option, and
// the app's `onChange` decides what to keep.

export function Select(props: {
    options: string[];
    value: () => string;
    onChange: (v: string) => void;
    width?: number;
}): El {
    const width = props.width === undefined ? 170 : props.width;
    const chgRef = new StrFnRef();
    chgRef.fn = props.onChange;
    return h("select", {
        style: { width: width },
        value: props.value,
        options: props.options,
        onChange: (ev: HostEvent) => {
            const f = chgRef.fn;
            if (f !== null) f(String(ev.value));
        },
    });
}

// ---------------------------------------------------------------------------
// Progress / Spinner / Rating / Slider (native)
// ---------------------------------------------------------------------------
// The remaining native gpui-component widgets (see main.rs `build_native`).
// Progress and Slider are controlled: the `value` getter drives a `setValue`
// op and the widget's events echo back. Spinner is a pure animation; Rating
// keeps its own star state host-side and emits `change`.
//
// Scale-ish keys (`min` / `max` / `step` / `loading`) ride the *style* map —
// that is the one bag the protocol guarantees reaches the node, and the host
// reads them off `node.style`.

/** Determinate / indeterminate progress bar. `value` is 0..=100. */
export function Progress(props: {
    value: () => number;
    loading?: boolean;
    height?: number;
    style?: Style;
}): El {
    const style: Style = {};
    if (props.height !== undefined) style.height = props.height;
    if (props.loading === true) style.loading = 1;
    if (props.style !== undefined) mergeStyleInto(style, props.style);
    return h("progress", { style: style, value: props.value });
}

/** A cycling loading spinner (`fontSize` scales the icon). */
export function Spinner(props: { size?: number; style?: Style }): El {
    const style: Style = {};
    if (props.size !== undefined) style.fontSize = props.size;
    if (props.style !== undefined) mergeStyleInto(style, props.style);
    return h("spinner", { style: style });
}

/** Star rating; a click emits `change` with the new star count. */
export function Rating(props: {
    value: () => number;
    onChange?: (v: number) => void;
    max?: number;
    style?: Style;
}): El {
    const chgRef = new StrFnRef();
    if (props.onChange !== undefined) {
        chgRef.fn = (v: string) => {
            if (props.onChange !== undefined) props.onChange(parseFloat(v));
        };
    }
    const style: Style = {};
    if (props.max !== undefined) style.max = props.max;
    if (props.style !== undefined) mergeStyleInto(style, props.style);
    return h("rating", {
        style: style,
        value: props.value,
        onChange: (ev: HostEvent) => {
            const f = chgRef.fn;
            if (f !== null) f(String(ev.value));
        },
    });
}

/**
 * A drag slider bound to a scale. min/max/step are creation-time (a
 * different scale is a new slider, like the select option list). Dragging
 * emits `input` per tick and `change` on release; the controlled `value`
 * getter pushes the position back down.
 */
export function Slider(props: {
    value: () => number;
    onInput?: (v: number) => void;
    onChange?: (v: number) => void;
    min?: number;
    max?: number;
    step?: number;
    width?: number;
    style?: Style;
}): El {
    const inRef = new StrFnRef();
    if (props.onInput !== undefined) {
        inRef.fn = (v: string) => {
            if (props.onInput !== undefined) props.onInput(parseFloat(v));
        };
    }
    const chgRef = new StrFnRef();
    if (props.onChange !== undefined) {
        chgRef.fn = (v: string) => {
            if (props.onChange !== undefined) props.onChange(parseFloat(v));
        };
    }
    const style: Style = {};
    if (props.width !== undefined) style.width = props.width;
    if (props.min !== undefined) style.min = props.min;
    if (props.max !== undefined) style.max = props.max;
    if (props.step !== undefined) style.step = props.step;
    if (props.style !== undefined) mergeStyleInto(style, props.style);
    const onNum = (ev: HostEvent) => String(ev.value);
    return h("slider", {
        style: style,
        value: props.value,
        onInput: (ev: HostEvent) => {
            const f = inRef.fn;
            if (f !== null) f(onNum(ev));
        },
        onChange: (ev: HostEvent) => {
            const f = chgRef.fn;
            if (f !== null) f(onNum(ev));
        },
    });
}

// ---------------------------------------------------------------------------
// Extended gpui-component widgets (native bridge)
// ---------------------------------------------------------------------------
// One wrapper per new native tag registered in `main.rs::build_native`:
//   textarea / combobox / colorpicker / radio / tabs / pagination / breadcrumb /
//   alert / badge / tag / avatar / separator / skeleton / label / link /
//   collapsible
//
// The controlled contract is the same as the other native widgets: the frontend
// owns the value and pushes it via `setValue`; user interaction echoes back as
// an event the `onChange` / `onInput` / `onClick` / `onClose` callback receives.
// Callbacks are copied into class refs before any closure captures them so the
// same source compiles under scriptc (see the note at the top of this file).

/** Multi-line editor. Controlled like `Input`: `value` is the text; edits echo
 *  `input` (per keystroke) and `change` (on Enter). */
export function Textarea(props: {
    value: () => string;
    onInput?: (v: string) => void;
    onChange?: (v: string) => void;
    height?: number;
    style?: Style;
}): El {
    const inRef = new StrFnRef();
    if (props.onInput !== undefined) inRef.fn = props.onInput;
    const chRef = new StrFnRef();
    if (props.onChange !== undefined) chRef.fn = props.onChange;
    const style: Style = { width: "full" };
    if (props.height !== undefined) style.height = props.height;
    if (props.style !== undefined) mergeStyleInto(style, props.style);
    return h("textarea", {
        style: style,
        value: props.value,
        onInput: (ev: HostEvent) => {
            const f = inRef.fn;
            if (f !== null) f(String(ev.value));
        },
        onChange: (ev: HostEvent) => {
            const f = chRef.fn;
            if (f !== null) f(String(ev.value));
        },
    });
}

/** Searchable single-select. `value` is the chosen option text; picking echoes
 *  `change` with the option text. */
export function Combobox(props: {
    options: string[];
    value: () => string;
    onChange?: (v: string) => void;
    width?: number;
    style?: Style;
}): El {
    const chgRef = new StrFnRef();
    if (props.onChange !== undefined) chgRef.fn = props.onChange;
    const style: Style = {};
    if (props.width !== undefined) style.width = props.width;
    if (props.style !== undefined) mergeStyleInto(style, props.style);
    return h("combobox", {
        style: style,
        value: props.value,
        options: props.options,
        onChange: (ev: HostEvent) => {
            const f = chgRef.fn;
            if (f !== null) f(String(ev.value));
        },
    });
}

/** Colour picker. `value` is `#rrggbb`; changing echoes `change` with the hex. */
export function ColorPicker(props: {
    value: () => string;
    onChange?: (v: string) => void;
    style?: Style;
}): El {
    const chgRef = new StrFnRef();
    if (props.onChange !== undefined) chgRef.fn = props.onChange;
    const style: Style = {};
    if (props.style !== undefined) mergeStyleInto(style, props.style);
    return h("colorpicker", {
        style: style,
        value: props.value,
        onChange: (ev: HostEvent) => {
            const f = chgRef.fn;
            if (f !== null) f(String(ev.value));
        },
    });
}

/** Native radio group. `value` is the selected index; a click echoes `change`
 *  with the chosen index (0-based). */
export function RadioNative(props: {
    options: string[];
    value: () => number;
    onChange?: (index: number) => void;
    direction?: "row" | "column";
}): El {
    const selRef = new StrFnRef();
    if (props.onChange !== undefined) {
        selRef.fn = (v: string) => props.onChange!(parseInt(v, 10));
    }
    const style: Style = {
        flexDirection: props.direction === "column" ? "column" : "row",
        gap: 16,
        alignItems: "center",
    };
    return h("radio", {
        style: style,
        value: () => String(props.value()),
        options: props.options,
        onChange: (ev: HostEvent) => {
            const f = selRef.fn;
            if (f !== null) f(String(ev.value));
        },
    });
}

/** Tab bar. `value` is the selected index; a click echoes `change` with the
 *  chosen index (0-based). */
export function TabsNative(props: {
    options: string[];
    value: () => number;
    onChange?: (index: number) => void;
}): El {
    const selRef = new StrFnRef();
    if (props.onChange !== undefined) {
        selRef.fn = (v: string) => props.onChange!(parseInt(v, 10));
    }
    return h("tabs", {
        value: () => String(props.value()),
        options: props.options,
        onChange: (ev: HostEvent) => {
            const f = selRef.fn;
            if (f !== null) f(String(ev.value));
        },
    });
}

/** Pager. `total` is the page count; `value` is the current 1-based page; a
 *  click echoes `change` with the target page. */
export function PaginationNative(props: {
    total: number;
    value: () => number;
    onChange?: (page: number) => void;
}): El {
    const selRef = new StrFnRef();
    if (props.onChange !== undefined) {
        selRef.fn = (v: string) => props.onChange!(parseInt(v, 10));
    }
    const style: Style = {};
    style.total = props.total;
    return h("pagination", {
        style: style,
        value: () => String(props.value()),
        onChange: (ev: HostEvent) => {
            const f = selRef.fn;
            if (f !== null) f(String(ev.value));
        },
    });
}

/** Breadcrumb trail. `options` are the crumb labels; a click echoes `change`
 *  with the clicked crumb index (0-based). */
export function BreadcrumbNative(props: {
    options: string[];
    onChange?: (index: number) => void;
}): El {
    const selRef = new StrFnRef();
    if (props.onChange !== undefined) {
        selRef.fn = (v: string) => props.onChange!(parseInt(v, 10));
    }
    return h("breadcrumb", {
        options: props.options,
        onChange: (ev: HostEvent) => {
            const f = selRef.fn;
            if (f !== null) f(String(ev.value));
        },
    });
}

/** Dismissable banner. `text` is the message; `onClose` fires when the dismiss
 *  button is clicked (only wired when provided). */
export function AlertNative(props: {
    text: string;
    onClose?: () => void;
    style?: Style;
}): El {
    const closeRef = new CtxFnRef();
    if (props.onClose !== undefined) closeRef.fn = () => props.onClose!();
    const style: Style = {};
    if (props.style !== undefined) mergeStyleInto(style, props.style);
    return h("alert", {
        style: style,
        text: props.text,
        onClose: () => {
            const f = closeRef.fn;
            if (f !== null) f(undefined as unknown as Ctx);
        },
    });
}

/** Inline badge wrapping arbitrary content. */
export function Badge(props: { children?: Child; style?: Style }): El {
    const style: Style = {};
    if (props.style !== undefined) mergeStyleInto(style, props.style);
    return h("badge", { style: style }, props.children);
}

/** Small status tag. `variant` is one of default/secondary/danger/success/
 *  warning/info (drives the host's `TagVariant`). */
export function Tag(props: { variant?: string; children?: Child; style?: Style }): El {
    const style: Style = {};
    if (props.variant !== undefined) style.variant = props.variant;
    if (props.style !== undefined) mergeStyleInto(style, props.style);
    return h("tag", { style: style }, props.children);
}

/** User avatar; `name` is shown as initials. */
export function Avatar(props: { name: string; style?: Style }): El {
    const style: Style = {};
    if (props.style !== undefined) mergeStyleInto(style, props.style);
    return h("avatar", { style: style, text: props.name });
}

/** Layout divider. `orientation` is "horizontal" | "vertical". */
export function Separator(props: { orientation?: "horizontal" | "vertical"; style?: Style }): El {
    const style: Style = {};
    if (props.orientation !== undefined) style.orientation = props.orientation;
    if (props.style !== undefined) mergeStyleInto(style, props.style);
    return h("separator", { style: style });
}

/** Loading placeholder bar (host paints a shimmer). */
export function Skeleton(props: { style?: Style }): El {
    const style: Style = { width: "full", height: 14 };
    if (props.style !== undefined) mergeStyleInto(style, props.style);
    return h("skeleton", { style: style });
}

/** Static text label (a real gpui-component `Label`). */
export function Label(props: { text: string; style?: Style }): El {
    const style: Style = {};
    if (props.style !== undefined) mergeStyleInto(style, props.style);
    return h("label", { style: style, text: props.text });
}

/** Hyperlink. `href` is informational (no navigation happens); `onClick` fires
 *  when clicked (only wired when provided). */
export function Link(props: {
    href?: string;
    children?: Child;
    onClick?: () => void;
    style?: Style;
}): El {
    const clickRef = new CtxFnRef();
    if (props.onClick !== undefined) clickRef.fn = () => props.onClick!();
    const style: Style = {};
    if (props.href !== undefined) style.href = props.href;
    if (props.style !== undefined) mergeStyleInto(style, props.style);
    return h(
        "link",
        {
            style: style,
            onClick: () => {
                const f = clickRef.fn;
                if (f !== null) f(undefined as unknown as Ctx);
            },
        },
        props.children,
    );
}

/** Expandable section. `open` is the controlled state; the children are the
 *  revealed content. */
export function Collapsible(props: {
    open: () => boolean;
    children?: Child;
    style?: Style;
}): El {
    const style: Style = {};
    if (props.style !== undefined) mergeStyleInto(style, props.style);
    return h(
        "collapsible",
        {
            style: style,
            value: () => (props.open() ? "true" : "false"),
        },
        props.children,
    );
}

// ---------------------------------------------------------------------------
// ScrollArea
// ---------------------------------------------------------------------------

/**
 * A scroll container. `overflow: "scroll"` is all the frontend says: the host
 * creates the scroll handle, clips the content, paints the scrollbar over the
 * right edge and reports `scroll` events back.
 *
 * **Exactly one of `height` / `grow` is needed** — a scroll area with an
 * unbounded height has nothing to scroll, exactly like CSS. `height` fixes it in
 * px; `grow` fills whatever sized parent it sits in (a flex column whose own
 * height is bounded).
 */
export function ScrollArea(props: {
    height?: number;
    grow?: boolean;
    children?: Child;
    onScroll?: (ev: HostEvent) => void;
    style?: Style;
}): El {
    const style: Style = {
        flexDirection: "column",
        gap: 6,
        padding: 8,
        overflow: "scroll",
        background: C.cardAlt,
        borderRadius: 10,
        borderWidth: 1,
        borderColor: C.border,
    };
    if (props.height !== undefined) style.height = props.height;
    if (props.grow === true) style.grow = 1;
    if (props.style !== undefined) mergeStyleInto(style, props.style);
    const box = (
        <div style={style} onScroll={props.onScroll}>
            {props.children}
        </div>
    );
    return box;
}

// ---------------------------------------------------------------------------
// CanvasView
// ---------------------------------------------------------------------------

/**
 * A reactive canvas: `draw` re-runs whenever a signal it reads changes.
 *
 * That works because the whole draw call happens inside a `createEffect` — the
 * runtime tracks signal reads per effect, so "redraw when the data changes" needs
 * no dependency array and no invalidation bookkeeping. It is the same mechanism
 * that makes `text(() => …)` and `For(() => …)` update.
 *
 * `onPresent` is called *from inside that effect*, so it must not read a signal
 * the same effect writes (that is a self-retriggering loop). It is handed the
 * command count and the running redraw count instead — the counter here is a
 * plain closure variable precisely so callers never have to think about it.
 */
export function CanvasView(props: {
    width: number;
    height: number;
    draw: (ctx: Ctx) => void;
    style?: Style;
    /** `(commands in this frame, redraws so far)`, write-only from inside. */
    onPresent?: (cmds: number, redraws: number) => void;
}): El {
    const cv = createCanvas(props.width, props.height, props.style);
    let redraws = 0;
    // Draw/onPresent go through class refs — scriptc may not call a function
    // held in a props record from inside a closure.
    const drawRef = new CtxFnRef();
    drawRef.fn = props.draw;
    const presentRef = new PresentFnRef();
    if (props.onPresent !== undefined) presentRef.fn = props.onPresent;
    createEffect(() => {
        cv.ctx.reset();
        const draw = drawRef.fn;
        if (draw !== null) draw(cv.ctx);
        cv.present();
        redraws = redraws + 1;
        const notify = presentRef.fn;
        if (notify !== null) notify(cv.count(), redraws);
    });
    return cv.el;
}
