/**
 * Ambient declarations for editor / `tsc` only — nothing here reaches the
 * bundle (esbuild erases types, and the host provides these globals at runtime).
 *
 * Two deliberate choices:
 *
 * 1. `process` — the host's process shim (QuickJS has no such global; the
 *    Perry/node runtimes do). Declared by hand because this project has no
 *    `@types/node` dependency.
 *
 * 2. **The BOM is declared by hand and `lib.dom` is NOT used.** Pulling in
 *    `DOM` would type `document`, `localStorage`, `fetch`, `XMLHttpRequest` and
 *    friends as present, when the host provides none of them — a type error you
 *    only discover at runtime is worse than no type at all. What is declared
 *    below is exactly what `host/src/bootstrap.js` installs, so `tsc` doubles
 *    as the guard against reaching for an API that does not exist here.
 *
 * Keep this file in sync with `host/src/bootstrap.js`. Its behaviour (including
 * the "document is absent" guarantee) is covered by `host/test/bootstrap.test.mjs`.
 */

// ---------------------------------------------------------------------------
// host process shim
// ---------------------------------------------------------------------------

interface HostProcess {
    env: { [key: string]: string | undefined };
    argv: string[];
    platform: string;
    stdout: { write(s: string): void };
    stderr: { write(s: string): void };
    stdin: {
        setEncoding(encoding: string): void;
        on(event: string, handler: (chunk: any) => void): void;
    };
    exit(code?: number): void;
}

declare const process: HostProcess;

/** Emitted by the host in `direct` transport mode (QuickJS in-process). */
declare function __hostEmit(line: string): void;

/**
 * The published package name for the runtime (`import … from "nativets"`).
 * The perry pretranspile rewrites it to `./io` before AOT compilation; the
 * QuickJS esbuild plugin maps it to the package runtime; editor tsc sees
 * this declaration so demos in `src/` type-check without a node_modules
 * install of the package itself. The surface mirrors `types/index.d.ts`.
 */
declare module "nativets" {
    export * from "./runtime";
}

// ---------------------------------------------------------------------------
// console
// ---------------------------------------------------------------------------

interface HostConsole {
    log(...args: any[]): void;
    info(...args: any[]): void;
    warn(...args: any[]): void;
    error(...args: any[]): void;
    debug(...args: any[]): void;
    trace(...args: any[]): void;
    dir(o: any): void;
    assert(condition: any, ...args: any[]): void;
}

declare const console: HostConsole;

// ---------------------------------------------------------------------------
// timers & scheduling
// ---------------------------------------------------------------------------

declare function setTimeout(cb: (...args: any[]) => void, ms?: number): number;
declare function clearTimeout(id: number): void;
declare function setInterval(cb: (...args: any[]) => void, ms?: number): number;
declare function clearInterval(id: number): void;
declare function queueMicrotask(cb: () => void): void;

/** `time` is a `performance.now()` timestamp, as in a browser. */
declare function requestAnimationFrame(cb: (time: number) => void): number;
declare function cancelAnimationFrame(id: number): void;

interface IdleDeadline {
    readonly didTimeout: boolean;
    timeRemaining(): number;
}
declare function requestIdleCallback(cb: (deadline: IdleDeadline) => void, options?: { timeout?: number }): number;
declare function cancelIdleCallback(id: number): void;

// ---------------------------------------------------------------------------
// events
// ---------------------------------------------------------------------------

interface HostEventInit {
    bubbles?: boolean;
    cancelable?: boolean;
}

interface HostCustomEventInit<T = any> extends HostEventInit {
    detail?: T;
}

declare class Event {
    constructor(type: string, init?: HostEventInit);
    readonly type: string;
    /** The global object — there is no DOM to target. */
    target: any;
    readonly timeStamp: number;
    defaultPrevented: boolean;
    readonly bubbles: boolean;
    readonly cancelable: boolean;
    preventDefault(): void;
    stopPropagation(): void;
}

declare class CustomEvent<T = any> extends Event {
    constructor(type: string, init?: HostCustomEventInit<T>);
    readonly detail: T;
}

// ---------------------------------------------------------------------------
// window / navigator / location / screen / performance / crypto
// ---------------------------------------------------------------------------

interface HostStyle {
    readonly width: number;
    readonly height: number;
    readonly availWidth: number;
    readonly availHeight: number;
    readonly colorDepth: number;
    readonly pixelDepth: number;
}

interface HostLocation {
    readonly href: string;
    readonly protocol: string;
    readonly host: string;
    readonly hostname: string;
    readonly port: string;
    readonly pathname: string;
    readonly search: string;
    readonly hash: string;
    readonly origin: string;
    reload(): void;
    assign(url: string): void;
    replace(url: string): void;
    toString(): string;
}

interface HostNavigator {
    readonly userAgent: string;
    readonly platform: string;
    readonly language: string;
    readonly languages: readonly string[];
    /** Always false: there is no network stack. */
    readonly onLine: boolean;
    readonly cookieEnabled: boolean;
    readonly hardwareConcurrency: number;
    readonly maxTouchPoints: number;
    readonly isSecureContext: boolean;
}

interface HostPerformance {
    readonly timeOrigin: number;
    /** Monotonic milliseconds — `Date.now()` is wall clock and can step. */
    now(): number;
    mark(name: string): void;
    measure(name: string, start?: string, end?: string): void;
}

interface HostCrypto {
    /** Fills in place from the OS CSPRNG and returns the same array. */
    getRandomValues<T extends ArrayBufferView>(array: T): T;
    randomUUID(): string;
}

