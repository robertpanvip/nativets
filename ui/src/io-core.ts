/**
 * io.ts — scriptc-compatible protocol core (the static-subset half of the
 * old runtime transport).
 *
 * Design (validated by probes #2/#3, ui/build/sc2/):
 *
 *   - NO `typeof x !== "undefined"` gates (SC1090 refuses typeof on any
 *     statically-typed value). Instead: **injectable sink**. The platform
 *     entry (QuickJS bootstrap / scriptc host wrapper) assigns `setSink()`
 *     before `createRoot()`; until then outbound lines are counted+dropped.
 *
 *   - NO function-valued Maps (SC2009). Handler registries are classes with
 *     a function-typed FIELD — class instances sit inside scriptc's Map
 *     value whitelist, and closure capture into a field is probe-verified.
 *
 *   - NO Promise/microtask flush (SC4005). Flush is *synchronous*: the
 *     scriptc FFI host drives everything from its own thread anyway, and
 *     batching still groups whole mount sequences into one line.
 *
 *   - NO process.* anywhere. The old trace/stderr/stdin-facade roles are
 *     gone: transport is always an injected sink (QuickJS injects
 *     `__hostEmit`; the scriptc host polls `gpts_poll`).
 *
 * The QuickJS path keeps identical semantics — bootstrap assigns the real
 * `__hostEmit` as the sink at startup. io-core is transport-agnostic.
 */

// ---------------------------------------------------------------------------
// outbound sink
// ---------------------------------------------------------------------------

/**
 * The outbound line channel. Platform entries MUST call setSink() before
 * createRoot() — until then lines go nowhere (counted + dropped).
 */
let sink: ((line: string) => void) | null = null;

export function setSink(fn: (line: string) => void): void {
    sink = fn;
}

export function hasSink(): boolean {
    return sink !== null;
}

export const stats = {
    batchesSent: 0,
    opsSent: 0,
    eventsReceived: 0,
};

// ---------------------------------------------------------------------------
// protocol value shapes
// ---------------------------------------------------------------------------

/**
 * One protocol operation. The `op` field is what the host switches on; the
 * optional payloads mirror the op shapes (create / setText-family /
 * setStyle / setCanvas / append / remove / clear / setEvents / setTitle).
 * One flat interface instead of a union — the host is the only producer
 * side consumer and switches on `op` anyway (same rationale as HostEvent).
 */
export interface Op {
    op: string;
    id?: number;
    parent?: number;
    tag?: string;
    text?: string;
    value?: string;
    placeholder?: string;
    events?: string[];
    title?: string;
    style?: Style;
    cmds?: Cmd[];
    /** Native `select`: the option list, fixed at creation. */
    options?: string[];
}

/**
 * One canvas display-list command — the exact wire shape host/src/draw.rs
 * parses: `k` selects the primitive (`rect` / `ellipse` / `line` / `poly` /
 * `text`) and an absent key means "channel not set" (draw.rs::parse_cmd).
 *
 * Concrete optional fields instead of an index signature: `[key: string]:
 * any` has no static representation and is refused outright (SC2011). The
 * field set below is the closed union of everything draw.rs reads.
 */
export interface Cmd {
    k?: string;
    x?: number;
    y?: number;
    w?: number;
    h?: number;
    r?: number;
    cx?: number;
    cy?: number;
    rx?: number;
    ry?: number;
    x1?: number;
    y1?: number;
    x2?: number;
    y2?: number;
    pts?: number[][];
    close?: boolean;
    t?: string;
    size?: number;
    weight?: string;
    fill?: string;
    stroke?: string;
    line?: number;
    a?: number;
}

/**
 * Styles applied to a node. Keys are the flat names the host understands —
 * the closed set dispatched in host/src/main.rs::apply_style. Concrete
 * optional fields instead of `Record<string, any>`: scriptc lowers literal
 * assignment against the interface (SC2011 refused the open record), and
 * this documents the real protocol surface.
 */
