/**
 * Page layout: the scrolling main panel (every card stacked) and the footer.
 */

import { h, text } from "../io";
import type { El, Props } from "../io";
import { C } from "../theme";
import { ScrollArea } from "../kit";
import { CounterCard } from "./counter";
import { TasksCard } from "./tasks";
import { BomCard } from "./bom";
import { FormCard } from "./form";
import { CanvasCard } from "./canvas";
import { ScrollCard } from "./scroll";
import { DelegationCard } from "./delegation";
import { WidgetsCard } from "./widgets";
import { ExtendedComponentsCard } from "./extended";
import { StatsRow } from "./stats";
import { outerScrolls, setOuterScrolls, nav, mode } from "../state";

export function MainPanel(_props: Props): El {
    return (
        <div style={{ flexDirection: "column", gap: 14, grow: 1, height: "100%", minWidth: 0 }}>
            {/* The whole dashboard body scrolls, not just the cards below the
                fold: a scroll container needs a bounded height (the same rule as
                CSS) and `height: "100%"` inside the sized main panel is what
                gives it one. Scrolling everything keeps the viewport tall enough
                to be worth a scrollbar. */}
            <ScrollArea
                grow
                onScroll={() => setOuterScrolls(outerScrolls() + 1)}
                style={{
                    gap: 14,
                    padding: 0,
                    paddingRight: 8,
                    background: C.bg,
                    borderRadius: 0,
                    borderWidth: 0,
                }}
            >
                <FormCard />
                <StatsRow />
                <CounterCard />
                <BomCard />
                <CanvasCard />
                <ScrollCard />
                <DelegationCard />
                <WidgetsCard />
                <ExtendedComponentsCard />
                <TasksCard />
            </ScrollArea>
        </div>
    );
}

export function Footer(_props: Props): El {
    return (
        <div
            style={{
                flexDirection: "row",
                justifyContent: "between",
                alignItems: "center",
                padding: 10,
                paddingX: 16,
                background: C.cardAlt,
                borderRadius: 10,
                borderWidth: 1,
                borderColor: C.border,
            }}
        >
            {text(() => "frontend: " + mode + " · protocol 1", { fontSize: 12, color: C.textMuted })}
            {text(() => "navigation: " + nav(), { fontSize: 12, color: C.textSecondary })}
            {text(() => "outer scroll " + outerScrolls() + "×", { fontSize: 12, color: C.textMuted })}
        </div>
    );
}
