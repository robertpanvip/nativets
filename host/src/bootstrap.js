/**
 * QuickJS bootstrap — the JS environment the host installs *before* the app
 * bundle is evaluated.
 *
 * Split out of `quickjs.rs` (via `include_str!`) so it is a real, lintable,
 * editable JS file and so the node test suite can load the exact same source
 * (`ui/test/bootstrap.test.mjs` stubs the `__host*` primitives and asserts the
 * BOM semantics without starting a GUI).
 *
 * Contract with the host (`quickjs.rs::register_globals`):
 *   __hostWrite(s)          stdout sink            (Pipe mode: the ops pipe)
 *   __hostTrace(level,msg)  diagnostics → stderr   (never the protocol channel)
 *   __hostEmit(line)        one protocol line      (Direct mode only)
 *   __hostExit(code)        real process exit
 *   __hostWindow() -> JSON  {"innerWidth":…,"innerHeight":…,"dpr":…,"screenWidth":…,"screenHeight":…}
 *   __hostPerfNow() -> f64  monotonic ms since host start
 *   __hostEntropy(n) -> hex n random bytes (2n hex chars)
 *   __hostDialog(kind,message,title) -> i32   1 = ok, 0 = cancel
 *
 * Every one of them is optional: the BOM degrades to sane defaults when a
 * primitive is absent, which is what makes the node tests possible.
 *
 * This file is evaluated by QuickJS directly, NOT compiled by Perry, so the
 * Perry subset restrictions that apply to `ui/src/*.ts` (no try/catch, no
 * for...of) do not apply here. It is still written in a conservative ES5-ish
 * style on purpose — it is the floor everything else stands on.
 */

// ---------------------------------------------------------------------------
// timers
// ---------------------------------------------------------------------------

globalThis.__stdinCbs = {};
globalThis.__timers = [];
globalThis.__tid = 0;

/** Normalise a timer delay: anything not a non-negative finite number is 0. */
function __delay(ms) {
    return typeof ms === "number" && isFinite(ms) && ms > 0 ? ms : 0;
}

/** Fired by the host every engine tick (see `quickjs.rs::TICK`). */
globalThis.__hostTick = function () {
    var now = Date.now();
    var ts = globalThis.__timers;
    for (var i = 0; i < ts.length; i++) {
        var t = ts[i];
        if (t.due <= now) {
            try { t.cb(); } catch (e) {}
            // Repeating is a property of *which* registrar was called, not of
            // the delay: `setInterval(fn, 0)` must still repeat. Deriving it
            // from `interval > 0` made a 0ms interval behave like setTimeout.
            if (t.repeat) { t.due = now + t.every; }
            else { ts.splice(i, 1); i--; }
        }
    }
    var raf = globalThis.__raf;
    if (raf) globalThis.__rafMaybeFrame(now);
};

globalThis.setInterval = function (cb, ms) {
    var id = ++globalThis.__tid;
    var d = __delay(ms);
    globalThis.__timers.push({ id: id, cb: cb, due: Date.now() + d, every: d, repeat: true });
    return id;
};
globalThis.setTimeout = function (cb, ms) {
    var id = ++globalThis.__tid;
    globalThis.__timers.push({ id: id, cb: cb, due: Date.now() + __delay(ms), every: 0, repeat: false });
    return id;
};
globalThis.clearInterval = globalThis.clearTimeout = function (id) {
    var ts = globalThis.__timers;
    for (var i = 0; i < ts.length; i++) if (ts[i].id === id) { ts.splice(i, 1); return; }
};

globalThis.queueMicrotask = function (cb) {
    Promise.resolve().then(cb);
};

// ---------------------------------------------------------------------------
// process shim (QuickJS has no `process`)
// ---------------------------------------------------------------------------

process.stdin.setEncoding = function () {};
process.stdin.on = function (ev, cb) { globalThis.__stdinCbs[ev] = cb; };
// process.exit really exits (the app calls it from stdin 'end')
process.exit = function (code) { globalThis.__hostExit(code || 0); };

// ---------------------------------------------------------------------------
// console → original stderr (never the protocol channel)
// ---------------------------------------------------------------------------

