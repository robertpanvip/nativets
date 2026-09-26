/**
 * Demo app composition root — nativets × GPUI desktop dashboard.
 *
 * Authored in TSX. The build compiles JSX directly to the runtime's `h()`
 * factory (`--jsx=transform --jsx-factory=h --jsx-fragment=Fragment`), so JSX
 * is pure sugar over the same mutation protocol.
 *
 * The dashboard is composed from small card modules under `./cards` plus shared
 * primitives (`./primitives`) and the centralised state (`./state`). This file
 * only wires the shell (header + sidebar + main panel + footer) and the runtime
 * heartbeat — every card and every signal lives elsewhere, so adding a screen is
 * a new file, not an edit to a 1000-line module.
 */

import { h, setRootStyle, appendChild, onHostEvent, onPulse, stats } from "./io";
import type { El } from "./io";
import { C } from "./theme";
import { windowSize, onResize } from "./bom";
import { Header, Sidebar } from "./cards/header";
import { MainPanel, Footer } from "./cards/layout";
import {
    setClock,
    clicks,
    setClicks,
    batches,
    setBatches,
    opsTotal,
    setOpsTotal,
    winLabel,
    setWinLabel,
} from "./state";

/** Two-digit pad. (scriptc has no padStart; `slice(-2)` is unproven there.) */
function pad2(n: number): string {
    let s: string = String(n);
    if (n < 10) s = "0" + s;
    return s;
}

/** Last second rendered in the clock, so a 4 Hz pulse repaints at 1 Hz. */
let lastClockSec: number = -1;

/** Repaint the clock from the timestamp the platform handed the heartbeat. */
function tickClock(nowMs: number): void {
    const total: number = Math.floor(nowMs / 1000);
    if (total === lastClockSec) return;
    lastClockSec = total;
    const hh: number = Math.floor(total / 3600) % 24;
    const mm: number = Math.floor(total / 60) % 60;
    const ss: number = total % 60;
    setClock(pad2(hh) + ":" + pad2(mm) + ":" + pad2(ss));
}

function wireStats(): void {
    onHostEvent(() => setClicks(clicks() + 1));
    // The host pushes geometry whenever it changes; re-read rather than trusting
    // the event payload, so there is one source of truth for the getters.
    onResize(() => setWinLabel(windowSize()));
    // Stats and clock refresh off the platform heartbeat — `setInterval` is
    // refused by scriptc (SC4005) and the host driver thread already ticks.
    // QuickJS drives this from runtime.ts (250 ms, local wall clock); scriptc
    // from gpts_tick (uptime, since a compiled graph has no clock of its own).
    onPulse((nowMs: number) => {
        setBatches(stats.batchesSent);
        setOpsTotal(stats.opsSent);
        tickClock(nowMs);
    });
}

export function App(root: El): void {
    setRootStyle({
        flexDirection: "column",
        gap: 12,
        padding: 16,
        width: "100%",
        height: "100%",
        background: C.bg,
        color: C.textPrimary,
        fontSize: 14,
    });
    wireStats();

    // NOTE: 绑定局部变量再传参（perry 0.5.1520 下实参位置的多层调用会错绑）
    const shell = (
        <div style={{ flexDirection: "row", gap: 12, grow: 1 }}>
            <Sidebar />
            <MainPanel />
        </div>
    );

    appendChild(root, <Header />);
    appendChild(root, shell);
    appendChild(root, <Footer />);
}
