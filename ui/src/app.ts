/**
 * Demo app — PerryTS × GPUI desktop dashboard.
 * 全部 UI 由 TypeScript 编写（Solid 风格响应式），经 Perry 编译为原生
 * 可执行文件，通过 mutation 协议驱动 GPUI 渲染。
 */

import {
    createRoot,
    createSignal,
    h,
    text,
    For,
    onHostEvent,
    setRootStyle,
    appendChild,
    stats,
} from "./runtime";
import type { El } from "./runtime";

// ---------------------------------------------------------------------------
// palette
// ---------------------------------------------------------------------------

const C = {
    bg: "#0f1117",
    card: "#161b26",
    cardAlt: "#141926",
    elevated: "#1c2333",
    border: "#262d3d",
    accent: "#4f8cff",
    accentDim: "#2a3145",
    textPrimary: "#e8eaf2",
    textSecondary: "#8b93a7",
    textMuted: "#56607a",
    green: "#3dd68c",
    red: "#e5484d",
    amber: "#f5a623",
};

// ---------------------------------------------------------------------------
// state
// ---------------------------------------------------------------------------

const [count, setCount] = createSignal(0);
const [nav, setNav] = createSignal("仪表盘");
const [clicks, setClicks] = createSignal(0);
const [batches, setBatches] = createSignal(0);
const [opsTotal, setOpsTotal] = createSignal(0);
const [clock, setClock] = createSignal("--:--:--");
const [tasks, setTasks] = createSignal<Task[]>([
    { id: 1, title: "Perry 编译 TS → 原生二进制", done: true },
    { id: 2, title: "GPUI retained tree 渲染", done: true },
    { id: 3, title: "mutation 协议 stdio 回环", done: false },
    { id: 4, title: "Solid 风格细粒度更新", done: false },
]);

interface Task {
    id: number;
    title: string;
    done: boolean;
}

let nextTaskId = 5;
const samples = [
    "验证 Windows Direct3D 后端",
    "对比 napi 桥与 stdio 桥延迟",
    "补 TextInput 协议扩展",
    "写 README 架构图",
];

// ---------------------------------------------------------------------------
// shared components
// ---------------------------------------------------------------------------

function card(title: string, ...children: El[]): El {
    return h("div", {
        flexDirection: "column",
        gap: 10,
        padding: 16,
        background: C.card,
        borderRadius: 12,
        borderWidth: 1,
        borderColor: C.border,
        grow: 1,
    },
        text(title, { fontSize: 12, color: C.textSecondary }),
        ...children
    );
}

function statCard(label: string, value: () => string, color: string): El {
    return h("div", {
        flexDirection: "column",
        gap: 6,
        padding: 14,
        background: C.cardAlt,
        borderRadius: 10,
        borderWidth: 1,
        borderColor: C.border,
        grow: 1,
    },
        text(label, { fontSize: 11, color: C.textMuted }),
        text(value, { fontSize: 24, fontWeight: "bold", color: color })
    );
}

function pillBtn(label: string, bg: string, fg: string, onClick: () => void): El {
    return h("div", {
        background: bg,
        color: fg,
        fontSize: 14,
        fontWeight: "medium",
        padding: 10,
        paddingX: 16,
        borderRadius: 8,
        onClick: onClick,
    }, label);
}

// ---------------------------------------------------------------------------
// sections
// ---------------------------------------------------------------------------

function header(): El {
    return h("div", {
        flexDirection: "row",
        alignItems: "center",
        justifyContent: "between",
        padding: 14,
        paddingX: 18,
        background: C.card,
        borderRadius: 12,
        borderWidth: 1,
        borderColor: C.border,
    },
        h("div", { flexDirection: "row", alignItems: "center", gap: 12 },
            h("div", {
                text: "P",
                background: C.accent,
                color: "#ffffff",
                fontSize: 16,
                fontWeight: "bold",
                width: 34,
                height: 34,
                alignItems: "center",
                justifyContent: "center",
                borderRadius: 9,
            }),
            h("div", { flexDirection: "column", gap: 2 },
                text("PerryTS × GPUI", { fontSize: 17, fontWeight: "bold", color: C.textPrimary }),
                text("TypeScript 前端 · Perry 原生编译 · GPUI GPU 渲染", { fontSize: 12, color: C.textSecondary })
            )
        ),
        text(clock, { fontSize: 15, fontWeight: "medium", color: C.accent })
    );
}

function navItem(label: string): El {
    const active = nav() === label;
    return h("div", {
        padding: 11,
        paddingX: 14,
        borderRadius: 9,
        fontSize: 14,
        background: active ? C.elevated : C.bg,
        color: active ? C.accent : C.textSecondary,
        borderWidth: 1,
        borderColor: active ? C.accent : C.border,
        onClick: () => setNav(label),
    }, label);
}

function sidebar(): El {
    const items = ["仪表盘", "任务", "指标", "日志", "设置"];
    return h("div", {
        flexDirection: "column",
        gap: 6,
        width: 178,
    }, For(() => items, navItem));
}

