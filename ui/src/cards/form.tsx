/**
 * Form controls card — input / radio / checkbox / switch / select, all bound to
 * shared signals through the kit wrappers.
 */

import { h, text } from "../io";
import type { El, Props } from "../io";
import { C } from "../theme";
import { Card } from "../primitives";
import { Input, DatePicker, Select, Switch, RadioGroup, Checkbox } from "../kit";
import {
    env,
    setEnv,
    verbose,
    autoRefresh,
    region,
    keyword,
    setKeyword,
    committed,
    fieldEvents,
    setFieldEvents,
    setCommitted,
    pickedDate,
    setPickedDate,
    setRegion,
    setAutoRefresh,
    setVerbose,
    ENVS,
    REGIONS,
} from "../state";

/** Every control on one line. Reading four signals here is all it takes for the
 *  text to keep itself current. */
function formSummary(): string {
    const mode = verbose() ? "详细" : "精简";
    const refresh = autoRefresh() ? "自动刷新" : "手动刷新";
    return env() + " · " + region() + " · " + mode + " · " + refresh;
}

function committedLabel(): string {
    const v = committed();
    return "input 事件 " + fieldEvents() + " 次 · Enter 提交值：" + (v === "" ? "(空)" : v);
}

export function FormCard(_props: Props): El {
    return (
        <Card title="表单控件 · input / radio / checkbox / switch / select">
            <div style={{ flexDirection: "row", gap: 24, alignItems: "start" }}>
                <div style={{ flexDirection: "column", gap: 12, width: 300 }}>
                    <Input
                        label="关键字（回车提交，Ctrl+V 粘贴）"
                        value={keyword}
                        placeholder="输入后回车…"
                        onInput={(v: string) => {
                            setKeyword(v);
                            setFieldEvents(fieldEvents() + 1);
                        }}
                        onChange={(v: string) => setCommitted(v)}
                    />
                    <DatePicker
                        value={pickedDate}
                        placeholder="选择日期…"
                        onChange={(v: string) => {
                            setPickedDate(v);
                            console.log("[app] date picked:", v);
                        }}
                    />
                    <div style={{ flexDirection: "row", alignItems: "center", gap: 14 }}>
                        <Select options={REGIONS} value={region} onChange={setRegion} />
                        <Switch
                            checked={autoRefresh}
                            onToggle={() => setAutoRefresh(!autoRefresh())}
                            label="自动刷新"
                        />
                    </div>
                </div>
                <div style={{ flexDirection: "column", gap: 12, grow: 1 }}>
                    <RadioGroup options={ENVS} value={env} onChange={setEnv} />
                    <Checkbox
                        label="输出详细日志"
                        checked={verbose}
                        onToggle={() => setVerbose(!verbose())}
                    />
                    {text(() => "当前：" + formSummary(), { fontSize: 12, color: C.textSecondary })}
                    {text(committedLabel, { fontSize: 12, color: C.textMuted })}
                </div>
            </div>
        </Card>
    );
}
