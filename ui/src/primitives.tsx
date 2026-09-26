/**
 * Shared presentational primitives used across the dashboard cards.
 *
 * These are pure rendering helpers — no application state lives here, so any card
 * can import them without pulling in a signal graph. The palette comes from
 * `./theme` so the cards and the kit's controls cannot drift apart.
 */

import { h, text } from "./io";
import type { Child, El, Style } from "./io";
import { C } from "./theme";

export function Card(props: { title: string; children?: Child }): El {
    return (
        <div
            style={{
                flexDirection: "column",
                gap: 10,
                padding: 16,
                background: C.card,
                borderRadius: 12,
                borderWidth: 1,
                borderColor: C.border,
                // Cards keep their content height: they live inside the main
                // scroller, so growing would make them fight for the viewport
                // and shrinking would squash their content into the card above.
                // (The host also pins scroller children — see `build_node`.)
                shrink: 0,
            }}
        >
            {text(props.title, { fontSize: 12, color: C.textSecondary })}
            {props.children}
        </div>
    );
}

export function StatCard(props: { label: string; value: () => string; color: string }): El {
    return (
        <div
            style={{
                flexDirection: "column",
                gap: 6,
                padding: 14,
                background: C.cardAlt,
                borderRadius: 10,
                borderWidth: 1,
                borderColor: C.border,
                grow: 1,
            }}
        >
            {text(props.label, { fontSize: 11, color: C.textMuted })}
            {text(props.value, { fontSize: 24, fontWeight: "bold", color: props.color })}
        </div>
    );
}

/** `bg` / `fg` are *component* props — they are not style keys, which is the
 *  whole point of keeping styling behind `style`. The native `button` tag is
 *  rendered by the host as a real gpui-component `Button`; `bg`/`fg` cross as
 *  style overrides on the same channel. */
export function PillBtn(props: { bg: string; fg: string; onClick: () => void; children?: Child }): El {
    const style: Style = {
        background: props.bg,
        color: props.fg,
        fontSize: 14,
        fontWeight: "medium",
    };
    return (
        <button style={style} onClick={props.onClick}>
            {props.children}
        </button>
    );
}

/** Label + reactive value, the BOM card's unit of layout. */
export function Field(props: { label: string; value: () => string }): El {
    return (
        <div style={{ flexDirection: "column", gap: 4, grow: 1 }}>
            {text(props.label, { fontSize: 11, color: C.textMuted })}
            {text(props.value, { fontSize: 13, color: C.textPrimary })}
        </div>
    );
}

/** `██░░` — a bar that can only change via a reactive text update. */
export function bar(n: number, total: number): string {
    const filled = Math.round((n / total) * 20);
    return "█".repeat(filled) + "░".repeat(20 - filled);
}