export interface Style {
    flexDirection?: string;
    direction?: string;
    gap?: number;
    padding?: number;
    paddingX?: number;
    paddingY?: number;
    paddingLeft?: number;
    paddingRight?: number;
    paddingTop?: number;
    paddingBottom?: number;
    width?: number | string;
    height?: number | string;
    minWidth?: number | string;
    minHeight?: number | string;
    maxWidth?: number | string;
    maxHeight?: number | string;
    grow?: number;
    flex?: number;
    shrink?: number;
    alignItems?: string;
    align?: string;
    justifyContent?: string;
    justify?: string;
    position?: string;
    top?: number;
    left?: number;
    right?: number;
    bottom?: number;
    cursor?: string;
    background?: string;
    bg?: string;
    backgroundColor?: string;
    borderRadius?: number | string;
    radius?: number | string;
    borderWidth?: number;
    borderColor?: string;
    opacity?: number;
    overflow?: string;
    overflowY?: string;
    overflowX?: string;
    display?: string;
    color?: string;
    fontSize?: number;
    fontWeight?: number | string;
    elevate?: number;
    // --- native-widget scale keys (host reads them off `node.style`) ---
    /** Native slider: scale minimum (creation-time). */
    min?: number;
    /** Native slider: scale maximum (creation-time). */
    max?: number;
    /** Native slider: step quantum (creation-time). */
    step?: number;
    /** Native progress: truthy shows the indeterminate animation. */
    loading?: number;
    // --- extended native-widget keys (host: build_native extended arms) ---
    /** Native pagination: total page count. */
    total?: number;
    /** Native tag: default/secondary/danger/success/warning/info. */
    variant?: string;
    /** Native separator: "horizontal" | "vertical". */
    orientation?: string;
    /** Native link target URL. */
    href?: string;
}

/**
 * Overlay `over` onto `base`, field by field, in place. The naive
 * `Object.keys` loop is refused by scriptc (SC1090: dynamic keyed writes on a
 * record without an index signature), and `Style` is a closed interface —
 * so the merge is spelled out. Both `canvasStyle` (canvas2d) and `ScrollArea`
 * route their "user style wins over defaults" step through this.
 */
export function mergeStyleInto(base: Style, over: Style): void {
    if (over.flexDirection !== undefined) base.flexDirection = over.flexDirection;
    if (over.direction !== undefined) base.direction = over.direction;
    if (over.gap !== undefined) base.gap = over.gap;
    if (over.padding !== undefined) base.padding = over.padding;
    if (over.paddingX !== undefined) base.paddingX = over.paddingX;
    if (over.paddingY !== undefined) base.paddingY = over.paddingY;
    if (over.paddingLeft !== undefined) base.paddingLeft = over.paddingLeft;
    if (over.paddingRight !== undefined) base.paddingRight = over.paddingRight;
    if (over.paddingTop !== undefined) base.paddingTop = over.paddingTop;
    if (over.paddingBottom !== undefined) base.paddingBottom = over.paddingBottom;
    if (over.width !== undefined) base.width = over.width;
    if (over.height !== undefined) base.height = over.height;
    if (over.minWidth !== undefined) base.minWidth = over.minWidth;
    if (over.minHeight !== undefined) base.minHeight = over.minHeight;
    if (over.maxWidth !== undefined) base.maxWidth = over.maxWidth;
    if (over.maxHeight !== undefined) base.maxHeight = over.maxHeight;
    if (over.grow !== undefined) base.grow = over.grow;
    if (over.flex !== undefined) base.flex = over.flex;
    if (over.shrink !== undefined) base.shrink = over.shrink;
    if (over.alignItems !== undefined) base.alignItems = over.alignItems;
    if (over.align !== undefined) base.align = over.align;
    if (over.justifyContent !== undefined) base.justifyContent = over.justifyContent;
    if (over.justify !== undefined) base.justify = over.justify;
    if (over.position !== undefined) base.position = over.position;
    if (over.top !== undefined) base.top = over.top;
    if (over.left !== undefined) base.left = over.left;
    if (over.right !== undefined) base.right = over.right;
    if (over.bottom !== undefined) base.bottom = over.bottom;
    if (over.cursor !== undefined) base.cursor = over.cursor;
    if (over.background !== undefined) base.background = over.background;
    if (over.bg !== undefined) base.bg = over.bg;
    if (over.backgroundColor !== undefined) base.backgroundColor = over.backgroundColor;
    if (over.borderRadius !== undefined) base.borderRadius = over.borderRadius;
    if (over.radius !== undefined) base.radius = over.radius;
    if (over.borderWidth !== undefined) base.borderWidth = over.borderWidth;
    if (over.borderColor !== undefined) base.borderColor = over.borderColor;
    if (over.opacity !== undefined) base.opacity = over.opacity;
    if (over.overflow !== undefined) base.overflow = over.overflow;
    if (over.overflowY !== undefined) base.overflowY = over.overflowY;
    if (over.overflowX !== undefined) base.overflowX = over.overflowX;
    if (over.display !== undefined) base.display = over.display;
    if (over.color !== undefined) base.color = over.color;
    if (over.fontSize !== undefined) base.fontSize = over.fontSize;
    if (over.fontWeight !== undefined) base.fontWeight = over.fontWeight;
    if (over.elevate !== undefined) base.elevate = over.elevate;
    if (over.min !== undefined) base.min = over.min;
    if (over.max !== undefined) base.max = over.max;
    if (over.step !== undefined) base.step = over.step;
    if (over.loading !== undefined) base.loading = over.loading;
}

