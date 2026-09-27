/**
 * Canvas 2D card — the frontend records a display list that the host paints on
 * the GPU. Drawn with `ui/src/canvas2d.ts`; nothing here knows about GPUI.
 *
 * Perry 0.5.1520 codegen notes (both verified under this card):
 *   * a call with MORE THAN 3 positional arguments loses params ≥4
 *     (`drawChart(ctx, 430, 150, data, tone)` bound ctx/w/h, passed
 *     `undefined` for values/tone — probed with arg-type breadcrumbs);
 *   * a module-level `const X = new Class()` instance read from another
 *     function of the same module misbinds (坑 #8 family) — so the chart
 *     drawer lives as a class instantiated INSIDE the component body, the
 *     exact shape kit.tsx's CtxFnRef uses, and `draw()` takes 1 argument.
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

/**
 * The chart painter. Instantiated per component (never module-level — see the
 * codegen notes in the file header).
 *
 * Perry 0.5.1520 second finding (probed with debug symbols): calling a
 * cross-module signal getter (`bars()` / `chartTone()` from `state.ts`) from
 * INSIDE a class method returns `undefined` — 坑 #8's imported-binding
 * miscompile, method flavor. So this method reads ONLY its own fields; the
 * draw closure (plain function scope, where signal reads are proven) stages
 * `values` / `tone` onto the instance before invoking `draw`.
 */
class CanvasChart {
    values: number[];
    tone: number;
    constructor() {
        this.values = [];
        this.tone = 0;
    }
    draw(ctx: Ctx): void {
        const values = this.values === undefined ? [] : this.values;
        const tone = this.tone === undefined ? 0 : this.tone;
        const w = CHART_W;
        const h = CHART_H;
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
}

export function CanvasCard(_props: Props): El {
    const chart = new CanvasChart();
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
                        // Signal reads stay in closure scope (method-flavor of
                        // the imported-binding miscompile — see class notes);
                        // hand data to the painter via instance fields.
                        // Perry 0.5.1520 staticlib resolves the imported signal
                        // bindings of THIS module to wrong slots (坑 #8) and
                        // yields `undefined` — degrade to an empty chart instead
                        // of killing the whole mount with a TypeError.
                        const data = bars();
                        const tone = chartTone();
                        chart.values = data === undefined ? [] : data;
                        chart.tone = tone === undefined ? 0 : tone;
                        chart.draw(ctx);
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
                        ◐ 换色调
                    </PillBtn>
                    <div style={{ fontSize: 12, color: C.textMuted }}>
                        <text text={drawStats} />
                    </div>
                </div>
            </div>
        </Card>
    );
}
