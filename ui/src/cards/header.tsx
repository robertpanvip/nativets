/**
 * Top bar + left navigation. `nav` is the active section; `NavItem` toggles it.
 */

import { h, text } from "../io";
import type { El, Props, Style } from "../io";
import { C } from "../theme";
import { nav, setNav, clock } from "../state";

export function Header(_props: Props): El {
    return (
        <div
            style={{
                flexDirection: "row",
                alignItems: "center",
                justifyContent: "between",
                padding: 14,
                paddingX: 18,
                background: C.card,
                borderRadius: 12,
                borderWidth: 1,
                borderColor: C.border,
            }}
        >
            <div style={{ flexDirection: "row", alignItems: "center", gap: 12 }}>
                <div
                    style={{
                        background: C.accent,
                        color: "#ffffff",
                        fontSize: 16,
                        fontWeight: "bold",
                        width: 34,
                        height: 34,
                        alignItems: "center",
                        justifyContent: "center",
                        borderRadius: 9,
                    }}
                >
                    P
                </div>
                <div style={{ flexDirection: "column", gap: 2 }}>
                    {text("PerryTS × GPUI", { fontSize: 17, fontWeight: "bold", color: C.textPrimary })}
                    {text("TypeScript 前端 · Perry 原生编译 · GPUI GPU 渲染", {
                        fontSize: 12,
                        color: C.textSecondary,
                    })}
                </div>
            </div>
            {text(clock, { fontSize: 15, fontWeight: "medium", color: C.accent })}
        </div>
    );
}

export function NavItem(props: { label: string }): El {
    const active = nav() === props.label;
    const style: Style = {
        padding: 11,
        paddingX: 14,
        borderRadius: 9,
        fontSize: 14,
        background: active ? C.elevated : C.bg,
        color: active ? C.accent : C.textSecondary,
        borderWidth: 1,
        borderColor: active ? C.accent : C.border,
    };
    return (
        <div style={style} onClick={() => setNav(props.label)}>
            {props.label}
        </div>
    );
}

export function Sidebar(_props: Props): El {
    const items = ["仪表盘", "任务", "指标", "日志", "设置"];
    return (
        <div style={{ flexDirection: "column", gap: 6, width: 178 }}>
            {items.map((label) => (
                <NavItem label={label} />
            ))}
        </div>
    );
}