export type StyleInput = Style;

// ---------------------------------------------------------------------------
// outbound batching
// ---------------------------------------------------------------------------

let pendingOps: Op[] = [];

/**
 * Synchronous flush. Same grouping semantics as the old microtask version —
 * a whole mount sequence becomes one `batch` line — just driven at call
 * time instead of task-queue time. Under QuickJS the host-side engine drains
 * the ops channel per line, so behavior is unchanged for every caller.
 */
export function flush(): void {
    if (pendingOps.length === 0) return;
    const ops = pendingOps;
    pendingOps = [];
    stats.batchesSent++;
    stats.opsSent += ops.length;
    writeLine(JSON.stringify({ t: "batch", ops: ops }));
}

/** Backward-compat alias (call sites read like the old deferred flush). */
export function scheduleFlush(): void {
    flush();
}

/**
 * `hello` handshake + seeded window title, flushed synchronously. The
 * jsx layer's `createRoot` calls this instead of touching the private
 * transport (`writeLine`) or the batch buffer directly.
 */
export function sendHello(title: string): void {
    writeLine(JSON.stringify({ t: "hello", proto: 1, title: title }));
    // `hello` is diagnostics — the window belongs to the host, so the title
    // only really changes via this op (tree.apply renames the window).
    pendingOps.push({ op: "setTitle", title: title });
    flush();
}

function writeLine(line: string): void {
    if (sink !== null) {
        sink(line);
        return;
    }
    // No sink wired: the line is dropped (stats still advance). A platform
    // entry that has not called setSink() has no channel by definition.
}

// ---------------------------------------------------------------------------
// inbound dispatch (host -> frontend)
// ---------------------------------------------------------------------------

/**
 * A host → frontend event — the `{"t":"event",…}` line. Flat interface
 * rather than a discriminated union: the host is the only producer and every
 * consumer switches on `kind`.
 *
 * Click events carry the DOM-ish control surface: `stopPropagation()` halts
 * the runtime's ancestor walk (see `handleLine`), and `currentTarget` is the
 * element whose handler is currently running (`target` stays the element the
 * interaction physically hit). Both exist on every event for shape stability,
 * but only `click` actually bubbles.
 */
export interface HostEvent {
    kind: string;
    target?: number;
    value?: string;
    top?: number;
    max?: number;
    viewport?: number;
    content?: number;
    /** Element whose handler is currently running (undefined at the target). */
    currentTarget?: number;
    /** Halt the ancestor walk. A no-op outside dispatch. */
    stopPropagation?: () => void;
    /** DOM-compat no-op: the host has no default action to cancel. */
    preventDefault?: () => void;
    /** Dispatch bookkeeping — set by stopPropagation, cleared after the walk. */
    stopped?: boolean;
}

