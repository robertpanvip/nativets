/**
 * VList card — virtual scrolling demo: 2000 rows, only a fixed window mounted.
 *
 * The point the card proves: the retained tree stays tiny (≈ overscan +
 * viewport rows) no matter how far you scroll, while the scrollbar still
 * behaves as if all 2000 rows were live — because the spacers own the content
 * height and the host owns the scrolling (see `VList` in kit.tsx).
 */

import { h, text } from "../io";
import type { El, Props } from "../io";
import { C } from "../theme";
import { Card } from "../primitives";
import { VList, VLIST_ROW_H } from "../kit";
import {
    vlistTotal,
    vlistFirst,
    vlistLast,
    vlistMounted,
    vlistScrolls,
    setVlistFirst,
    setVlistLast,
    setVlistMounted,
    setVlistScrolls,
    setVlistTotal,
} from "../state";

const TOTAL_ROWS = 2000;

function vlistLabel(): string {
    return (
        "数据 " +
        vlistTotal() +
        " 行 · 真实窗口 [" +
        vlistFirst() +
        ", " +
        vlistLast() +
        ") · 实挂 " +
        vlistMounted() +
        " 行"
    );
}

/** One row — called only for indices inside the current window. */
function vrow(i: number): El {
    const shade = i % 2 === 0 ? C.card : C.cardAlt;
    return (
        <div
            style={{
                flexDirection: "row",
                alignItems: "center",
                gap: 10,
                height: VLIST_ROW_H,
                paddingX: 10,
                background: shade,
                borderWidth: 1,
                borderColor: C.border,
                borderRadius: 7,
            }}
        >
            {text("row " + String(i).padStart(4, "0"), {
                fontSize: 12,
                fontWeight: "bold",
                color: C.accent,
            })}
            {text("虚拟列表第 " + (i + 1) + " 行 — 不在窗口内即卸载", {
                fontSize: 12,
                color: C.textSecondary,
            })}
        </div>
    );
}

function onVlistScroll(): void {
    // The window numbers themselves are written by the VList callback below;
    // this counter just proves events flow.
    setVlistScrolls(vlistScrolls() + 1);
}

export function VListCard(_props: Props): El {
    setVlistTotal(TOTAL_ROWS);
    return (
        <Card title="虚拟滚动 · vlist 标签 + 前端窗口化 + 宿主滚动条">
            <div style={{ flexDirection: "column", gap: 8 }}>
                <div style={{ flexDirection: "row", justifyContent: "between", alignItems: "center" }}>
                    {text(vlistLabel, { fontSize: 12, color: C.textSecondary })}
                    {text(() => "scroll 事件 " + vlistScrolls() + " 次", {
                        fontSize: 11,
                        color: C.textMuted,
                    })}
                </div>
                <VList
                    items={() => TOTAL_ROWS}
                    row={vrow}
                    height={260}
                    onScroll={function (ev): void {
                        const first = Math.floor((ev.top === undefined ? 0 : ev.top) / VLIST_ROW_H);
                        const rowH = VLIST_ROW_H + 2; // + border
                        const viewport = ev.viewport === undefined ? 0 : ev.viewport;
                        const mounted = Math.ceil((viewport + 12) / rowH) + 8;
                        let last = first + mounted;
                        if (last > TOTAL_ROWS) last = TOTAL_ROWS;
                        setVlistFirst(first);
                        setVlistLast(last);
                        setVlistMounted(mounted);
                        onVlistScroll();
                    }}
                />
            </div>
        </Card>
    );
}
