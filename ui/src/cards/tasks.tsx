/**
 * Task list card — `For` dynamic rendering + event callbacks round-tripping back
 * to the frontend. `nextTaskId` is a plain local counter (not a signal): it only
 * feeds the id of newly-added rows.
 */

import { h, text, For } from "../io";
import type { El, Props } from "../io";
import { C } from "../theme";
import { Card, PillBtn } from "../primitives";
import { tasks, setTasks, samples } from "../state";
import type { Task } from "../state";

let nextTaskId = 5;

export function TaskRow(props: { t: Task }): El {
    const t = props.t;
    return (
        <div
            style={{
                flexDirection: "row",
                alignItems: "center",
                justifyContent: "between",
                padding: 10,
                paddingX: 12,
                background: C.cardAlt,
                borderRadius: 8,
                borderWidth: 1,
                borderColor: C.border,
            }}
        >
            <div style={{ flexDirection: "row", alignItems: "center", gap: 10 }}>
                <div
                    style={{
                        color: t.done ? C.green : C.textMuted,
                        fontSize: 15,
                        fontWeight: "bold",
                        paddingX: 4,
                    }}
                    onClick={() => toggleTask(t.id)}
                >
                    {t.done ? "√" : "○"}
                </div>
                {text(t.title, { fontSize: 13, color: t.done ? C.textMuted : C.textPrimary })}
            </div>
            <div
                style={{ color: C.red, fontSize: 12, paddingX: 8, paddingY: 4, borderRadius: 6 }}
                onClick={() => removeTask(t.id)}
            >
                删除
            </div>
        </div>
    );
}

export function TasksCard(_props: Props): El {
    return (
        <Card title="任务列表 · For 动态渲染 + 事件回传">
            <div style={{ flexDirection: "column", gap: 8 }}>
                {For(
                    tasks,
                    (t) => <TaskRow t={t} />,
                )}
            </div>
            <div style={{ flexDirection: "row", gap: 10, alignItems: "center" }}>
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

function toggleTask(id: number): void {
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