/**
 * scriptc workaround for `Map<string, (msg) => void>` (SC2009: Map values
 * must not be functions): a class with a function-typed FIELD. Class
 * instances are inside the Map value whitelist; closure capture into the
 * field is probe-verified (probe P2/P16).
 */
class LineEntry {
    kind: string = "";
    fn: HostEventHandler | null = null;
}

const lineHandlers = new Map<string, LineEntry>();

export function setLineHandler(kind: string, fn: HostEventHandler | null): void {
    const existing = lineHandlers.get(kind);
    if (existing === undefined) {
        const entry = new LineEntry();
        entry.kind = kind;
        entry.fn = fn;
        lineHandlers.set(kind, entry);
    } else {
        existing.fn = fn;
    }
}

/**
 * The host's clock, when the platform has one to push. Dynamic engines never
 * need it (they own `Date`); a compiled graph has no clock at all, so the
 * host sends `{"t":"now","ms":N}` on the same inbound channel as events and
 * the entry hands that value to pumpPulses. Transport-level data — app code
 * still just receives a timestamp from the heartbeat.
 */
let hostNow: number = 0;

/** Latest host-pushed wall clock in ms since local midnight (0 = never sent). */
export function hostNowMs(): number {
    return hostNow;
}

/**
 * Feed one inbound line. The platform entry calls this — QuickJS bootstrap
 * pumps its injected stdin facade; the scriptc host feeds `gpts_event`.
 * Line shape: `{"t":"event",…}` or `{"t":"now","ms":…}` — anything else is
 * ignored.
 */
export function handleLine(line: string): void {
    // Cheap prefix check (host only ever sends well-formed JSONL; no
    // try/catch in the static subset — same contract as before).
    if (line.length < 2 || line.charAt(0) !== "{") return;
    const parsed: LineMsg = JSON.parse(line);
    if (parsed.t === "now") {
        if (parsed.ms !== undefined) hostNow = parsed.ms;
        return;
    }
    if (parsed.t !== "event") return;
    stats.eventsReceived++;
    // A well-formed event line always carries kind+target; build the typed
    // view once (scriptc lowering keeps this a record copy, no dynamic API).
    const ev: HostEvent = {
        kind: parsed.kind === undefined ? "" : parsed.kind,
        target: parsed.target,
        value: parsed.value,
        top: parsed.top,
        max: parsed.max,
        viewport: parsed.viewport,
        content: parsed.content,
        stopPropagation: function (): void { ev.stopped = true; },
        preventDefault: function (): void { /* no host default action to cancel */ },
    };
    dispatchEvent(ev);
}

/**
 * Dispatch one event to its target and — for bubbling kinds — up the
 * ancestor chain. Element handlers are keyed `(target, kind)`; the global
 * `onHostEvent` listener runs once per event, after the walk, untouched by
 * `stopPropagation` (it is diagnostics, not delegation).
 *
 * Each hop re-points `currentTarget` before calling, so a delegated handler
 * reads the ancestor it is registered on — the DOM contract. The walk uses
 * the frontend's own parent registry (`childRegistry`), i.e. the *mount*
 * tree, which is what a DOM event would bubble through.
 */
function dispatchEvent(ev: HostEvent): void {
    // Field-based stop flag (scriptc: no WeakSet of events, and `stopped`
    // must be per-event, not per-walk — nested dispatch cannot occur since
    // the host feeds one line at a time, but the field costs nothing).
    // The target ALWAYS runs (its own handler is the primary contract);
    // `bubbles` only decides whether the walk continues past it.
    let walk: number | undefined = ev.target;
    while (walk !== undefined) {
        const fn = eventFns.get(numKey(walk, ev.kind));
        if (fn !== undefined && fn.fn !== null) {
            ev.currentTarget = walk;
            const f = fn.fn;
            f(ev);
            if (ev.stopped === true) return finishEvent(ev);
        }
        if (!bubbles(ev.kind)) break; // target-only kind: one hop
        walk = parentOf(walk);
    }
    // Global listener (once, regardless of bubbling — it is diagnostics,
    // not delegation).
    const g = lineHandlers.get("event");
    if (g !== undefined && g.fn !== null) {
        const f2 = g.fn;
        f2(ev);
    }
    finishEvent(ev);
}