function counterCard(): El {
    return card("计数器 · 状态 → mutation → GPU 回环",
        h("div", { flexDirection: "row", alignItems: "center", gap: 14 },
            pillBtn("-", C.accentDim, C.textPrimary, () => setCount(count() - 1)),
            text(count, { fontSize: 34, fontWeight: "bold", color: C.textPrimary, width: 90, alignItems: "center" }),
            pillBtn("+", C.accent, "#ffffff", () => setCount(count() + 1)),
            pillBtn("重置", C.bg, C.textSecondary, () => setCount(0))
        ),
        text(() => (count() >= 10 ? "★ 状态更新正常，GPU 渲染流畅" : "点击按钮测试细粒度更新"), {
            fontSize: 12,
            color: C.textMuted,
        })
    );
}

function taskRow(t: Task): El {
    return h("div", {
        flexDirection: "row",
        alignItems: "center",
        justifyContent: "between",
        padding: 10,
        paddingX: 12,
        background: C.cardAlt,
        borderRadius: 8,
        borderWidth: 1,
        borderColor: C.border,
    },
        h("div", { flexDirection: "row", alignItems: "center", gap: 10 },
            h("div", {
                text: t.done ? "√" : "○",
                color: t.done ? C.green : C.textMuted,
                fontSize: 15,
                fontWeight: "bold",
                paddingX: 4,
                onClick: () => toggleTask(t.id),
            }),
            text(t.title, {
                fontSize: 13,
                color: t.done ? C.textMuted : C.textPrimary,
            })
        ),
        h("div", {
            text: "删除",
            color: C.red,
            fontSize: 12,
            paddingX: 8,
            paddingY: 4,
            borderRadius: 6,
            onClick: () => removeTask(t.id),
        })
    );
}

function tasksCard(): El {
    return card("任务列表 · For 动态渲染 + 事件回传",
        h("div", { flexDirection: "column", gap: 8 }, For(tasks, taskRow)),
        h("div", { flexDirection: "row", gap: 10, alignItems: "center" },
            pillBtn("＋ 快速添加", C.elevated, C.accent, addTask),
            text(() => "已完成 " + doneCount() + " / " + tasks().length, { fontSize: 12, color: C.textMuted })
        )
    );
}

function statsRow(): El {
    return h("div", { flexDirection: "row", gap: 12 },
        statCard("点击事件回传", () => String(clicks()), C.green),
        statCard("mutation 批", () => String(batches()), C.accent),
        statCard("累计 mutation", () => String(opsTotal()), C.amber)
    );
}

function mainPanel(): El {
    return h("div", { flexDirection: "column", gap: 14, grow: 1 },
        statsRow(),
        counterCard(),
        tasksCard()
    );
}

function footer(): El {
    return h("div", {
        flexDirection: "row",
        justifyContent: "between",
        alignItems: "center",
        padding: 10,
        paddingX: 16,
        background: C.cardAlt,
        borderRadius: 10,
        borderWidth: 1,
        borderColor: C.border,
    },
        text(() => "frontend: " + mode + " · protocol 1", { fontSize: 12, color: C.textMuted }),
        text(() => "navigation: " + nav(), { fontSize: 12, color: C.textSecondary })
    );
}

// ---------------------------------------------------------------------------
// actions
// ---------------------------------------------------------------------------

function toggleTask(id: number): void {
    // NOTE: 先绑定局部再调用 —— 避免 f(g(h())) 深嵌套（perry 下行为异常）
    const cur = tasks();
    setTasks(cur.map((t) => (t.id === id ? { id: t.id, title: t.title, done: !t.done } : t)));
}

function removeTask(id: number): void {
    const cur = tasks();
    setTasks(cur.filter((t) => t.id !== id));
}

function addTask(): void {
    const title = samples[(nextTaskId - 5) % samples.length];
    const cur = tasks();
    setTasks(cur.concat([{ id: nextTaskId++, title: title, done: false }]));
}

function doneCount(): number {
    return tasks().filter((t) => t.done).length;
}

const mode = typeof (process as any).env.PERRY_DEV === "undefined" ? "perry-native" : "node-dev";

// ---------------------------------------------------------------------------
// wiring
// ---------------------------------------------------------------------------

function wireStats(): void {
    onHostEvent(() => setClicks(clicks() + 1));
    setInterval(() => {
        setBatches(stats.batchesSent);
        setOpsTotal(stats.opsSent);
    }, 500);
    setInterval(() => {
        const d = new Date();
        const p = (n: number) => ("0" + n).slice(-2);
        setClock(p(d.getHours()) + ":" + p(d.getMinutes()) + ":" + p(d.getSeconds()));
    }, 1000);
}

// ---------------------------------------------------------------------------
// app entry
// ---------------------------------------------------------------------------

export function App(root: El): void {
    setRootStyle({
        flexDirection: "column",
        gap: 12,
        padding: 16,
        width: "100%",
        height: "100%",
        background: C.bg,
        color: C.textPrimary,
        fontSize: 14,
    });
    wireStats();
    appendChild(root, header());
    appendChild(root, h("div", { flexDirection: "row", gap: 12, grow: 1 }, sidebar(), mainPanel()));
    appendChild(root, footer());
}
