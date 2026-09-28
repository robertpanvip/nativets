/**
 * BOM semantics test suite for `host/src/bootstrap.js`.
 *
 * The bootstrap is the JS environment the host installs before the app bundle,
 * and it is a real `.js` file precisely so it can be tested like one: load the
 * exact source into a `vm` context, stub the `__host*` primitives, and assert
 * behaviour without starting a GUI, a window, or the QuickJS engine.
 *
 * Run from the repo root:   node --test "host/test/*.test.mjs"
 * No dependencies — node built-ins only.
 *
 * (Pass a glob, not the bare directory: this node build resolves `host/test`
 * as a module and dies with MODULE_NOT_FOUND.)
 */
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import vm from "node:vm";

const SOURCE = readFileSync(new URL("../src/bootstrap.js", import.meta.url), "utf8");

const DEFAULT_METRICS = {
    innerWidth: 1180,
    innerHeight: 760,
    dpr: 1.5,
    screenWidth: 1920,
    screenHeight: 1080,
};

/**
 * Load the bootstrap into an isolated context.
 *
 * Each call gets a fresh realm, which is the point: the bootstrap installs
 * globals (timers, console, window, …) and tests must not see each other's.
 */
function loadBootstrap(opts = {}) {
    const state = { entropyCalls: [], dialogCalls: [], windowCalls: 0, traces: [] };
    let entropySeq = 0;

    const sandbox = {
        process: {
            env: {
                GPUI_TS_ENGINE: "quickjs",
                NUMBER_OF_PROCESSORS: "8",
                LANG: "zh_CN.UTF-8",
                ...(opts.env || {}),
            },
            platform: "win32",
            stdin: { setEncoding() {}, on() {} },
            stdout: { write() {} },
            stderr: { write() {} },
        },
        __hostWindow: () => {
            state.windowCalls++;
            return JSON.stringify(opts.metrics || DEFAULT_METRICS);
        },
        __hostPerfNow: () => (opts.perf === undefined ? 1234.5 : opts.perf),
        // Deterministic but *varying* per call: proves the bytes travel from the
        // host into the typed array. It says nothing about RNG quality, which is
        // the OS CSPRNG's job and cannot be tested from here.
        __hostEntropy: (n) => {
            state.entropyCalls.push(n);
            let s = "";
            for (let i = 0; i < n; i++) {
                s += ((entropySeq + i) % 256).toString(16).padStart(2, "0");
            }
            entropySeq += n;
            return s;
        },
        __hostDialog: (kind, message, title) => {
            state.dialogCalls.push({ kind, message, title });
            return opts.dialogAnswer === undefined ? 1 : opts.dialogAnswer;
        },
        __hostTrace: (level, message) => state.traces.push({ level, message }),
        __hostWrite: () => {},
        __hostExit: () => {},
    };

    if (opts.noWindow) delete sandbox.__hostWindow;
    if (opts.noEntropy) delete sandbox.__hostEntropy;
    if (opts.noDialog) delete sandbox.__hostDialog;
    if (opts.noPerf) delete sandbox.__hostPerfNow;

    const ctx = vm.createContext(sandbox);
    vm.runInContext(SOURCE, ctx, { filename: "bootstrap.js" });
    // Intrinsics (Date, Uint8Array, …) live on the *realm's* global, not on the
    // sandbox object we passed in, and the realm's TypeError is not node's.
    // Anything realm-specific therefore has to be constructed/evaluated inside.
    const run = (code) => vm.runInContext(code, ctx);
    return { ctx, state, run };
}

// ---------------------------------------------------------------------------
// window & metrics
// ---------------------------------------------------------------------------

test("window and self alias the global object", () => {
    const { ctx } = loadBootstrap();
    assert.equal(ctx.window, ctx.self);
    assert.equal(ctx.window.window, ctx.window);
});

test("geometry comes from the host at boot", () => {
    const { ctx, state } = loadBootstrap();
    assert.equal(state.windowCalls, 1, "the host is pulled exactly once at boot");
    assert.equal(ctx.innerWidth, 1180);
    assert.equal(ctx.innerHeight, 760);
    assert.equal(ctx.outerWidth, 1180);
    assert.equal(ctx.devicePixelRatio, 1.5);
    assert.equal(ctx.screen.width, 1920);
    assert.equal(ctx.screen.height, 1080);
    assert.equal(ctx.screen.availWidth, 1920);
    assert.equal(ctx.screen.colorDepth, 32);
});

test("geometry accessors are live, not snapshots", () => {
    const { ctx } = loadBootstrap();
    ctx.__bomLine('{"t":"bom","w":900,"h":640,"dpr":2,"sw":1920,"sh":1080}');
    assert.equal(ctx.innerWidth, 900);
    assert.equal(ctx.innerHeight, 640);
    assert.equal(ctx.devicePixelRatio, 2);
    assert.equal(ctx.window.innerWidth, 900);
});