/** Control fields back to their resting state; the event escapes to callers. */
function finishEvent(ev: HostEvent): void {
    ev.currentTarget = undefined;
    ev.stopped = false;
}

function bubbles(kind: string): boolean {
    for (let i = 0; i < BUBBLING_KINDS.length; i++) {
        if (BUBBLING_KINDS[i] === kind) return true;
    }
    return false;
}

/** The mount-tree parent of an element (the registry appendChild keeps). */
function parentOf(id: number): number | undefined {
    // Reverse edge in childRegistry: tiny trees make a scan fine; a real
    // parent map would double the bookkeeping append/remove must maintain.
    let found: number | undefined = undefined;
    childRegistry.forEach(function (kids: number[], parent: number): void {
        if (found !== undefined) return;
        for (let i = 0; i < kids.length; i++) {
            if (kids[i] === id) {
                found = parent;
                return;
            }
        }
    });
    return found;
}

/** Inbound line envelope: t is "event" (handled), "now" (host clock) or other. */
interface LineMsg {
    t?: string;
    kind?: string;
    target?: number;
    value?: string;
    top?: number;
    max?: number;
    viewport?: number;
    content?: number;
    ms?: number;
}

/**
 * Kinds that bubble to ancestors. `click` is a *user interaction* and follows
 * DOM semantics: a handler on any ancestor sees it unless someone calls
 * `ev.stopPropagation()`. The rest are control *state* (a field was edited,
 * a scroller moved, focus landed) — they belong to exactly one element and
 * stay target-only, like DOM `input`/`scroll` don't bubble.
 */
const BUBBLING_KINDS: string[] = ["click"];

/** Element-scoped event registry — same class workaround as lineHandlers. */
class EventEntry {
    key: string = "";
    fn: HostEventHandler | null = null;
}

const eventFns = new Map<string, EventEntry>();

function numKey(id: number, kind: string): string {
    return id + ":" + kind;
}

function setEventFn(id: number, kind: string, fn: HostEventHandler): void {
    const key = numKey(id, kind);
    const existing = eventFns.get(key);
    if (existing === undefined) {
        const entry = new EventEntry();
        entry.key = key;
        entry.fn = fn;
        eventFns.set(key, entry);
    } else {
        existing.fn = fn;
    }
}

function dropEventFnsWithPrefix(prefix: string): void {
    // Map.forEach with a lambda is probe-verified (P15). Collect-then-delete
    // keeps the same semantics as the old two-pass loop.
    const doomed: string[] = [];
    eventFns.forEach(function (entry: EventEntry, key: string): void {
        if (key.indexOf(prefix) === 0) doomed.push(key);
    });
    for (let i = 0; i < doomed.length; i++) eventFns.delete(doomed[i]);
}

// ---------------------------------------------------------------------------
// elements & mutations
// ---------------------------------------------------------------------------

export interface El {
    id: number;
}

let nextId = 1;

/**
 * Children per element, so unmounting a subtree can also drop the event
 * handlers registered for it. Map<number, number[]> is probe-verified (P4).
 */
const childRegistry = new Map<number, number[]>();

export function create(tag: string, text?: string, placeholder?: string, options?: string[]): El {
    const id = nextId++;
    const op: Op = { op: "create", id: id, tag: tag };
    if (text !== undefined) op.text = text;
    if (placeholder !== undefined) op.placeholder = placeholder;
    if (options !== undefined) op.options = options;
    pendingOps.push(op);
    flush();
    return { id: id };
}

export function setText(e: El, text: string): void {
    // String() 收敛：数字 signal 会把 number 推进 op，host 端 parse_op 对
    // setText 严格取字符串，number op 会被整条丢弃（计数器空白根因）。
    pendingOps.push({ op: "setText", id: e.id, text: String(text) });
    flush();
}

