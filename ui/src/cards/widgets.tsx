/**
 * Native component card — Progress / Slider / Rating / Spinner.
 *
 * Progress and Slider are controlled: the `value` getter drives a `setValue` op
 * and the widget's events echo back. Spinner is a pure animation; Rating keeps
 * its own star state host-side and emits `change`.
 */

import { h, text, Show } from "../io";
import type { El, Props } from "../io";
import { C } from "../theme";
import { Card, PillBtn } from "../primitives";
import { Button, Progress, Slider, Rating, Spinner } from "../kit";
import {
    progress,
    setProgress,
    sliderVal,
    setSliderVal,
    sliderLog,
    setSliderLog,
    stars,
    setStars,
    spinnerRun,
    setSpinnerRun,
} from "../state";

export function WidgetsCard(_props: Props): El {
    const stepProgress = (d: number): void => {
        const next = Math.max(0, Math.min(100, progress() + d));
        setProgress(next);
    };
    const onSliderInput = (v: number): void => {
        setSliderVal(v);
        setSliderLog("input " + String(v));
    };
    const onSliderChange = (v: number): void => {
        setSliderVal(v);
        setSliderLog("change " + String(v));
    };
    return (
        <Card title="原生组件 · Progress / Slider / Rating / Spinner">
            <div style={{ flexDirection: "column", gap: 12 }}>
                {/* Progress：确定性 + 不定态 */}
                <div style={{ flexDirection: "column", gap: 6 }}>
                    {text(() => "进度 " + String(progress()) + "%", { fontSize: 12, color: C.textSecondary })}
                    <Progress value={progress} height={8} style={{ width: "100%" }} />
                    <div style={{ flexDirection: "row", gap: 8, alignItems: "center" }}>
                        <Button label="-10" onClick={() => stepProgress(-10)} style={{ height: 26, paddingX: 10 }} />
                        <Button label="+10" onClick={() => stepProgress(10)} style={{ height: 26, paddingX: 10 }} />
                        <Button label="重置" onClick={() => setProgress(0)} style={{ height: 26, paddingX: 10 }} />
                        {Show(spinnerRun, () => (
                            <div style={{ flexDirection: "row", gap: 8, alignItems: "center", grow: 1 }}>
                                <Progress value={() => 0} loading height={6} style={{ width: "100%" }} />
                            </div>
                        ))}
                    </div>
                </div>
                {/* Slider：拖动 */}
                <div style={{ flexDirection: "column", gap: 6 }}>
                    <div style={{ flexDirection: "row", gap: 10, alignItems: "center" }}>
                        {text("音量", { fontSize: 12, color: C.textSecondary })}
                        <Slider
                            value={sliderVal}
                            min={0}
                            max={100}
                            step={1}
                            width={240}
                            onInput={onSliderInput}
                            onChange={onSliderChange}
                        />
                        {text(() => String(sliderVal()), { fontSize: 13, color: C.textPrimary })}
                    </div>
                    {text(() => (sliderLog() === "" ? "（拖动滑块）" : sliderLog()), {
                        fontSize: 11,
                        color: C.textMuted,
                    })}
                </div>
                {/* Rating + Spinner */}
                <div style={{ flexDirection: "row", gap: 24, alignItems: "center" }}>
                    <div style={{ flexDirection: "column", gap: 4 }}>
                        {text(() => "评分 " + String(stars()) + "/5", { fontSize: 12, color: C.textSecondary })}
                        <Rating value={stars} onChange={(v: number) => setStars(v)} />
                    </div>
                    <div style={{ flexDirection: "column", gap: 4 }}>
                        {text("加载中", { fontSize: 12, color: C.textSecondary })}
                        <Spinner size={18} />
                    </div>
                </div>
            </div>
        </Card>
    );
}