test("a geometry change dispatches resize; an identical push does not", () => {
    const { ctx } = loadBootstrap();
    const seen = [];
    ctx.addEventListener("resize", (ev) => seen.push(ev.type));

    ctx.__bomLine('{"t":"bom","w":900,"h":640,"dpr":1.5,"sw":1920,"sh":1080}');
    assert.deepEqual(seen, ["resize"]);

    ctx.__bomLine('{"t":"bom","w":900,"h":640,"dpr":1.5,"sw":1920,"sh":1080}');
    assert.equal(seen.length, 1, "an unchanged push must not fire resize");

    ctx.__bomLine('{"t":"bom","w":901,"h":640,"dpr":1.5,"sw":1920,"sh":1080}');
    assert.equal(seen.length, 2);
});

test("a malformed bom line is ignored, not fatal", () => {
    const { ctx } = loadBootstrap();
    ctx.__bomLine("not json at all");
    ctx.__bomLine('{"t":"other","w":1}');
    assert.equal(ctx.innerWidth, 1180);
});

test("without the host primitive the metrics stay at their env defaults", () => {
    const { ctx } = loadBootstrap({
        noWindow: true,
        env: { GPUI_TS_WIDTH: "800", GPUI_TS_HEIGHT: "600", GPUI_TS_DPR: "2" },
    });
    assert.equal(ctx.innerWidth, 800);
    assert.equal(ctx.innerHeight, 600);
    assert.equal(ctx.devicePixelRatio, 2);
});

// ---------------------------------------------------------------------------
// events
// ---------------------------------------------------------------------------

test("addEventListener / removeEventListener / dispatchEvent", () => {
    const { ctx } = loadBootstrap();
    const hits = [];
    const onFoo = (ev) => hits.push("a:" + ev.type);
    const other = () => hits.push("b");

    ctx.addEventListener("foo", onFoo);
    ctx.addEventListener("foo", onFoo); // duplicate registration is a no-op
    ctx.addEventListener("foo", other);
    ctx.dispatchEvent({ type: "foo" });
    assert.deepEqual(hits, ["a:foo", "b"]);

    ctx.removeEventListener("foo", onFoo);
    ctx.dispatchEvent({ type: "foo" });
    assert.deepEqual(hits, ["a:foo", "b", "b"]);
});

test("dispatchEvent accepts a bare string and defaults target to window", () => {
    const { ctx } = loadBootstrap();
    let target = "unset";
    ctx.addEventListener("ping", (ev) => { target = ev.target; });
    ctx.dispatchEvent("ping");
    assert.equal(target, ctx.window);
});

test("an `on<type>` property is called too", () => {
    const { ctx } = loadBootstrap();
    let called = 0;
    ctx.onresize = () => { called++; };
    ctx.dispatchEvent({ type: "resize" });
    assert.equal(called, 1);
});

test("CustomEvent carries detail and inherits Event", () => {
    const { ctx } = loadBootstrap();
    let detail = null;
    let prevented = null;
    ctx.addEventListener("custom", (ev) => {
        detail = ev.detail;
        ev.preventDefault();
        prevented = ev.defaultPrevented;
    });
    ctx.dispatchEvent(new ctx.CustomEvent("custom", { detail: { a: 1 } }));
    assert.deepEqual({ ...detail }, { a: 1 });
    assert.equal(prevented, true);
    assert.ok(new ctx.CustomEvent("x") instanceof ctx.Event);
});

test("a throwing listener does not stop the others", () => {
    const { ctx } = loadBootstrap();
    let reached = false;
    ctx.addEventListener("boom", () => { throw new Error("listener blew up"); });
    ctx.addEventListener("boom", () => { reached = true; });
    ctx.dispatchEvent("boom");
    assert.equal(reached, true);
});

// ---------------------------------------------------------------------------
// timers & rAF
// ---------------------------------------------------------------------------

test("setTimeout fires once on the tick; setInterval repeats", () => {
    const { ctx } = loadBootstrap();
    let once = 0;
    let every = 0;
    ctx.setTimeout(() => { once++; }, 0);
    // A 0ms interval used to be indistinguishable from a timeout, because the
    // tick keyed "repeat" off `interval > 0`. Covered here on purpose.
    const id = ctx.setInterval(() => { every++; }, 0);

    ctx.__hostTick();
    assert.equal(once, 1);
    assert.equal(every, 1);

    ctx.__hostTick();
    assert.equal(once, 1, "a timeout must not repeat");
    assert.equal(every, 2, "a 0ms interval must still repeat");

    ctx.clearInterval(id);
    ctx.__hostTick();
    assert.equal(every, 2);
});