globalThis.console = {
    log: function () { globalThis.__hostTrace("log", Array.prototype.map.call(arguments, String).join(" ")); },
    info: function () { globalThis.__hostTrace("info", Array.prototype.map.call(arguments, String).join(" ")); },
    warn: function () { globalThis.__hostTrace("warn", Array.prototype.map.call(arguments, String).join(" ")); },
    error: function () { globalThis.__hostTrace("error", Array.prototype.map.call(arguments, String).join(" ")); },
    debug: function () { globalThis.__hostTrace("debug", Array.prototype.map.call(arguments, String).join(" ")); },
    trace: function () { globalThis.__hostTrace("trace", "[console.trace] " + (new Error().stack || "")); },
    dir: function (o) { globalThis.__hostTrace("log", JSON.stringify(o)); },
    assert: function (cond) {
        if (!cond) {
            globalThis.__hostTrace(
                "error",
                "Assertion failed: " + Array.prototype.slice.call(arguments, 1).join(" ")
            );
        }
    }
};

// ---------------------------------------------------------------------------
// ambient helpers
// ---------------------------------------------------------------------------

var __env = (typeof process !== "undefined" && process.env) || {};

function __def(obj, name, getter) {
    try {
        Object.defineProperty(obj, name, { get: getter, configurable: true, enumerable: true });
    } catch (e) {}
}

function __num(v, fallback) {
    var n = typeof v === "string" ? parseFloat(v) : v;
    return typeof n === "number" && isFinite(n) ? n : fallback;
}

// ---------------------------------------------------------------------------
// window metrics — live getters over a cache the host refreshes
//
// The host owns the real numbers (GPUI window bounds + scale factor) and pushes
// a `{"t":"bom",…}` line whenever they change; at boot we pull once via
// `__hostWindow()` so the values are already right before the first paint.
// ---------------------------------------------------------------------------

globalThis.__bom = {
    win: {
        innerWidth: __num(__env.GPUI_TS_WIDTH, 0),
        innerHeight: __num(__env.GPUI_TS_HEIGHT, 0),
        dpr: __num(__env.GPUI_TS_DPR, 1),
        screenWidth: __num(__env.GPUI_TS_SCREEN_WIDTH, 0),
        screenHeight: __num(__env.GPUI_TS_SCREEN_HEIGHT, 0),
    },
};

globalThis.__bomApply = function (m) {
    var w = globalThis.__bom.win;
    var changed =
        __num(m.innerWidth, w.innerWidth) !== w.innerWidth ||
        __num(m.innerHeight, w.innerHeight) !== w.innerHeight;
    w.innerWidth = __num(m.innerWidth, w.innerWidth);
    w.innerHeight = __num(m.innerHeight, w.innerHeight);
    w.dpr = __num(m.dpr, w.dpr);
    w.screenWidth = __num(m.screenWidth, w.screenWidth);
    w.screenHeight = __num(m.screenHeight, w.screenHeight);
    // The very first apply happens before `dispatchEvent` exists (and before
    // the app could have registered a listener), so this is a real guard, not
    // a defensive habit.
    if (changed && typeof globalThis.dispatchEvent === "function") {
        globalThis.dispatchEvent({ type: "resize" });
    }
};

if (typeof __hostWindow === "function") {
    try {
        var __m = JSON.parse(__hostWindow());
        if (__m) globalThis.__bomApply(__m);
    } catch (e) {}
}

/**
 * Live update from the host: `{"t":"bom","w":1180,"h":760,"dpr":1,"sw":1920,"sh":1080}`.
 *
 * Short keys because this rides the live event channel; `__hostWindow()` (once
 * per boot) uses the readable names. The host intercepts these lines, so they
 * never reach the app's stdin `data` handler.
 */
globalThis.__bomLine = function (line) {
    try {
        var o = JSON.parse(line);
        if (!o || o.t !== "bom") return;
        globalThis.__bomApply({
            innerWidth: o.w,
            innerHeight: o.h,
            dpr: o.dpr,
            screenWidth: o.sw,
            screenHeight: o.sh,
        });
    } catch (e) {}
};