/** Set a text field value (`tag: "input"`). State, not content. */
export function setValue(e: El, value: string): void {
    pendingOps.push({ op: "setValue", id: e.id, value: String(value) });
    flush();
}

/** Replace a `canvas` node display list (see `canvas2d.ts`). */
export function setCanvas(e: El, cmds: Cmd[]): void {
    pendingOps.push({ op: "setCanvas", id: e.id, cmds: cmds });
    flush();
}

/** Set (or re-set) an element style. The reactive half of `style={...}`. */
export function setStyle(e: El, style: Style): void {
    pendingOps.push({ op: "setStyle", id: e.id, style: style });
    flush();
}

export function wireEvent(e: El, kind: string, fn: HostEventHandler): void {
    setEventFn(e.id, kind, fn);
}

/** Declare which host events this element wants; the host attaches only those. */
export function setEvents(e: El, kinds: string[]): void {
    pendingOps.push({ op: "setEvents", id: e.id, events: kinds });
    flush();
}

/** Mount `child` under `parent`. */
export function appendChild(parent: El, child: El): void {
    const list = childRegistry.get(parent.id);
    if (list === undefined) childRegistry.set(parent.id, [child.id]);
    else list.push(child.id);
    pendingOps.push({ op: "append", id: child.id, parent: parent.id });
    flush();
}

/** Forget a subtree handlers and bookkeeping (ids only — no protocol op). */
function forget(e: El): void {
    const kids = childRegistry.get(e.id);
    if (kids !== undefined) {
        for (let i = 0; i < kids.length; i++) {
            const kid: El = { id: kids[i] };
            forget(kid);
        }
        childRegistry.delete(e.id);
    }
    dropEventFnsWithPrefix(e.id + ":");
}

/** Remove an element and its whole subtree on the host. */
export function remove(e: El): void {
    forget(e);
    pendingOps.push({ op: "remove", id: e.id });
    flush();
}

/** Remove all children of an element on the host. */
export function clearEl(e: El): void {
    const kids = childRegistry.get(e.id);
    if (kids !== undefined) {
        for (let i = 0; i < kids.length; i++) {
            const kid: El = { id: kids[i] };
            forget(kid);
        }
        childRegistry.delete(e.id);
    }
    pendingOps.push({ op: "clear", id: e.id });
    flush();
}

// ---------------------------------------------------------------------------
// reactivity (fine-grained, Solid-style)
// ---------------------------------------------------------------------------

let activeEffect: (() => void) | null = null;

/**
 * Signal subscribers — the old `Set<() => void>` (function-valued Set) is
 * refused by scriptc; a class holder array is the static equivalent. Order
 * matches insertion; duplicates are prevented by identity check (the old
 * Set.add semantics).
 */
class Subscriber {
    run: (() => void) | null = null;
}