test("a timer with a real delay waits for its due time", () => {
    const { ctx } = loadBootstrap();
    let fired = 0;
    ctx.setTimeout(() => { fired++; }, 10_000);
    ctx.__hostTick();
    assert.equal(fired, 0);
    assert.equal(ctx.__timers.length, 1);
});

test("requestAnimationFrame fires from the engine tick with a perf timestamp", () => {
    const { ctx } = loadBootstrap();
    let stamp = null;
    ctx.requestAnimationFrame((t) => { stamp = t; });
    assert.equal(stamp, null, "nothing runs before a tick");
    ctx.__hostTick();
    assert.equal(stamp, 1234.5, "the callback gets performance.now()");
});

test("rAF honours the cadence and defers re-registration to the next frame", () => {
    const { ctx } = loadBootstrap();
    let frames = 0;
    const loop = () => { frames++; ctx.requestAnimationFrame(loop); };
    ctx.requestAnimationFrame(loop);

    ctx.__rafMaybeFrame(10000); // first frame (last === 0 → due)
    assert.equal(frames, 1);

    ctx.__rafMaybeFrame(10005); // 5ms < 20ms cadence
    assert.equal(frames, 1, "must not run faster than the cadence");

    ctx.__rafMaybeFrame(10025); // 25ms ≥ 20ms
    assert.equal(frames, 2, "the callback re-registered during a frame runs next frame");

    ctx.__rafMaybeFrame(10030);
    assert.equal(frames, 2);
});

test("cancelAnimationFrame drops a pending callback", () => {
    const { ctx } = loadBootstrap();
    let ran = false;
    const id = ctx.requestAnimationFrame(() => { ran = true; });
    ctx.cancelAnimationFrame(id);
    ctx.__rafMaybeFrame(10000);
    assert.equal(ran, false);
    assert.equal(ctx.__raf.pending, 0);
});

test("rAF cadence is overridable", () => {
    const { ctx } = loadBootstrap({ env: { GPUI_TS_RAF_MS: "50" } });
    assert.equal(ctx.__raf.ms, 50);
});

test("queueMicrotask runs at the end of the current turn", async () => {
    const { ctx } = loadBootstrap();
    const order = [];
    ctx.queueMicrotask(() => order.push("micro"));
    order.push("sync");
    // Microtasks belong to the vm realm and drain on the host's microtask
    // queue, so a couple of awaits is all it takes.
    await Promise.resolve();
    await Promise.resolve();
    assert.deepEqual(order, ["sync", "micro"]);
});

// ---------------------------------------------------------------------------
// structuredClone
// ---------------------------------------------------------------------------

test("structuredClone deep-copies plain data", () => {
    const { ctx } = loadBootstrap();
    const src = { a: 1, b: { c: [1, 2, { d: "x" }] }, e: [true, null] };
    const copy = ctx.structuredClone(src);
    assert.deepEqual(JSON.parse(JSON.stringify(copy)), {
        a: 1,
        b: { c: [1, 2, { d: "x" }] },
        e: [true, null],
    });
    assert.notEqual(copy, src);
    assert.notEqual(copy.b, src.b);
    assert.notEqual(copy.b.c, src.b.c);
    copy.b.c[0] = 99;
    assert.equal(src.b.c[0], 1, "the copy must not share nested references");
});

test("structuredClone handles cycles and Dates", () => {
    const { ctx, run } = loadBootstrap();
    const src = { name: "root" };
    src.self = src;
    src.when = run("new Date(1700000000000)");

    const copy = ctx.structuredClone(src);
    assert.equal(copy.self, copy, "a cycle points at the copy, not the original");
    assert.equal(copy.self.name, "root");
    assert.notEqual(copy.when, src.when);
    assert.equal(copy.when.getTime(), 1700000000000);
});

test("structuredClone rejects functions", () => {
    const { ctx } = loadBootstrap();
    // Match the message, not the constructor: the realm's TypeError is not
    // node's TypeError.
    assert.throws(() => ctx.structuredClone({ fn: () => {} }), /could not be cloned/);
});

// ---------------------------------------------------------------------------
// crypto
// ---------------------------------------------------------------------------

test("getRandomValues fills from host entropy and returns the array", () => {
    const { ctx, state, run } = loadBootstrap();
    const arr = run("new Uint8Array(4)");
    const returned = ctx.crypto.getRandomValues(arr);
    assert.equal(returned, arr, "the same array is returned, filled in place");
    assert.deepEqual(Array.from(arr), [0, 1, 2, 3]);
    assert.deepEqual(state.entropyCalls, [4], "entropy is requested from the host");
});

