/**
 * Event delegation demo: click bubbles up the mount tree (DOM semantics). The
 * parent container receives clicks on children that have no own handler.
 */

import { h, text } from "../io";
import type { El, Props, HostEvent } from "../io";
import { C } from "../theme";
import { Card } from "../primitives";
import { delegLog, setDelegLog } from "../state";

function delegAppend(line: string): void {
    // console.info 同步一份：宿主日志是 E2E 的断言源（signal 只喂 UI）
    console.info("[deleg] " + line);
    // 最新的在上面；限长防爆
    const next = line + "\n" + delegLog();
    setDelegLog(next.length > 400 ? next.slice(0, 400) : next);
}

/**
 * 事件委托演示卡：
 *  - 「冒泡」行：子 div 无自己的 onClick，父容器通过委托拿到点击
 *    （currentTarget 指向父级，target 指向被点的子块）。
 *  - 「拦截」行：子 div 的 onClick 调 stopPropagation()，父容器收不到。
 */
export function DelegationCard(_props: Props): El {
    // 父级委托 handler（挂在外层容器上，两个子区共享）
    const onParentClick = (ev: HostEvent): void => {
        const at = ev.currentTarget === ev.target ? "自身" : "委托(父级)";
        delegAppend("→ 父级收到 " + at + " target=" + String(ev.target));
    };
    // 拦截行子块：处理后截断冒泡
    const onStopChild = (ev: HostEvent): void => {
        delegAppend("● 拦截行子块点击（stopPropagation）");
        if (ev.stopPropagation !== undefined) ev.stopPropagation();
    };
    const onBubbleChild = (ev: HostEvent): void => {
        delegAppend("○ 冒泡行子块点击 target=" + String(ev.target));
    };
    const zone = (labelText: string, child: El): El => (
        <div style={{ flexDirection: "column", gap: 4 }} onClick={onParentClick}>
            {text(labelText, { fontSize: 11, color: C.textMuted })}
            {child}
        </div>
    );
    const bubbleZone = zone("冒泡（父级委托捕获）", (
        <div
            style={{ padding: 8, background: C.bg, borderRadius: 6, borderWidth: 1, borderColor: C.border }}
            onClick={onBubbleChild}
        >
            {text("点我：子先跑，父级随后收到（currentTarget=父级）", { fontSize: 12 })}
        </div>
    ));
    const stopZone = zone("拦截（stopPropagation）", (
        <div
            style={{ padding: 8, background: C.bg, borderRadius: 6, borderWidth: 1, borderColor: C.border }}
            onClick={onStopChild}
        >
            {text("点我：父级不会收到", { fontSize: 12 })}
        </div>
    ));
    return (
        <Card title="事件委托 · click 冒泡（DOM 语义）">
            <div style={{ flexDirection: "column", gap: 8 }}>
                {bubbleZone}
                {stopZone}
                {text(() => (delegLog() === "" ? "（还没有点击）" : delegLog()), {
                    fontSize: 11,
                    color: C.textSecondary,
                })}
            </div>
        </Card>
    );
}
