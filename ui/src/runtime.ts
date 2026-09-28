/**
 * runtime.ts — the public authoring surface, now a thin shell over io.ts.
 *
 * io.ts is the scriptc-compatible protocol core (static subset: class-based
 * handler registries, synchronous flush, injectable sink — see its header).
 * This module keeps two responsibilities:
 *
 *   1. re-export the whole authoring API (identical names as before, so no
 *      call site changes), and
 *   2. wire the QuickJS/Node transport: probe the host-injected `__hostEmit`
 *      (typeof on `globalThis` — dynamic engine only, never compiled by
 *      scriptc) and install it as the outbound sink; pump the injected stdin
 *      facade into handleLine(). The old process.* leftovers (trace, stderr,
 *      exit-on-stdin-end) are gone — shutdown is the host's call now.
 *
 * scriptc never compiles this file: its graph starts at a static entry that
 * imports io.ts directly (see build-scriptc.mjs / sc-main wiring).
 */

// ---- full API re-export (names unchanged for every call site) --------------

export {
    stats,
    setSink,
    hasSink,
    flush,
    scheduleFlush,
    handleLine,
    setValue, setCanvas, setStyle,
    appendChild, remove,
    createSignal, createEffect,
    Fragment,
    h,
    text,
    Show, For,
    setRootStyle,
    onHostEvent,
    setEngineLabel, engineName,
    onPulse, pumpPulses,
    createRoot,
} from "./io";

export type {
    HostEvent, HostEventHandler,
    Op, Cmd, Style,
    El,
    Child, Child0,
    TextSource,
} from "./io";

import {
    setSink,
    handleLine,
    handleHostEvent,
    stats,
    flush,
    setEngineLabel,
    pumpPulses,
} from "./io";
import { setBomEnv } from "./bom";
import { hasSink } from "./io-core";

// ---------------------------------------------------------------------------
// QuickJS / Node transport wiring (dynamic-engine only)
// ---------------------------------------------------------------------------

/**
 * Outbound: install the host-injected native emitter as the sink. When the
 * host injects nothing (Perry embedded: JSONL over the stdio pipes), fall
 * back to `process.stdout.write` — the embedded host splices the frontend's
 * stdout into an anonymous pipe and parses the same JSONL protocol off it.
 * `typeof` probing is legal here — this file only ever runs on a dynamic
 * engine (QuickJS/node/perry), never under scriptc's static compiler.
 *
 * HISTORY NOTE: the 0ac0388 split dropped this stdout fallback ("no sink =
 * drop"), which silently killed the Perry backend — its host injects no
 * `__hostEmit`, so every hello/batch line was discarded and the window came
 * up blank. The fallback is load-bearing for perry; keep it.
 */
// io-core now self-wires a default sink at its own top level (hostEmit
// probe → stdout fallback), so plain `import "./io"` entries get a working
// channel without this module. This block stays for compatibility and is
// a no-op when io-core already wired something (hasSink guard) — no double
// wrapping of stdout.

const hostEmit: ((line: string) => void) | null = (function (): ((line: string) => void) | null {
    if (typeof globalThis === "undefined") return null;
    const g = globalThis as { __hostEmit?: unknown };
    if (typeof g.__hostEmit === "function") return g.__hostEmit as (line: string) => void;
    return null;
})();

if (!hasSink()) {
    if (hostEmit !== null) {
        setSink(hostEmit);
    } else if (
        typeof process !== "undefined" &&
        process.stdout !== undefined &&
        typeof process.stdout.write === "function"
    ) {
        // JSONL over stdout — child-process / Perry embedded / `pipe` transport.
        setSink(function (line: string): void {
            process.stdout.write(line + "\n");
        });
    }
}

/**
 * Inbound: the host pushes event lines through the stdin facade it injects
 * (`process.stdin.on("data", …)`, a callback registry — no real fd). Keep
 * that contract but own the buffering here, guarded so a runtime without
 * the facade simply never receives events.
 *
 * NOTE: no process.exit-on-end — window close tears the host process down;
 * the old handshake is obsolete under the always-injected transport.
 */
interface StdinFacade {
    on: (ev: string, fn: (chunk: unknown) => void) => void;
    setEncoding: (enc: string) => void;
}

const stdinFacade: StdinFacade | null = (function (): StdinFacade | null {
    if (typeof process === "undefined") return null;
    const p = process as unknown as { stdin?: { on?: unknown; setEncoding?: unknown } };
    const s = p.stdin;
    if (s !== undefined && typeof s.on === "function" && typeof s.setEncoding === "function") {
        return s as unknown as StdinFacade;
    }
    return null;
})();

let stdinBuffer = "";