// `window` and `self` are the global object (single global, like a browser
// Worker). Everything below lands on globalThis so `window.x`, `self.x` and
// bare `x` all resolve to the same binding.
globalThis.window = globalThis;
globalThis.self = globalThis;

__def(globalThis, "innerWidth", function () { return globalThis.__bom.win.innerWidth; });
__def(globalThis, "innerHeight", function () { return globalThis.__bom.win.innerHeight; });
__def(globalThis, "outerWidth", function () { return globalThis.__bom.win.innerWidth; });
__def(globalThis, "outerHeight", function () { return globalThis.__bom.win.innerHeight; });
__def(globalThis, "devicePixelRatio", function () { return globalThis.__bom.win.dpr; });
__def(globalThis, "screenX", function () { return 0; });
__def(globalThis, "screenY", function () { return 0; });

globalThis.screen = {};
__def(globalThis.screen, "width", function () { return globalThis.__bom.win.screenWidth; });
__def(globalThis.screen, "height", function () { return globalThis.__bom.win.screenHeight; });
__def(globalThis.screen, "availWidth", function () { return globalThis.__bom.win.screenWidth; });
__def(globalThis.screen, "availHeight", function () { return globalThis.__bom.win.screenHeight; });
globalThis.screen.colorDepth = 32;
globalThis.screen.pixelDepth = 32;

// ---------------------------------------------------------------------------
// location / navigator
//
// There is no URL and no network here, so these are honest placeholders: the
// scheme says `nativets:` rather than pretending to be `https:`. They exist so
// feature detection and logging in third-party-ish code don't explode.
// ---------------------------------------------------------------------------

globalThis.location = {
    href: "nativets://app/index.tsx",
    protocol: "nativets:",
    host: "app",
    hostname: "app",
    port: "",
    pathname: "/index.tsx",
    search: "",
    hash: "",
    origin: "nativets://app",
    reload: function () {},
    assign: function (_url) {},
    replace: function (_url) {},
    toString: function () { return "nativets://app/index.tsx"; },
};

(function () {
    var lang = String(__env.GPUI_TS_LANG || __env.LC_ALL || __env.LANG || "en_US");
    lang = lang.split(".")[0].split("@")[0].replace("_", "-");
    if (!lang || lang === "C" || lang.indexOf("-") < 0) lang = "en-US";
    var engine = String(__env.GPUI_TS_ENGINE || "quickjs");
    var platform = String((typeof process !== "undefined" && process.platform) || "win32");
    globalThis.navigator = {
        userAgent: "nativets/0.1 (" + platform + "; GPUI) " + engine,
        platform: platform,
        language: lang,
        languages: [lang, "en"],
        onLine: false, // no network stack
        cookieEnabled: false,
        hardwareConcurrency: __num(__env.NUMBER_OF_PROCESSORS, 1),
        maxTouchPoints: 0,
        isSecureContext: true,
    };
})();

// ---------------------------------------------------------------------------
// performance (monotonic; wall clock would jump on NTP/DST)
// ---------------------------------------------------------------------------

(function () {
    var origin = Date.now();
    globalThis.performance = {
        timeOrigin: origin,
        now: function () {
            if (typeof __hostPerfNow === "function") {
                try { return __hostPerfNow(); } catch (e) {}
            }
            // fallback: wall clock minus boot — good enough for relative timing
            return Date.now() - origin;
        },
        mark: function () {},
        measure: function () {},
    };
})();

// ---------------------------------------------------------------------------
// crypto (entropy comes from the host's OS CSPRNG)
// ---------------------------------------------------------------------------

function __bomEntropyHex(n) {
    if (typeof __hostEntropy === "function") {
        try {
            var h = __hostEntropy(n);
            if (typeof h === "string" && h.length >= n * 2) return h;
        } catch (e) {}
    }
    // No host entropy: NOT cryptographically secure, and we do not pretend
    // otherwise — callers that need real randomness get it from the host.
    var s = "";
    for (var i = 0; i < n; i++) {
        var b = Math.floor(Math.random() * 256);
        s += (b < 16 ? "0" : "") + b.toString(16);
    }
    return s;
}

