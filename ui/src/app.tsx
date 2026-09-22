/**
 * Demo app — nativets × GPUI desktop dashboard.
 *
 * Authored in TSX. The build compiles JSX directly to the runtime's `h()`
 * factory (`--jsx=transform --jsx-factory=h --jsx-fragment=Fragment`), so JSX
 * is pure sugar over the same mutation protocol:
 *
 *     <div padding={16}>hi {n}</div>   ⟺   h("div", { padding: 16 }, "hi ", n)
 *
 * A capitalised tag is a component: `<Card title="…">…</Card>` becomes
 * `h(Card, { title }, …)`, with children delivered as `props.children`.
 * Style keys stay flat (they *are* the style object the host applies) — there
 * is no nested `style={{…}}` layer.
 */

import {
    createSignal,
    h,
    text,
    For,
    onHostEvent,
    setRootStyle,
    appendChild,
    stats,
} from "./runtime";
import type { El, Child } from "./runtime";

// `h` is referenced by the JSX transform itself, not by hand-written calls —
// keep it imported even though no `h(` appears in this file.

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

interface Task {
    id: number;
    title: string;
    done: boolean;
}

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

function Card(props: { title: string; children?: Child }): El {
    return (
        <div
            flexDirection="column"
            gap={10}
            padding={16}
            background={C.card}
            borderRadius={12}
            borderWidth={1}
            borderColor={C.border}
            grow={1}
        >
            {text(props.title, { fontSize: 12, color: C.textSecondary })}
            {props.children}
        </div>
    );
}

function StatCard(props: { label: string; value: () => string; color: string }): El {
    return (
        <div
            flexDirection="column"
            gap={6}
            padding={14}
            background={C.cardAlt}
            borderRadius={10}
            borderWidth={1}
            borderColor={C.border}
            grow={1}
        >
            {text(props.label, { fontSize: 11, color: C.textMuted })}
            {text(props.value, { fontSize: 24, fontWeight: "bold", color: props.color })}
        </div>
    );
}

function PillBtn(props: { bg: string; fg: string; onClick: () => void; children?: Child }): El {
    return (
        <div
            background={props.bg}
            color={props.fg}
            fontSize={14}
            fontWeight="medium"
            padding={10}
            paddingX={16}
            borderRadius={8}
            onClick={props.onClick}
        >
            {props.children}
        </div>
    );
}

// ---------------------------------------------------------------------------
// sections
// ---------------------------------------------------------------------------

