/**
 * BOM access layer — one guarded door to the browser-ish globals.
 *
 * The host installs the BOM as part of the QuickJS environment
 * (`host/src/bootstrap.js`). The **Perry** backend brings its own JS runtime
 * and gets no BOM at all, and the **scriptc** AOT build has no dynamic global
 * environment to read from — reading a bare global there TRAPS at runtime
 * (`Uncaught ReferenceError: window is not defined` in gpts_init).
 *
 * So the globals are never read HERE. The dynamic-engine shell (`runtime.ts`,
 * which scriptc never compiles) probes them with `typeof` — legal in real JS —
 * and injects a `BomEnv` of live closures via `setBomEnv`. The accessors below
 * take the no-BOM branch unless an environment was injected, which keeps the
 * app source identical on all three platforms.
 *
 * Nothing here caches a value: sizes and DPR change at runtime, so the
 * injected closures re-read their globals on every call.
 */

/**
 * The injected BOM environment: live closures over the browser-ish globals.
 * scriptc compiles this interface but never sees an instance — the only
 * constructor lives in the QuickJS/perry shell.
 */
export interface BomEnv {
    /** `"1180 × 760"` — re-read on every call (resize-safe). */
    size: () => string;
    /** Device pixel ratio, formatted. */
    dpr: () => string;
    agent: () => string;
    href: () => string;
    /** Monotonic ms since host start. */
    now: () => number;
    uuid: () => string;
    onResize: (fn: () => void) => void;
    /** Schedule a frame; `time` is a `performance.now()` stamp. */
    raf: (fn: (time: number) => void) => void;
    alert: (msg: string) => void;
    confirm: (msg: string) => boolean;
}

let env: BomEnv | null = null;

/** Inject (or clear) the BOM environment. Called once by `runtime.ts`. */
export function setBomEnv(e: BomEnv | null): void {
    env = e;
}

/**
 * The gate. Kept as a module-internal check inside each accessor — see
 * docs/gpui-ts-plan.md 坑 #8: perry 0.5.1520 mishandles an imported const's
 * binding in the staticlib codegen, so consumers must never branch on an
 * exported `hasBom` value; they use the formatters below instead.
 */
function hasEnv(): boolean {
    return env !== null;
}

/** `1180 × 760`, or an explicit note when there is no BOM. */
export function windowSize(): string {
    if (env === null) return "无 BOM（Perry 后端）";
    const f = env.size;
    return f();
}

export function dpr(): string {
    if (env === null) return "-";
    const f = env.dpr;
    return f();
}

export function userAgent(): string {
    if (env === null) return "perry-native（无 BOM）";
    const f = env.agent;
    return f();
}

export function href(): string {
    if (env === null) return "-";
    const f = env.href;
    return f();
}

/** Monotonic ms since host start; 0 where there is no BOM. */
export function perfNow(): number {
    if (env === null) return 0;
    const f = env.now;
    return f();
}

/**
 * Format an uptime sample taken at mount.
 *
 * A function rather than an inline gate at the call site on purpose — see the
 * note above 坑 #8.
 */
export function uptimeLabel(ms: number): string {
    return hasEnv() ? ms + " ms" : "-";
}

export function newUuid(): string {
    if (env === null) return "无 BOM";
    const f = env.uuid;
    return f();
}

/** Subscribe to host window resizes. No-op without a BOM. */
export function onResize(fn: () => void): void {
    if (env === null) return;
    const f = env.onResize;
    f(fn);
}

/**
 * Schedule a frame. The callback's `time` is a `performance.now()` stamp.
 *
 * Returns whether anything was actually scheduled — callers use that instead of
 * testing the gate themselves.
 */
export function raf(fn: (time: number) => void): boolean {
    if (env === null) return false;
    const f = env.raf;
    f(fn);
    return true;
}

/** Blocking modal; without a host dialog it just proceeds (true). */
export function askConfirm(message: string): boolean {
    if (env === null) return true;
    const f = env.confirm;
    return f(message);
}

/** Blocking modal notice; a no-op without a BOM. */
export function notify(message: string): void {
    if (env === null) return;
    const f = env.alert;
    f(message);
}
