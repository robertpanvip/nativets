// 分析 trace 文件：配对 EVT_RECV 与其后第一个 FLUSH，计算逐段延迟
// 用法: node analyze-trace.mjs <feed.txt> <frontend-trace.txt> <label>
import { readFileSync } from "node:fs";
const [, , feedFile, traceFile, label] = process.argv;
const feed = readFileSync(feedFile, "utf8").split("\n").filter((l) => l.startsWith("FEED")).map((l) => Number(l.split(" ")[1]));
const evt = [];
const flush = [];
for (const line of readFileSync(traceFile, "utf8").split("\n")) {
    if (line.startsWith("EVT_RECV")) {
        const ms = Number(line.split(" ")[1]);
        const feeder = Number(line.split("feeder=")[1]);
        evt.push({ ms, feeder });
    } else if (line.startsWith("FLUSH")) {
        flush.push(Number(line.split(" ")[1]));
    }
}
const d1s = [], d2s = [];
let fi = 0;
for (let i = 0; i < evt.length; i++) {
    const e = evt[i];
    d1s.push(e.ms - e.feeder); // stdin 送达延迟（feeder 写入 → 前端收到）
    while (fi < flush.length && flush[fi] < e.ms) fi++; // 跳过事件前的 flush
    if (fi < flush.length) d2s.push(flush[fi] - e.ms); // 事件处理 → flush 写出
}
const stat = (a) => {
    const sorted = [...a].sort((x, y) => x - y);
    const avg = a.reduce((s, v) => s + v, 0) / a.length;
    return `avg=${avg.toFixed(1)}ms p50=${sorted[Math.floor(a.length / 2)]}ms max=${sorted[sorted.length - 1]}ms`;
};
console.log(`--- ${label} (n=${d1s.length}) ---`);
console.log(`stdin 送达: ${stat(d1s)}`);
console.log(`事件→flush: ${stat(d2s)}`);
