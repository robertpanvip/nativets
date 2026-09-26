/**
 * Scroll card — host-driven scrolling, host-painted scrollbar, and `scroll` events
 * echoed back into the shared signals.
 */

import { h, text, For } from "../io";
import type { El, Props, HostEvent } from "../io";
import { C } from "../theme";
import { Card } from "../primitives";
import { ScrollArea } from "../kit";
import {
    LOG_ROWS,
    scrollPct,
    scrollTop,
    scrollContent,
    scrollEvents,
    setScrollTop,
    setScrollContent,
    setScrollPct,
    setScrollEvents,
} from "../state";

function LogRow(props: { row: string; index: number }): El {
    const level = props.row.slice(0, 5).trim();
    const color =
        level === "WARN" ? C.amber : level === "DEBUG" ? C.textMuted : level === "ERROR" ? C.red : C.green;
    return (
        <div
            style={{
                flexDirection: "row",
                gap: 10,
                padding: 7,
                paddingX: 9,
                background: C.card,
                borderRadius: 7,
                borderWidth: 1,
                borderColor: C.border,
            }}
        >
            {text(String(props.index + 1).padStart(2, "0"), { fontSize: 11, color: C.textMuted })}
            {text(level, { fontSize: 11, fontWeight: "bold", color: color })}
            {text(props.row.slice(5).trim(), { fontSize: 12, color: C.textSecondary })}
        </div>
    );
}

/** Host-side numbers, recomputed on every `scroll` event. */
function scrollLabel(): string {
    return "滚动 " + scrollPct() + "% · top " + scrollTop() + "px · 内容高 " + scrollContent() + "px";
}

function onLogScroll(ev: HostEvent): void {
    const top = ev.top === undefined ? 0 : ev.top;
    const max = ev.max === undefined ? 0 : ev.max;
    const content = ev.content === undefined ? 0 : ev.content;
    setScrollTop(Math.round(top));
    setScrollContent(Math.round(content));
    setScrollPct(max > 0 ? Math.round((top / max) * 100) : 0);
    setScrollEvents(scrollEvents() + 1);
}

export function ScrollCard(_props: Props): El {
    return (
        <Card title="滚动区 · 宿主滚动 + 宿主绘制滚动条 + scroll 事件">
            <div style={{ flexDirection: "column", gap: 8 }}>
                <div style={{ flexDirection: "row", justifyContent: "between", alignItems: "center" }}>
                    {text(scrollLabel, { fontSize: 12, color: C.textSecondary })}
                    {text(() => "scroll 事件 " + scrollEvents() + " 次", { fontSize: 11, color: C.textMuted })}
                </div>
                <ScrollArea height={168} onScroll={onLogScroll}>
                    {For(
                        () => LOG_ROWS,
                        (row, i) => <LogRow row={row} index={i} />,
                    )}
                </ScrollArea>
            </div>
        </Card>
    );
}