export function createSignal<T>(initial: T): [() => T, (v: T) => void] {
    let value = initial;
    const subs: Subscriber[] = [];
    const read = (): T => {
        if (activeEffect !== null) {
            let found = false;
            for (let i = 0; i < subs.length; i++) {
                if (subs[i].run === activeEffect) {
                    found = true;
                    break;
                }
            }
            if (!found) {
                const s = new Subscriber();
                s.run = activeEffect;
                subs.push(s);
            }
        }
        return value;
    };
    const write = (v: T): void => {
        value = v;
        // Snapshot first — a subscriber that writes another signal must not
        // see a mutating array (same guarantee as the old forEach copy).
        const snapshot: Subscriber[] = [];
        for (let i = 0; i < subs.length; i++) snapshot.push(subs[i]);
        for (let i = 0; i < snapshot.length; i++) {
            const s = snapshot[i];
            if (s.run !== null) s.run();
        }
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
 * JSX fragment marker. Only identity-comparable — a plain string keeps it
 * free of any Symbol/runtime dependency.
 */
export const Fragment = "#fragment";

/**
 * Anything allowed as a JSX child (also accepted by `h`). Recursive alias —
 * QuickJS path needs the nesting in the type (JSX arrays of children); the
 * scriptc pipeline rewrites this alias to a flat union in its desugar step
 * (recursion is the ONE thing scriptc's type system cannot carry).
 */
export type Child = El | string | number | boolean | null | undefined | Child[];

/**
 * Loose child-list element type — the any-free element domain of ChildList.
 */
export type Child0 = El | string | number | boolean | null | undefined;

/**
 * Child list shape for appendChildren. The nesting level is one array deep
 * in the type (arrays of children), which is enough for the common
 * `{arr.map(...)}` shape; deeper nesting is still handled at runtime.
 */
export type ChildList = Child0[];

/**
 * The static stand-in for `any`: an explicit closed union of the value kinds
 * this protocol can carry. Used at the component boundary, where the value's
 * shape is the app's business (`Task`, the canvas `Ctx` callback).
 * `any` in value position has no static representation and is refused
 * outright (SC2011); this union is what the checker asks for instead.
 *
 * No `symbol` arm: nothing in this runtime has a Symbol (Fragment is a plain
 * string for exactly that reason).
 */
export type Any = string | number | boolean | object | null | undefined;

/** A JSX component: a props bag in, an element out. */
export type AnyComponent = (props: Any) => El;

/**
 * Handler shape on intrinsic tags: zero-param arrows and host-event handlers
 * both assign exactly (see HostEventHandler). Components such as TextField
 * keep their own `(v: string) => void` callbacks — the desugar step calls
 * components directly, so those never flow through this vocabulary.
 */
export type HostEventHandler = ((ev: HostEvent) => void) | (() => void);

/**
 * The JSX prop vocabulary — ONE closed interface for every attribute the app
 * writes on any tag, intrinsic or component.
 *
 * Why closed: the desugar step turns JSX into real `h(tag, props, …)` calls,
 * so scriptc's checker validates those literals against this interface. An
 * open type (`Record<string, any>`) has no static representation (SC2011),
 * and a dynamic value cannot be cast into an interface that carries function
 * fields ("a dynamic value can only be validated against JSON-representable
 * types"). So the bag is enumerated here, exactly as `Style` is for the style
 * channel. Adding an attribute to a component means adding its field here —
 * that is the price (and the point) of a statically compiled graph.
 *
 * `globals.d.ts` still owns the author-facing view: intrinsic tags are checked
 * against `JSX.IntrinsicProps` there, and components against their own `props`
 * type. This interface is the *lowered* view of the same vocabulary.
 */
export interface Props {
    // --- intrinsic surface: read by h() itself, so typed precisely ---
    style?: Style;
    text?: string;
    value?: string | number | (() => string) | (() => number);
    placeholder?: string;
    children?: Child;
    onClick?: HostEventHandler;
    onInput?: HostEventHandler;
    onChange?: HostEventHandler;
    onScroll?: HostEventHandler;
    onFocus?: HostEventHandler;
    onBlur?: HostEventHandler;
    // --- component vocabulary (app.tsx + kit.tsx); the components read these
    // through their own declared prop types, this side only has to accept
    // them at the call site ---
    title?: string;
    label?: string;
    color?: string;
    bg?: string;
    fg?: string;
    row?: string;
    width?: number;
    height?: number;
    index?: number;
    /** Native slider scale keys (creation-time, like the select option list). */
    min?: number;
    max?: number;
    step?: number;
    /** Native progress: `1` shows the indeterminate animation. */
    loading?: number;
    grow?: boolean;
    options?: string[];
    checked?: () => boolean;
    selected?: () => boolean;
    onToggle?: () => void;
    onSelect?: () => void;
    onPick?: () => void;
    /** Native alert: dismissed by the user. */
    onClose?: () => void;
    onPresent?: (cmds: number, redraws: number) => void;
    /** App-level payload objects (e.g. `Task`) and canvas callbacks (`Ctx`):
     * their shapes live with the app, so they cross as `Any`. */
    t?: Any;
    draw?: Any;
}