test("getRandomValues enforces the spec quota", () => {
    const { ctx, run } = loadBootstrap();
    assert.throws(() => ctx.crypto.getRandomValues(run("new Uint8Array(65537)")), /quota/);
    assert.throws(() => ctx.crypto.getRandomValues(null), /typed array/);
});

test("randomUUID is a well-formed v4 UUID, unique per call", () => {
    const { ctx } = loadBootstrap();
    const a = ctx.crypto.randomUUID();
    const b = ctx.crypto.randomUUID();
    assert.match(a, /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/);
    assert.notEqual(a, b);
});

test("entropy falls back (and says so) when the host has none", () => {
    const { ctx } = loadBootstrap({ noEntropy: true });
    const id = ctx.crypto.randomUUID();
    assert.match(id, /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/);
});

// ---------------------------------------------------------------------------
// performance
// ---------------------------------------------------------------------------

test("performance.now uses the host's monotonic clock", () => {
    const { ctx } = loadBootstrap({ perf: 42.25 });
    assert.equal(ctx.performance.now(), 42.25);
    assert.equal(typeof ctx.performance.timeOrigin, "number");
});

test("performance.now falls back to a relative clock without the host", () => {
    const { ctx } = loadBootstrap({ noPerf: true });
    const a = ctx.performance.now();
    assert.equal(typeof a, "number");
    assert.ok(a >= 0 && a < 1000, `expected a small relative value, got ${a}`);
});

// ---------------------------------------------------------------------------
// navigator / location / screen
// ---------------------------------------------------------------------------

test("navigator reports the engine, platform and locale honestly", () => {
    const { ctx } = loadBootstrap();
    assert.match(ctx.navigator.userAgent, /^nativets\//);
    assert.match(ctx.navigator.userAgent, /win32/);
    assert.match(ctx.navigator.userAgent, /quickjs/);
    assert.equal(ctx.navigator.platform, "win32");
    assert.equal(ctx.navigator.language, "zh-CN");
    assert.deepEqual(Array.from(ctx.navigator.languages), ["zh-CN", "en"]);
    assert.equal(ctx.navigator.hardwareConcurrency, 8);
    assert.equal(ctx.navigator.onLine, false, "there is no network stack");
});

test("navigator.language degrades sanely from a POSIX locale", () => {
    assert.equal(loadBootstrap({ env: { LANG: "C" } }).ctx.navigator.language, "en-US");
    assert.equal(loadBootstrap({ env: { LANG: "" } }).ctx.navigator.language, "en-US");
    assert.equal(loadBootstrap({ env: { LANG: "en_GB.UTF-8" } }).ctx.navigator.language, "en-GB");
});

test("location is a nativets: placeholder, not a fake https URL", () => {
    const { ctx } = loadBootstrap();
    assert.equal(ctx.location.protocol, "nativets:");
    assert.match(ctx.location.href, /^nativets:\/\//);
    assert.equal(String(ctx.location), ctx.location.href);
});

test("document is deliberately absent", () => {
    const { ctx } = loadBootstrap();
    assert.equal(ctx.document, undefined, "no DOM is emulated — absence is the honest signal");
});

// ---------------------------------------------------------------------------
// dialogs
// ---------------------------------------------------------------------------

test("confirm returns the user's answer and passes the message through", () => {
    const { ctx, state } = loadBootstrap({ dialogAnswer: 1 });
    assert.equal(ctx.confirm("重置计数？"), true);
    assert.deepEqual(state.dialogCalls, [
        { kind: "confirm", message: "重置计数？", title: "" },
    ]);

    const no = loadBootstrap({ dialogAnswer: 0 });
    assert.equal(no.ctx.confirm("确定？"), false);
});

test("alert goes through the host dialog too", () => {
    const { ctx, state } = loadBootstrap();
    ctx.alert("hello");
    assert.deepEqual(state.dialogCalls, [{ kind: "alert", message: "hello", title: "" }]);
});

test("without a host dialog, alert traces and confirm is false (never throws)", () => {
    const { ctx, state } = loadBootstrap({ noDialog: true });
    ctx.alert("nope");
    assert.equal(ctx.confirm("nope"), false);
    assert.ok(state.traces.some((t) => t.message.includes("[alert]")));
    assert.ok(state.traces.some((t) => t.message.includes("[confirm]")));
});

test("prompt is explicitly unimplemented rather than silently wrong", () => {
    const { ctx, state } = loadBootstrap();
    assert.equal(ctx.prompt("name?", "fallback"), "fallback");
    assert.ok(state.traces.some((t) => t.level === "warn" && t.message.includes("[prompt]")));
});
