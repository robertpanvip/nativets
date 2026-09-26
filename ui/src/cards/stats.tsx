/**
 * Top stats row — click / mutation-batch / cumulative-mutation counters, read live
 * from the shared signals.
 */

import { h } from "../io";
import type { El } from "../io";
import { C } from "../theme";
import { StatCard } from "../primitives";
import { clicks, batches, opsTotal } from "../state";

export function StatsRow(_props: unknown): El {
    return (
        <div style={{ flexDirection: "row", gap: 12 }}>
            <StatCard label="点击事件回传" value={() => String(clicks())} color={C.green} />
            <StatCard label="mutation 批" value={() => String(batches())} color={C.accent} />
            <StatCard label="累计 mutation" value={() => String(opsTotal())} color={C.amber} />
        </div>
    );
}