globalThis.crypto = {
    getRandomValues: function (arr) {
        if (!arr || typeof arr.length !== "number") {
            throw new TypeError("crypto.getRandomValues: expected a typed array");
        }
        var n = arr.length;
        if (n > 65536) throw new Error("crypto.getRandomValues: quota exceeded (max 65536 bytes)");
        var hex = __bomEntropyHex(n);
        for (var i = 0; i < n; i++) {
            arr[i] = parseInt(hex.substr(i * 2, 2), 16) || 0;
        }
        return arr;
    },
    randomUUID: function () {
        var hex = __bomEntropyHex(16);
        var b = [];
        for (var i = 0; i < 16; i++) b.push(parseInt(hex.substr(i * 2, 2), 16) || 0);
        b[6] = (b[6] & 0x0f) | 0x40; // version 4
        b[8] = (b[8] & 0x3f) | 0x80; // variant 10xx
        var out = "";
        for (var j = 0; j < 16; j++) out += (b[j] < 16 ? "0" : "") + b[j].toString(16);
        return (
            out.slice(0, 8) + "-" + out.slice(8, 12) + "-" + out.slice(12, 16) +
            "-" + out.slice(16, 20) + "-" + out.slice(20)
        );
    },
};

// ---------------------------------------------------------------------------
// structuredClone
// ---------------------------------------------------------------------------

globalThis.structuredClone = function (value) {
    var srcs = [];
    var dsts = [];

    function walk(v) {
        if (v === null || typeof v !== "object") {
            if (typeof v === "function") {
                throw new TypeError("structuredClone: a function could not be cloned");
            }
            return v;
        }
        for (var i = 0; i < srcs.length; i++) if (srcs[i] === v) return dsts[i];

        if (v instanceof Date) return new Date(v.getTime());
        if (typeof ArrayBuffer !== "undefined" && v instanceof ArrayBuffer) return v.slice(0);
        if (typeof ArrayBuffer !== "undefined" && ArrayBuffer.isView && ArrayBuffer.isView(v)) {
            return new v.constructor(v);
        }

        var out;
        if (Array.isArray(v)) out = [];
        else out = {};
        srcs.push(v);
        dsts.push(out);

        if (Array.isArray(v)) {
            for (var k = 0; k < v.length; k++) out[k] = walk(v[k]);
            return out;
        }
        for (var key in v) {
            if (Object.prototype.hasOwnProperty.call(v, key)) out[key] = walk(v[key]);
        }
        return out;
    }

    return walk(value);
};

// ---------------------------------------------------------------------------
// events (window-level; no DOM, so no bubbling — listeners fire on dispatch)
// ---------------------------------------------------------------------------

globalThis.__listeners = {};

globalThis.addEventListener = function (type, fn) {
    if (typeof fn !== "function" || !type) return;
    var list = globalThis.__listeners[type];
    if (!list) list = globalThis.__listeners[type] = [];
    if (list.indexOf(fn) < 0) list.push(fn);
};

globalThis.removeEventListener = function (type, fn) {
    var list = globalThis.__listeners[type];
    if (!list) return;
    for (var i = 0; i < list.length; i++) {
        if (list[i] === fn) { list.splice(i, 1); return; }
    }
};

globalThis.dispatchEvent = function (ev) {
    var e = typeof ev === "string" ? { type: ev } : ev;
    if (!e || !e.type) return false;
    if (e.target === undefined || e.target === null) e.target = globalThis;
    var list = globalThis.__listeners[e.type];
    if (list) {
        var snapshot = list.slice();
        for (var i = 0; i < snapshot.length; i++) {
            try { snapshot[i].call(globalThis, e); } catch (err) {}
        }
    }
    var on = globalThis["on" + e.type];
    if (typeof on === "function") {
        try { on.call(globalThis, e); } catch (err) {}
    }
    return true;
};

globalThis.Event = function (type, init) {
    this.type = String(type);
    this.target = null;
    this.timeStamp = globalThis.performance.now();
    this.defaultPrevented = false;
    this.bubbles = !!(init && init.bubbles);
    this.cancelable = !!(init && init.cancelable);
};
globalThis.Event.prototype.preventDefault = function () { this.defaultPrevented = true; };
globalThis.Event.prototype.stopPropagation = function () {};

