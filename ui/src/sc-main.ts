/**
 * scriptc library-mode entry over the REAL app graph (io core + app.tsx +
 * kit + canvas2d + bom).
 *
 * scriptc 0.1.3 `--lib` contract (established by probe, see ll + memory):
 *   - `abi.init_symbol` (gpts_init) resets the runtime ARENA and runs the
 *     MODULE TOP-LEVEL statements — it never calls any exported function.
 *     Anything the host must happen at init therefore lives at top level.
 *     Functions not reachable from exports/top-level are tree-shaken.
 *   - `exports[]` must be `export function` declarations in the entry module
 *     (SC4002), with symbols distinct from the reserved abi symbols (SC4001).
 *   - the graph must be async-free: no `Promise`/`setTimeout` (SC4005) and
 *     no timers surface — hence the pulse registry: the app registers
 *     periodic work, THIS entry's appTick drains it off the host quantum.
 *
 * This entry is the platform half of io.ts's two-fact contract:
 *   who am I      → setEngineLabel("scriptc-aot")
 *   heartbeat me  → appTick → pumpPulses(uptimeMs)
 * Everything above (the whole UI graph) is identical to the QuickJS build —
 * same app.tsx, same screens, same protocol lines.
 *
 * Host-pull protocol (everything synchronous, driven by the host thread):
 *
 *     gpts_init   → top level  : sink wiring + createRoot(App)
 *     gpts_tick   → appTick    : heartbeat, 10 ms quantum (pulse every 10th)
 *     gpts_poll   → appPoll    : one buffered protocol line ("" = drained)
 *     gpts_event  → appEvent   : host pushes `{"t":"event",…}` JSON
 *     gpts_reset  → appReset   : entry state reset (host does not call it today)
 *
 * This file is TS-source for scriptc's own checker. Never esbuild-bundle it
 * first: type erasure turns typed ops into dynamic dispatch (SC4005).
 */

import {
    createRoot,
    handleLine,
    hostNowMs,
    pumpPulses,
    setEngineLabel,
    setSink,
} from "./io";
import { App } from "./app";

// --- outbound: the sink writes into a "\n"-joined outbox string ------------
//
// A string, deliberately NOT string[]: under strictNullChecks every array
// read keeps `| undefined` in the lowered IR and scriptc refuses the export
// (SC4003). String slicing never unions.
let outbox: string = "";

setSink(function (line: string): void {
    outbox = outbox + line + "\n";
});

// --- platform fact 1: who am I ---------------------------------------------
setEngineLabel("scriptc-aot");

// --- platform fact 2: heartbeat --------------------------------------------
//
// The host driver calls gpts_tick every 10 ms and drains gpts_poll one line
// per tick (100 lines/s of headroom). Pumping the registry every 10th tick
// gives the app a 100 ms heartbeat — the same "refresh the stats card" beat
// the QuickJS build gets from runtime.ts, at a tenth of the poll pressure.
// The timestamp handed to the pulse is uptime: a compiled graph has no
// clock of its own (no Date, no runtime), so the entry's tick counter IS
// the clock — the Phase-1 demo shipped the same honest substitution.
let tickN: number = 0;

export function appTick(): void {
    tickN = tickN + 1;
    if (tickN % 10 === 0) {
        pumpPulses(tickN * 10);
    }
}

/** One buffered protocol line, or "" when drained (host-pull). */
export function appPoll(): string {
    if (outbox === "") {
        return "";
    }
    const idx: number = outbox.indexOf("\n");
    const line: string = outbox.substring(0, idx);
    outbox = outbox.substring(idx + 1);
    return line;
}

/** Host → frontend events: the same JSONL the other backends ingest. */
export function appEvent(json: string): void {
    handleLine(json);
}

/** Entry-state reset. (The host never calls this today — gpts_init is the
 * whole story — but the ABI symbol stays exported for symmetry.) */
export function appReset(): void {
    outbox = "";
    tickN = 0;
}

// Module top level == the body of abi.init_symbol (gpts_init): scriptc runs
// these statements after the runtime reset when the host calls gpts_init.
// Sink first (so the mount sequence is buffered, not dropped), then the app.
createRoot({ title: "nativets × scriptc" }, App);
