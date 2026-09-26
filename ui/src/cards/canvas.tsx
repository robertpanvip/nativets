/**
 * Canvas 2D card — the frontend records a display list that the host paints on
 * the GPU. Drawn with `ui/src/canvas2d.ts`; nothing here knows about GPUI.
 */

import { h, text } from "../io";
import type { El, Props } from "../io";
import { C } from "../theme";
import { Card, PillBtn } from "../primitives";
import { CanvasView } from "../kit";
import type { Ctx } from "../canvas2d";
import { bars, setBars, redraws, setRedraws, cmdCount, setCmdCount, chartTone, setChartTone } from "../state";

const CHART_W = 430;
const CHART_H = 150;

/**
 * Drawn with `ui/src/canvas2d.ts`: every call below becomes one command in the
 * node's display list (`fillRect` → `rect`, the path API → `poly`, …). Nothing
 * here knows about GPUI — and the same function would work on a backend that had
 * a real canvas, which is the point of keeping the API familiar.
 */
function drawChart(ctx: Ctx, w: number, h: number, values: number[], tone: number): void {
    const pad = 26;
    const plotW = w - pad * 2;
    const plotH = h - pad - 24;
    const accent = tone === 0 ? C.accent : C.green;

    let max = 1;
    for (let i = 0; i < values.length; i++) {
        if (values[i] > max) max = values[i];
    }
    const gap = 9;
    const bw = (plotW - gap * (values.length - 1)) / values.length;

    // baseline
    ctx.fillStyle = C.border;
    ctx.fillRect(pad, pad + plotH, plotW, 1, 0);

    const cx: number[] = [];
    const cy: number[] = [];
    for (let i = 0; i < values.length; i++) {
        const bh = Math.round((values[i] / max) * plotH);
        const x = pad + i * (bw + gap);
        const y = pad + plotH - bh;
        const isLast = i === values.length - 1;
        ctx.fillStyle = isLast ? C.cyan : accent;
        ctx.fillRect(x, y, bw, bh, 3);
        ctx.fontSize = 10;
        ctx.fillStyle = C.textMuted;
        ctx.text(String(values[i]), x + 1, y - 13, undefined);
        cx.push(x + bw / 2);
        cy.push(y);
    }

    // trend line through the bar tops — the path API half of the recorder
    if (cx.length > 0) {
        ctx.beginPath();
        ctx.moveTo(cx[0], cy[0]);
        for (let i = 1; i < cx.length; i++) {
            ctx.lineTo(cx[i], cy[i]);
        }
        ctx.stroke({ stroke: C.violet, line: 2 });
        for (let i = 0; i < cx.length; i++) {
            ctx.circle(cx[i], cy[i], 2.5, { fill: C.violet });
        }
    }

    ctx.fontSize = 11;
    ctx.fillStyle = C.textSecondary;
    ctx.text("mutation 批 · 每 5s 采样", pad, 6, undefined);
    ctx.fillStyle = C.textMuted;
    ctx.text("sample 1-8", pad + plotW - 60, 6, undefined);
}

/** Deterministic pseudo-random data: a fixed seed makes a redraw reproducible,
 *  which is what lets the ops stream be diffed between runs (and backends). */
let seed = 7;
function nextBars(): number[] {
    const out: number[] = [];
    for (let i = 0; i < 8; i++) {
        seed = (seed * 1103515245 + 12345) % 2147483648;
        out.push(24 + (seed % 72));
    }
    return out;
}

function drawStats(): string {
    return "重绘 " + redraws() + " 次 · 显示列表 " + cmdCount() + " 条";
}

function reshuffleBars(): void {
    setBars(nextBars());
}

export function CanvasCard(_props: Props): El {
    return (
        <Card title="Canvas 2D · 前端录制显示列表 → 宿主 GPU 绘制">
            <div style={{ flexDirection: "row", gap: 16, alignItems: "start" }}>
                <CanvasView
                    width={CHART_W}
                    height={CHART_H}
                    style={{
                        background: C.cardAlt,
                        borderRadius: 10,
                        borderWidth: 1,
                        borderColor: C.border,
                    }}
                    draw={(ctx: Ctx) => {
                        // Bind first (README 坑 #3: nested calls in argument
                        // positions misbehave under perry).
                        const data = bars();
                        const tone = chartTone();
                        drawChart(ctx, CHART_W, CHART_H, data, tone);
                    }}
                    onPresent={(n: number, r: number) => {
                        // Both args come from the canvas's own effect; reading a
                        // signal here that the effect writes would self-trigger.
                        setCmdCount(n);
                        setRedraws(r);
                    }}
                />
                <div style={{ flexDirection: "column", gap: 8, grow: 1 }}>
                    <PillBtn bg={C.elevated} fg={C.accent} onClick={reshuffleBars}>
                        ⟳ 换一组数据
                    </PillBtn>
                    <PillBtn
                        bg={C.bg}
                        fg={C.textSecondary}
                        onClick={() => setChartTone(chartTone() === 0 ? 1 : 0)}
                    >
                        切换配色
                    </PillBtn>
                    {text(drawStats, { fontSize: 11, color: C.textMuted })}
                    {text("改数据或换配色都会自动重绘：绘制函数读到的 signal 就是它的依赖。", {
                        fontSize: 11,
                        color: C.textMuted,
                    })}
                </div>
            </div>
        </Card>
    );
}