interface HostWindow {
    readonly innerWidth: number;
    readonly innerHeight: number;
    readonly outerWidth: number;
    readonly outerHeight: number;
    readonly devicePixelRatio: number;
    readonly screenX: number;
    readonly screenY: number;
    /** `window` is the global object, so `self` and bare globals are the same. */
    window: HostWindow;
    self: HostWindow;
    readonly screen: HostStyle;
    readonly location: HostLocation;
    readonly navigator: HostNavigator;
    readonly performance: HostPerformance;
    readonly crypto: HostCrypto;

    addEventListener(type: string, fn: (ev: any) => void): void;
    removeEventListener(type: string, fn: (ev: any) => void): void;
    /** Accepts an event object or a bare type string. */
    dispatchEvent(ev: { type: string } | string): boolean;

    /** `window.on<type>` works for any dispatched type. */
    [key: `on${string}`]: ((ev: any) => void) | undefined;
}

declare const window: HostWindow;
declare const self: HostWindow;

declare const screen: HostStyle;
declare const location: HostLocation;
declare const navigator: HostNavigator;
declare const performance: HostPerformance;
declare const crypto: HostCrypto;

declare const innerWidth: number;
declare const innerHeight: number;
declare const devicePixelRatio: number;

declare function addEventListener(type: string, fn: (ev: any) => void): void;
declare function removeEventListener(type: string, fn: (ev: any) => void): void;
declare function dispatchEvent(ev: { type: string } | string): boolean;

/** Deep clone. Throws on functions, like the browser API. */
declare function structuredClone<T>(value: T): T;

// ---------------------------------------------------------------------------
// dialogs (rendered by the host as a GPUI modal)
// ---------------------------------------------------------------------------

/** Blocks until dismissed, like a browser. */
declare function alert(message?: any): void;
/** Blocks until answered, like a browser. */
declare function confirm(message?: any): boolean;
/** Not implemented by the host — returns `defaultValue` and warns. */
declare function prompt(message?: string, defaultValue?: string): string | null;

// ---------------------------------------------------------------------------
// JSX
// ---------------------------------------------------------------------------

declare namespace JSX {
    /** A JSX expression evaluates to a host element handle. */
    type Element = import("./runtime").El;

    type Style = import("./runtime").Style;

    /**
     * The props an intrinsic (lowercase) tag understands. Styling goes in
     * `style` — because this interface has no index signature, writing a style
     * key at the top level (e.g. `<div padding={16}>`) is a type error rather
     * than a silently ignored attribute.
     *
     * The event props are not the `on…` handlers of the DOM: each one *declares*
     * an event to the host (`setEvents`), which then sends it back with a
     * payload (see `HostEvent`). `onFocus`/`onBlur` are field-only; `onScroll`
     * needs an `overflow: "scroll"` node.
     *
     * Delegation: the mouse family (`click`, `mousedown`, `mouseup`,
     * `dblclick`, `wheel`) bubbles like the DOM — an ancestor's handler fires
     * after the target's, with `ev.currentTarget` pointed at the ancestor;
     * `ev.stopped = true` inside a handler halts the walk. The state kinds
     * (`input`/`change`/`scroll`/`focus`/`blur`) stay on target, matching the
     * DOM (input/scroll/focus do not bubble).
     */
    interface IntrinsicProps {
        style?: Style;
        /** Bubbles to ancestors (DOM click semantics; ev.stopped honored). */
        onClick?: (ev: HostEvent) => void;
        /** Left button pressed; bubbles like click. */
        onMouseDown?: (ev: HostEvent) => void;
        /** Left button released over the element; bubbles like click. */
        onMouseUp?: (ev: HostEvent) => void;
        /** Two clicks within 500ms on the element; bubbles like click. */
        onDblClick?: (ev: HostEvent) => void;
        /** A wheel tick over a plain (non-scroll) div; bubbles like click. */
        onWheel?: (ev: HostEvent) => void;
        /** Text field edited; `ev.value` is the new text. */
        onInput?: (ev: HostEvent) => void;
        /** Enter pressed in a text field; `ev.value` is the committed text. */
        onChange?: (ev: HostEvent) => void;
        /** A scroll container moved; `ev.top` / `ev.max` / `ev.viewport` / `ev.content`. */
        onScroll?: (ev: HostEvent) => void;
        onFocus?: (ev: HostEvent) => void;
        onBlur?: (ev: HostEvent) => void;
        /** Initial text content; same as passing a string child. */
        text?: string;
        /**
         * Controlled value: a literal, or a getter for a controlled widget.
         * input: the text; checkbox/switch: "true"/"false"; select: the
         * option; date: ISO "YYYY-MM-DD"; progress/slider: a number
         * (percent 0..=100 / thumb position) — numeric getters must be
         * wrapped as `() => String(n())` (the runtime has one getter slot
         * type; a ()=>number value in a ()=>string slot traps).
         */
        value?: string | number | (() => string);
        /** Text field hint, shown while the value is empty. */
        placeholder?: string;
        /** Native checkbox/switch: reactive checked state ("true"/"false" on the wire). */
        checked?: boolean | (() => boolean);
        /** Native select: the option list. */
        options?: string[];
        /** Native checkbox/switch: the label rendered next to the control. */
        label?: string;
        /** Native slider scale (creation-time; a different scale is a new slider). */
        min?: number;
        max?: number;
        step?: number;
        /** Native progress: truthy shows the indeterminate loading animation. */
        loading?: number | boolean;
        /** Native slider dragged; `ev.value` is the live thumb position. */
        onInput?: (ev: HostEvent) => void;
        children?: any;
    }

    type HostEvent = import("./runtime").HostEvent;

    interface IntrinsicElements {
        [tag: string]: IntrinsicProps;
    }

    /** Tells the compiler which prop carries JSX children. */
    interface ElementChildrenAttribute {
        children: {};
    }
}
