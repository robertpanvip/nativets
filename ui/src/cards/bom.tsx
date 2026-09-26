/**
 * BOM card — everything the host's BOM provides, on display.
 *
 * Note the shape of the frame demo: `requestAnimationFrame` is paced by the
 * engine tick (see `bootstrap.js`), so it runs at a stable 50Hz and stops the
 * moment the last callback is not re-registered.
 */

import { h, text } from "../io";
import type { El, Props, Style } from "../io";
import { C } from "../theme";
import { Card, PillBtn, Field } from "../primitives";
import {
    windowSize,
    dpr,
    userAgent,
    href,
    uptimeLabel,
    newUuid,
    raf,
    notify,
} from "../bom";
import { winLabel, uuidText, setUuidText, frame, frameRunning, setFrameRunning, setFrame, mountMs, FRAME_TOTAL } from "../state";

const ROW: Style = { flexDirection: "row", gap: 18 };

/** `██░░` frame progress bar. */
function progressText(): string {
    const n = frame();
    return bar(n, FRAME_TOTAL);
}

/** `第 22 / 60 帧` — the counter next to the bar. */
function frameLabel(): string {
    const n = frame();
    return "第 " + n + " / " + FRAME_TOTAL + " 帧";
}

/** Mint a fresh v4 UUID into the card. */
function newUuidClick(): void {
    setUuidText(newUuid());
}

/** Row 1 — window metrics, all read live from the BOM getters. */
function BomRowMetrics(_props: Props): El {
    return (
        <div style={ROW}>
            <Field label="window 尺寸（随 resize 更新）" value={winLabel} />
            <Field label="devicePixelRatio" value={dpr} />
            <Field label="performance.now() @挂载" value={() => uptimeLabel(mountMs)} />
        </div>
    );
}

/** Row 2 — environment identity: where this bundle thinks it is running. */
function BomRowEnv(_props: Props): El {
    return (
        <div style={ROW}>
            <Field label="location.href" value={href} />
            <Field label="navigator.userAgent" value={userAgent} />
            <Field label="crypto.randomUUID()" value={uuidText} />
        </div>
    );
}

/**
 * Row 3 — the interactive parts: a finite rAF burst, a fresh UUID, the host
 * alert, and a text progress bar driven by the frame signal.
 *
 * Split out of `BomCard` on purpose. As one function `BomCard`'s body was a
 * single ~30-deep nested `h()` expression, and perry's `--output-type
 * staticlib` backend silently dropped the whole component (0 of its ops
 * emitted, no error anywhere). Small components keep every expression shallow.
 */
function BomRowActions(_props: Props): El {
    return (
        <div style={{ flexDirection: "row", alignItems: "center", gap: 12 }}>
            <PillBtn bg={C.elevated} fg={C.accent} onClick={runFrames}>
                {text(() => (frameRunning() ? "… 动画中" : "▶ 帧动画 rAF"))}
            </PillBtn>
            <PillBtn bg={C.bg} fg={C.textSecondary} onClick={newUuidClick}>
                新 UUID
            </PillBtn>
            <PillBtn bg={C.bg} fg={C.textSecondary} onClick={demoAlert}>
                alert 弹窗
            </PillBtn>
            {text(progressText, { fontSize: 13, color: C.accent })}
            {text(frameLabel, { fontSize: 11, color: C.textMuted })}
        </div>
    );
}

export function BomCard(_props: Props): El {
    return (
        <Card title="BOM · 宿主注入的浏览器环境">
            <BomRowMetrics />
            <BomRowEnv />
            <BomRowActions />
        </Card>
    );
}

/**
 * Run a finite burst of animation frames.
 *
 * Deliberately *finite*: a permanent rAF loop would keep one mutation batch per
 * frame crossing the transport forever, which would drown the mutation stats
 * below and keep the engine thread from ever parking.
 */
function runFrames(): void {
    if (frameRunning()) return;
    setFrameRunning(true);
    setFrame(0);
    const step = (): void => {
        const n = frame() + 1;
        setFrame(n);
        if (n < FRAME_TOTAL) raf(step);
        else setFrameRunning(false);
    };
    // `raf` reports whether it actually scheduled anything: without a BOM
    // nothing will ever tick, so do not leave the button in "animating".
    if (!raf(step)) setFrameRunning(false);
}

function demoAlert(): void {
    notify("这是宿主绘制的 GPUI 模态弹窗（alert）—— 前端协议里没有它。");
}

/** Local `bar` helper (kept here to avoid pulling it through `primitives`). */
function bar(n: number, total: number): string {
    const filled = Math.round((n / total) * 20);
    return "█".repeat(filled) + "░".repeat(20 - filled);
}
