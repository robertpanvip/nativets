/**
 * Extended native component card — exercises the 16 gpui-component widgets the
 * retained-tree bridge gained in this milestone:
 *   textarea / combobox / colorpicker / radio / tabs / pagination / breadcrumb /
 *   alert / badge / tag / avatar / separator / skeleton / label / link /
 *   collapsible
 *
 * Every control is controlled: the demo owns the value and pushes it via
 * `setValue`; the host echoes user interaction back as an event the `onChange` /
 * `onInput` / `onClick` / `onClose` callbacks receive.
 */

import { h, text, Show } from "../io";
import type { El, Props } from "../io";
import { C } from "../theme";
import { Card, PillBtn } from "../primitives";
import {
    Textarea,
    Combobox,
    ColorPicker,
    RadioNative,
    TabsNative,
    PaginationNative,
    BreadcrumbNative,
    AlertNative,
    Badge,
    Tag,
    Avatar,
    Separator,
    Skeleton,
    Label,
    Link,
    Collapsible,
    Button,
    Progress,
} from "../kit";
import {
    taText,
    setTaText,
    comboVal,
    setComboVal,
    colorVal,
    setColorVal,
    radioIdx,
    setRadioIdx,
    tabIdx,
    setTabIdx,
    page,
    setPage,
    crumbIdx,
    setCrumbIdx,
    collOpen,
    setCollOpen,
    alertVisible,
    setAlertVisible,
    extLog,
    setExtLog,
} from "../state";

function logExt(line: string): void {
    console.info("[ext] " + line);
    const next = line + "\n" + extLog();
    setExtLog(next.length > 300 ? next.slice(0, 300) : next);
}

export function ExtendedComponentsCard(_props: Props): El {
    return (
        <Card title="原生组件扩展 · 16 个 gpui-component 控件">
            <div style={{ flexDirection: "column", gap: 14 }}>
                {text("① 文本与输入", { fontSize: 12, color: C.textSecondary, fontWeight: "bold" })}
                <Textarea
                    value={taText}
                    height={70}
                    onInput={(v: string) => setTaText(v)}
                    style={{ borderWidth: 1, borderColor: C.border, borderRadius: 8 }}
                />
                <div style={{ flexDirection: "row", gap: 16, alignItems: "center" }}>
                    <Combobox
                        options={["上海", "北京", "深圳", "杭州"]}
                        value={comboVal}
                        width={160}
                        onChange={setComboVal}
                    />
                    {text(() => "选中：" + comboVal(), { fontSize: 12, color: C.textSecondary })}
                    <ColorPicker value={colorVal} onChange={setColorVal} />
                    {text(() => colorVal(), { fontSize: 12, color: C.textSecondary })}
                </div>

                {text("② 选择与时间无关的索引控件", { fontSize: 12, color: C.textSecondary, fontWeight: "bold" })}
                <RadioNative options={["红", "绿", "蓝"]} value={radioIdx} onChange={setRadioIdx} />
                <TabsNative options={["概览", "详情", "设置"]} value={tabIdx} onChange={setTabIdx} />
                <PaginationNative total={8} value={page} onChange={setPage} />
                <BreadcrumbNative
                    options={["首页", "文档", "当前页"]}
                    onChange={(i: number) => {
                        setCrumbIdx(i);
                        logExt("breadcrumb → " + String(i));
                    }}
                />

                {text("③ 展示型组件", { fontSize: 12, color: C.textSecondary, fontWeight: "bold" })}
                <div style={{ flexDirection: "row", gap: 10, alignItems: "center" }}>
                    <Badge style={{ background: C.accentDim, color: C.textPrimary, paddingX: 8, paddingY: 3, borderRadius: 6 }}>
                        {text("新", { fontSize: 12 })}
                    </Badge>
                    <Tag variant="success">{text("成功", { fontSize: 12 })}</Tag>
                    <Tag variant="danger">{text("危险", { fontSize: 12 })}</Tag>
                    <Tag variant="warning">{text("警告", { fontSize: 12 })}</Tag>
                    <Avatar name="张三" style={{}} />
                    <Label text="标签文本" style={{ fontSize: 13 }} />
                    <Link href="https://example.com" onClick={() => logExt("link clicked")}>
                        {text("链接", { fontSize: 13, color: C.accent })}
                    </Link>
                </div>
                <div style={{ flexDirection: "row", gap: 12, alignItems: "center" }}>
                    <Skeleton style={{ width: 180, height: 12 }} />
                    <Separator orientation="vertical" style={{ height: 24 }} />
                    <Skeleton style={{ width: 120, height: 12 }} />
                </div>

                {text("④ 折叠面板与告警", { fontSize: 12, color: C.textSecondary, fontWeight: "bold" })}
                {Show(alertVisible, () => (
                    <AlertNative
                        text="这是一条可关闭的告警（原生 Alert）"
                        onClose={() => setAlertVisible(false)}
                    />
                ))}
                <div style={{ flexDirection: "row", alignItems: "center", gap: 10 }}>
                    <Button
                        label={collOpen() ? "收起" : "展开"}
                        onClick={() => setCollOpen(!collOpen())}
                        style={{ height: 26, paddingX: 10 }}
                    />
                </div>
                <Collapsible open={collOpen}>
                    <div
                        style={{
                            flexDirection: "column",
                            gap: 6,
                            padding: 10,
                            background: C.cardAlt,
                            borderRadius: 8,
                            borderWidth: 1,
                            borderColor: C.border,
                        }}
                    >
                        {text("折叠内容：这里可以放任意子树。", { fontSize: 12, color: C.textSecondary })}
                        <Progress value={() => 42} height={8} style={{ width: "100%" }} />
                    </div>
                </Collapsible>

                {text(() => (extLog() === "" ? "（还没有交互）" : extLog()), {
                    fontSize: 11,
                    color: C.textMuted,
                })}
            </div>
        </Card>
    );
}
