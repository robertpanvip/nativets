/**
 * scriptc library-mode entry — the FFI-shaped facade over the demo UI.
 *
 * scriptc 0.1.3 `--lib` contract, established by probe (see ll + memory):
 *   - `abi.init_symbol` (gpts_init) resets the runtime ARENA and runs the
 *     MODULE TOP-LEVEL statements — it never calls any exported function.
 *     Anything the host must happen at init therefore lives at top level.
 *     Functions not reachable from exports/top-level are tree-shaken.
 *   - `exports[]` must be `export function` declarations in the entry module
 *     (SC4002), with symbols distinct from the reserved abi symbols (SC4001).
 *   - the graph must be async-free: no `Promise`/`setTimeout` (SC4005) and
 *     no timers surface.
 *   - array reads (`a[0]`, `.shift()`) keep `| undefined` in the lowered IR;
 *     string-buffer state avoids the SC4003 union refusal entirely.
 *
 * Host-pull protocol (everything synchronous, driven by the host thread):
 *
 *     gpts_init   → top level  : runtime reset + first tree build
 *     gpts_tick   → appTick    : host heartbeat; advances the clock
 *     gpts_poll   → appPoll    : one buffered protocol line ("" = drained)
 *     gpts_event  → appEvent   : host pushes `{"t":"event",…}` JSON
 *     gpts_reset  → appReset   : explicit re-init (host may re-enter)
 *
 * This file is TS-source for scriptc's own checker. Never esbuild-bundle it
 * first: type erasure turns typed ops into dynamic dispatch (SC4005).
 */

let count: number = 0;
let clock: string = "00:00";
let tickCount: number = 0;

// buffered protocol lines, "\n"-joined. A string, deliberately NOT string[]:
// under strictNullChecks every array read keeps `| undefined` in the lowered
// IR and scriptc refuses the export (SC4003). String slicing never unions.
let outbox: string = "";

function emit(line: string): void {
    outbox = outbox + line + "\n";
}

function buildTree(): void {
    emit('{"t":"hello","proto":1,"title":"nativets × scriptc"}');
    // One batch = one JSONL line: accumulate ops, then emit once.
    let ops: string = "";
    ops = ops + '{"op":"setTitle","title":"nativets × scriptc"},';
    ops = ops + '{"op":"create","id":1,"tag":"div"},';
    ops = ops + '{"op":"setStyle","id":1,"style":{"padding":24,"flexDirection":"column","gap":12,"background":"#12141d"}},';
    ops = ops + '{"op":"append","id":1,"parent":0}';

    // header
    ops = ops + ',{"op":"create","id":2,"tag":"text","text":"nativets · scriptc backend"},';
    ops = ops + '{"op":"setStyle","id":2,"style":{"fontSize":20,"weight":700,"color":"#e8eaf2"}},';
    ops = ops + '{"op":"append","id":2,"parent":1}';

    // counter card
    ops = ops + ',{"op":"create","id":3,"tag":"div"},';
    ops = ops + '{"op":"setStyle","id":3,"style":{"padding":16,"background":"#1b1e2b","borderRadius":10,"flexDirection":"row","gap":12,"alignItems":"center"}},';
    ops = ops + '{"op":"append","id":3,"parent":1}';

    ops = ops + ',{"op":"create","id":4,"tag":"button","text":"-"},';
    ops = ops + '{"op":"setStyle","id":4,"style":{"padding":8,"background":"#2a2f42","borderRadius":8,"color":"#ffffff"}},';
    ops = ops + '{"op":"append","id":4,"parent":3},';
    ops = ops + '{"op":"setEvents","id":4,"events":["click"]}';

    ops = ops + ',{"op":"create","id":5,"tag":"text","text":"0"},';
    ops = ops + '{"op":"setStyle","id":5,"style":{"fontSize":24,"color":"#7ee2a8"}},';
    ops = ops + '{"op":"append","id":5,"parent":3}';

    ops = ops + ',{"op":"create","id":6,"tag":"button","text":"+"},';
    ops = ops + '{"op":"setStyle","id":6,"style":{"padding":8,"background":"#2a2f42","borderRadius":8,"color":"#ffffff"}},';
    ops = ops + '{"op":"append","id":6,"parent":3},';
    ops = ops + '{"op":"setEvents","id":6,"events":["click"]}';

    // clock row
    ops = ops + ',{"op":"create","id":7,"tag":"text","text":"00:00"},';
    ops = ops + '{"op":"setStyle","id":7,"style":{"fontSize":14,"color":"#8b90a5"}},';
    ops = ops + '{"op":"append","id":7,"parent":1}';

    emit('{"t":"batch","ops":[' + ops + "]}");
}

function updateCounter(): void {
    emit('{"t":"batch","ops":[{"op":"setText","id":5,"text":"' + String(count) + '"}]}');
}

function updateClock(): void {
    emit('{"t":"batch","ops":[{"op":"setText","id":7,"text":"' + clock + '"}]}');
}

function reset(): void {
    count = 0;
    clock = "00:00";
    tickCount = 0;
    outbox = "";
    buildTree();
}

// --- profile exports (the FFI surface) --------------------------------------

export function appTick(): void {
    // Host-driven heartbeat: every 100th tick advances the clock. There is no
    // event loop here by design (SC4005) — the host's driver loop is the
    // pulse.
    tickCount = tickCount + 1;
    if (tickCount % 100 === 0) {
        const total: number = Math.floor(tickCount / 100);
        const mins: number = Math.floor(total / 60) % 60;
        const secs: number = total % 60;
        let m: string = String(mins);
        let s: string = String(secs);
        if (mins < 10) {
            m = "0" + m;
        }
        if (secs < 10) {
            s = "0" + s;
        }
        clock = m + ":" + s;
        updateClock();
    }
}

export function appPoll(): string {
    if (outbox === "") {
        return "";
    }
    const idx: number = outbox.indexOf("\n");
    const line: string = outbox.substring(0, idx);
    outbox = outbox.substring(idx + 1);
    return line;
}

export function appEvent(json: string): void {
    // Wire format: {"t":"event","kind":"click","target":N}
    if (json.indexOf('"kind":"click"') < 0) {
        return;
    }
    if (json.indexOf('"target":4') >= 0) {
        count = count - 1;
        updateCounter();
        return;
    }
    if (json.indexOf('"target":6') >= 0) {
        count = count + 1;
        updateCounter();
    }
}

export function appReset(): void {
    reset();
}

// Module top level == the body of abi.init_symbol (gpts_init): scriptc runs
// these statements after the runtime reset when the host calls gpts_init.
reset();