function Header(): El {
    return (
        <div
            flexDirection="row"
            alignItems="center"
            justifyContent="between"
            padding={14}
            paddingX={18}
            background={C.card}
            borderRadius={12}
            borderWidth={1}
            borderColor={C.border}
        >
            <div flexDirection="row" alignItems="center" gap={12}>
                <div
                    background={C.accent}
                    color="#ffffff"
                    fontSize={16}
                    fontWeight="bold"
                    width={34}
                    height={34}
                    alignItems="center"
                    justifyContent="center"
                    borderRadius={9}
                >
                    P
                </div>
                <div flexDirection="column" gap={2}>
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

function NavItem(props: { label: string }): El {
    const active = nav() === props.label;
    return (
        <div
            padding={11}
            paddingX={14}
            borderRadius={9}
            fontSize={14}
            background={active ? C.elevated : C.bg}
            color={active ? C.accent : C.textSecondary}
            borderWidth={1}
            borderColor={active ? C.accent : C.border}
            onClick={() => setNav(props.label)}
        >
            {props.label}
        </div>
    );
}

function Sidebar(): El {
    const items = ["仪表盘", "任务", "指标", "日志", "设置"];
    return (
        <div flexDirection="column" gap={6} width={178}>
            {For(
                () => items,
                (label) => <NavItem label={label} />
            )}
        </div>
    );
}

function CounterCard(): El {
    return (
        <Card title="计数器 · 状态 → mutation → GPU 回环">
            <div flexDirection="row" alignItems="center" gap={14}>
                <PillBtn bg={C.accentDim} fg={C.textPrimary} onClick={() => setCount(count() - 1)}>
                    -
                </PillBtn>
                {text(count, {
                    fontSize: 34,
                    fontWeight: "bold",
                    color: C.textPrimary,
                    width: 90,
                    alignItems: "center",
                })}
                <PillBtn bg={C.accent} fg="#ffffff" onClick={() => setCount(count() + 1)}>
                    +
                </PillBtn>
                <PillBtn bg={C.bg} fg={C.textSecondary} onClick={() => setCount(0)}>
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

function TaskRow(props: { t: Task }): El {
    const t = props.t;
    return (
        <div
            flexDirection="row"
            alignItems="center"
            justifyContent="between"
            padding={10}
            paddingX={12}
            background={C.cardAlt}
            borderRadius={8}
            borderWidth={1}
            borderColor={C.border}
        >
            <div flexDirection="row" alignItems="center" gap={10}>
                <div
                    color={t.done ? C.green : C.textMuted}
                    fontSize={15}
                    fontWeight="bold"
                    paddingX={4}
                    onClick={() => toggleTask(t.id)}
                >
                    {t.done ? "√" : "○"}
                </div>
                {text(t.title, { fontSize: 13, color: t.done ? C.textMuted : C.textPrimary })}
            </div>
            <div
                color={C.red}
                fontSize={12}
                paddingX={8}
                paddingY={4}
                borderRadius={6}
                onClick={() => removeTask(t.id)}
            >
                删除
            </div>
        </div>
    );
}

function TasksCard(): El {
    return (
        <Card title="任务列表 · For 动态渲染 + 事件回传">
            <div flexDirection="column" gap={8}>
                {For(
                    tasks,
                    (t) => <TaskRow t={t} />
                )}
            </div>
            <div flexDirection="row" gap={10} alignItems="center">
                <PillBtn bg={C.elevated} fg={C.accent} onClick={addTask}>
                    ＋ 快速添加
                </PillBtn>
                {text(() => "已完成 " + doneCount() + " / " + tasks().length, {
                    fontSize: 12,
                    color: C.textMuted,
                })}
            </div>
        </Card>
    );
}

function StatsRow(): El {
    return (
        <div flexDirection="row" gap={12}>
            <StatCard label="点击事件回传" value={() => String(clicks())} color={C.green} />
            <StatCard label="mutation 批" value={() => String(batches())} color={C.accent} />
            <StatCard label="累计 mutation" value={() => String(opsTotal())} color={C.amber} />
        </div>
    );
}

function MainPanel(): El {
    return (
        <div flexDirection="column" gap={14} grow={1}>
            <StatsRow />
            <CounterCard />
            <TasksCard />
        </div>
    );
}

function Footer(): El {
    return (
        <div
            flexDirection="row"
            justifyContent="between"
            alignItems="center"
            padding={10}
            paddingX={16}
            background={C.cardAlt}
            borderRadius={10}
            borderWidth={1}
            borderColor={C.border}
        >
            {text(() => "frontend: " + mode + " · protocol 1", { fontSize: 12, color: C.textMuted })}
            {text(() => "navigation: " + nav(), { fontSize: 12, color: C.textSecondary })}
        </div>
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

/**
 * Which runtime is hosting this bundle? Reported in the status bar.
 *   quickjs — the host injects `GPUI_TS_ENGINE=quickjs` into the process shim
 *   node-dev — `PERRY_DEV` is set by the dev launcher
 *   perry-native — anything else (the embedded Perry staticlib)
 */
const mode = (function (): string {
    if (typeof process !== "undefined" && process.env) {
        if (process.env.GPUI_TS_ENGINE === "quickjs") return "quickjs-embedded";
        if (process.env.PERRY_DEV !== undefined) return "node-dev";
    }
    return "perry-native";
})();

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

    // NOTE: 绑定局部变量再传参（perry 0.5.1520 下实参位置的多层调用会错绑）
    const shell = (
        <div flexDirection="row" gap={12} grow={1}>
            <Sidebar />
            <MainPanel />
        </div>
    );

    appendChild(root, <Header />);
    appendChild(root, shell);
    appendChild(root, <Footer />);
}
