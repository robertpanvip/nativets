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
// TextField
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

export function TextField(props: {
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