globalThis.CustomEvent = function (type, init) {
    globalThis.Event.call(this, type, init);
    this.detail = init && init.detail !== undefined ? init.detail : null;
};
globalThis.CustomEvent.prototype = Object.create(globalThis.Event.prototype);
globalThis.CustomEvent.prototype.constructor = globalThis.CustomEvent;

// ---------------------------------------------------------------------------
// requestAnimationFrame
//
// Paced by the engine tick rather than a timer of its own: `__hostTick` calls
// `__rafMaybeFrame` every 10ms and a frame is emitted once `ms` has elapsed.
// Two consequences, both deliberate:
//   * the achievable cadence is a multiple of `quickjs.rs::TICK` (10ms), so the
//     default is 20ms = exactly two ticks → a stable 50Hz. Asking for 16ms
//     would silently produce an uneven ~20ms, which is worse.
//   * no timer exists for rAF at all, so an idle app still lets the engine
//     thread park (Direct transport) instead of spinning at 50Hz forever.
// Override the cadence with GPUI_TS_RAF_MS.
// ---------------------------------------------------------------------------

globalThis.__raf = { id: 0, cbs: {}, pending: 0, ms: __num(__env.GPUI_TS_RAF_MS, 20), last: 0 };

globalThis.requestAnimationFrame = function (cb) {
    if (typeof cb !== "function") return 0;
    var raf = globalThis.__raf;
    var id = ++raf.id;
    raf.cbs[id] = cb;
    raf.pending = raf.pending + 1;
    return id;
};

globalThis.cancelAnimationFrame = function (id) {
    var raf = globalThis.__raf;
    if (raf.cbs[id]) {
        delete raf.cbs[id];
        raf.pending = raf.pending - 1;
    }
};

/**
 * Emit one frame if one is due. Callbacks registered *during* a frame land in
 * the next frame (the map is swapped out first), matching browser semantics.
 */
globalThis.__rafMaybeFrame = function (now) {
    var raf = globalThis.__raf;
    if (raf.pending === 0) return;
    var t = typeof now === "number" ? now : Date.now();
    if (raf.last !== 0 && t - raf.last < raf.ms) return;
    raf.last = t;
    var cbs = raf.cbs;
    raf.cbs = {};
    raf.pending = 0;
    var ids = Object.keys(cbs);
    var stamp = globalThis.performance.now();
    for (var i = 0; i < ids.length; i++) {
        try { cbs[ids[i]](stamp); } catch (e) {}
    }
};

globalThis.requestIdleCallback = function (cb, opts) {
    var timeout = (opts && opts.timeout) || 1;
    return globalThis.setTimeout(function () {
        try {
            cb({ didTimeout: false, timeRemaining: function () { return 50; } });
        } catch (e) {}
    }, timeout);
};
globalThis.cancelIdleCallback = function (id) { globalThis.clearTimeout(id); };

// ---------------------------------------------------------------------------
// dialogs — rendered as a GPUI modal by the host
//
// `confirm` blocks the engine thread until the user answers, which is exactly
// what a browser does: JS really does stop. `alert` returns immediately.
// ---------------------------------------------------------------------------

globalThis.alert = function (message) {
    if (typeof __hostDialog === "function") {
        try { __hostDialog("alert", String(message === undefined ? "" : message), ""); return; } catch (e) {}
    }
    globalThis.__hostTrace("log", "[alert] " + message);
};

globalThis.confirm = function (message) {
    if (typeof __hostDialog === "function") {
        try { return __hostDialog("confirm", String(message === undefined ? "" : message), "") === 1; } catch (e) {}
    }
    globalThis.__hostTrace("log", "[confirm] " + message + " → false (no host dialog)");
    return false;
};

globalThis.prompt = function (message, def) {
    globalThis.__hostTrace(
        "warn",
        "[prompt] not implemented (no text input in the host dialog): " + message
    );
    return def === undefined ? null : def;
};