function pumpStdin(chunk: unknown): void {
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

if (stdinFacade !== null) {
    stdinFacade.setEncoding("utf8");
    stdinFacade.on("data", pumpStdin);
}

// ---------------------------------------------------------------------------
// ABI tier (quickjs in-process host): register the typed event callback and
// expose it to the host. The engine's pump resolves the *global*
// `__hostEventSink` and calls it with `(kind, target, value, top, max,
// viewport, content)` — one call per event, no JSON stage on either side.
// The global here bridges into io-core's registered slot via
// `handleHostEvent` (which assembles the same HostEvent view `handleLine`
// would and runs the shared dispatch). Only installed when a quickjs host
// is actually present (`__hostOps` probe) so node-dev/perry environments
// keep their pristine global object.
// ---------------------------------------------------------------------------

(function installAbiEventBridge(): void {
    if (typeof globalThis === "undefined") return;
    const g = globalThis as { __hostOps?: unknown; __hostEventSink?: unknown };
    // Capability probe: a host that injects `__hostOps` (ABI tier) also
    // pushes structured events; JSONL-only hosts (perry embedded, node-dev)
    // must not see this global or their `t:"event"` lines would be double-
    // handled (they already reach io-core via pumpStdin → handleLine).
    if (typeof g.__hostOps !== "function") return;
    if (g.__hostEventSink !== undefined) return; // already installed
    g.__hostEventSink = function (
        kind: string,
        target: number,
        value: string,
        top: number,
        max: number,
        viewport: number,
        content: number,
    ): void {
        handleHostEvent(kind, target, value, top, max, viewport, content);
    };
})();

/**
 * Node-dev still has a real `process` but no host callbacks: nothing to do
 * for the ABI tier there — io-core's boot probe found no `__hostOps`, so
 * flush() uses the JSONL sink and events arrive via `pumpStdin`.
 */
void handleLine;

// ---------------------------------------------------------------------------
// BOM injection — live closures over the browser-ish globals
// (QuickJS implementation of the bom.ts injectable seam)
// ---------------------------------------------------------------------------

/**
 * The host's bootstrap installs `window`(=globalThis), `navigator`,
 * `location`, `performance`, `crypto`, `devicePixelRatio`,
 * `addEventListener`, `requestAnimationFrame`, `alert`, `confirm`. bom.ts
 * never reads those bare globals itself (a static engine has no global
 * environment to read — reading one TRAPS); it asks for this env instead.
 * Every closure re-reads its global, so sizes/DPR stay live.
 *
 * typeof guards are legal here: this file only runs on dynamic engines.
 * Perry installs no BOM — the gate stays closed and the app shows the
 * "无 BOM" diagnostics everywhere.
 */
if (
    typeof window !== "undefined" &&
    typeof performance !== "undefined" &&
    typeof crypto !== "undefined"
) {
    setBomEnv({
        size: function (): string {
            const w = window as { innerWidth?: unknown; innerHeight?: unknown };
            if (w.innerWidth === undefined || w.innerHeight === undefined) return "-";
            return String(w.innerWidth) + " × " + String(w.innerHeight);
        },
        dpr: function (): string {
            if (typeof devicePixelRatio === "undefined") return "-";
            return String(devicePixelRatio);
        },
        agent: function (): string {
            const nav = globalThis as { navigator?: { userAgent?: unknown } };
            if (nav.navigator === undefined || nav.navigator.userAgent === undefined) return "-";
            return String(nav.navigator.userAgent);
        },
        href: function (): string {
            const loc = globalThis as { location?: { href?: unknown } };
            if (loc.location === undefined || loc.location.href === undefined) return "-";
            return String(loc.location.href);
        },
        now: function (): number {
            return performance.now();
        },
        uuid: function (): string {
            const c = crypto as { randomUUID?: () => string };
            if (c.randomUUID === undefined) return "-";
            return c.randomUUID();
        },
        onResize: function (fn: () => void): void {
            (globalThis as unknown as { addEventListener: (t: string, f: () => void) => void })
                .addEventListener("resize", fn);
        },
        raf: function (fn: (time: number) => void): void {
            (globalThis as unknown as { requestAnimationFrame: (f: (t: number) => void) => void })
                .requestAnimationFrame(fn);
        },
        alert: function (msg: string): void {
            (globalThis as unknown as { alert: (m: string) => void }).alert(msg);
        },
        confirm: function (msg: string): boolean {
            return (globalThis as unknown as { confirm: (m: string) => boolean }).confirm(msg);
        },
    });
}

// ---------------------------------------------------------------------------
// platform hooks: engine label, wall clock, heartbeat
// (QuickJS implementations of the io.ts injectable seams)
// ---------------------------------------------------------------------------

/**
 * Engine label for the status bar — the old app.tsx IIFE moved here verbatim
 * (this is the one module allowed to probe `process`).
 *   quickjs — the host injects `GPUI_TS_ENGINE=quickjs` into the process shim
 *   node-dev — `PERRY_DEV` is set by the dev launcher
 *   perry-native — anything else (the embedded Perry staticlib)
 */
setEngineLabel((function (): string {
    if (typeof process !== "undefined" && process.env) {
        if (process.env.GPUI_TS_ENGINE === "quickjs") return "quickjs-embedded";
        if (process.env.PERRY_DEV !== undefined) return "node-dev";
    }
    return "perry-native";
})());

/** Wall clock: ms since local midnight — hour-of-day math reads local time. */
const dayMs = 24 * 60 * 60 * 1000;

function localMs(): number {
    const tzOffsetMs = new Date().getTimezoneOffset() * 60 * 1000;
    return (Date.now() - tzOffsetMs) % dayMs;
}

/**
 * Heartbeat: fire the app's pulse registry 4×/s, handing it the wall clock.
 * This is the whole timer surface left on the QuickJS path — the two
 * app-level intervals that used to live in wireStats() are now one
 * platform-driven pulse (the scriptc entry drives the same registry from
 * `gpts_tick`, with uptime as its clock).
 */
setInterval(function (): void {
    pumpPulses(localMs());
}, 250);

/**
 * Diagnostics one-liner for tests and tools (replaces the old stderr trace).
 */
export function statsLine(): string {
    return (
        "batches=" + String(stats.batchesSent) +
        " ops=" + String(stats.opsSent) +
        " events=" + String(stats.eventsReceived)
    );
}

// The flush import is referenced here so tree-shakers never consider the
// module-level wiring side effects dead.
void flush;
