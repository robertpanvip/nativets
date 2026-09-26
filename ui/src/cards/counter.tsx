/**
 * Counter card — proves the full loop: button click → signal → mutation → GPU.
 */

import { h, text } from "../io";
import type { El, Props } from "../io";
import { C } from "../theme";
import { Card, PillBtn } from "../primitives";
import { count, setCount } from "../state";

function resetCount(): void {
    setCount(0);
}

export function CounterCard(_props: Props): El {
    return (
        <Card title="计数器 · 状态 → mutation → GPU 回环">
            <div style={{ flexDirection: "row", alignItems: "center", gap: 14 }}>
                <PillBtn bg={C.accentDim} fg={C.textPrimary} onClick={() => setCount(count() - 1)}>
                    -
                </PillBtn>
                {text(() => String(count()), {
                    fontSize: 34,
                    fontWeight: "bold",
                    color: C.textPrimary,
                    width: 90,
                    alignItems: "center",
                })}
                <PillBtn bg={C.accent} fg="#ffffff" onClick={() => setCount(count() + 1)}>
                    +
                </PillBtn>
                <PillBtn bg={C.bg} fg={C.textSecondary} onClick={resetCount}>
                    重置
                </PillBtn>
            </div>
            {text(() => (count() >= 10 ? "★ 状态更新正常，GPU 渲染流畅" : "点击按钮测试细粒度更新"), {
                fontSize: 12,
                color: C.textMuted,
            })}
        </Card>
    );
}
