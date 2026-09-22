// 点击延迟探针：向被测前端 stdin 注入 10 次带时间戳的 click 事件。
// stderr 输出 FEED <ms>；配合前端 PERRY_UI_TRACE=1 的 EVT_RECV/FLUSH 计算逐段延迟。
let n = 0;
const timer = setInterval(() => {
    n++;
    const line = JSON.stringify({ t: "event", target: 9, kind: "click", ts: Date.now() });
    process.stdout.write(line + "\n");
    process.stderr.write("FEED " + Date.now() + "\n");
    if (n >= 10) {
        clearInterval(timer);
        setTimeout(() => process.exit(0), 800);
    }
}, 200);
