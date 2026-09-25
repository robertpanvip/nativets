/**
 * Canvas 2D — a recorder that speaks the familiar API and lowers each call to
 * the host's five drawing primitives (`host/src/draw.rs`).
 *
 * There is no raster canvas here, and deliberately so: the host owns the GPU and
 * the text system, the frontend owns the *description* of what to draw. A
 * `beginPath/arc/fill` sequence becomes one `{"k":"poly",…}` command; a whole
 * frame becomes one `setCanvas` op. Three consequences:
 *
 *   * a redraw is one op, not a subtree rebuild;
 *   * the display list is plain JSON, so `GPUI_TS_LOG_OPS=1` shows exactly what
 *     the app asked for (a canvas bug is debuggable from a log);
 *   * the same app source runs on a backend with no canvas — it simply never
 *     receives a `setCanvas`.
 *
 * Coordinate system: CSS pixels, origin at the element's **content box**
 * top-left (padding excluded), y down — like a browser canvas inside a padded
 * parent.
 *
 * Divergences from `CanvasRenderingContext2D` (all deliberate, all small):
 *   * `text(t, x, y)` treats `y` as the **top of the line box**, not the
 *     baseline (the host exposes no ascent to place a baseline).
 *   * No `clearRect`: a frame is stateless — the list is repainted over the
 *     element's own background — so `reset()` is how you start over.
 *   * No transforms, gradients, images, `measureText`, shadows or clipping;
 *     `arc` becomes a sampled polyline.
 *   * State is small and per-context: `fillStyle` / `strokeStyle` / `lineWidth`
 *     / `fontSize` / `fontWeight` / `globalAlpha`.
 *
 * Colors accept anything the host's parser does: `#rgb`, `#rrggbb`,
 * `#rrggbbaa`, `rgb()`, `rgba()`, or a bare hex number.
 *
 * Perry subset rules apply (no try/catch, no for...of, no Proxy).
 */

import { h, mergeStyleInto, setCanvas } from "./io";
import type { Cmd, El, Style } from "./io";

/**
 * The display-list command type lives in io.ts — it is the protocol shape
 * (mirrors host/src/draw.rs::parse_cmd). `k` selects the primitive; the other
 * keys are that primitive's arguments (`x/y/w/h/r`, `cx/cy/rx/ry`, `pts`,
 * `t/size/weight`, `fill`/`stroke`/`line`/`a`) and an absent key means the
 * channel is simply not set. The host validates whatever arrives and logs the
 * rejects.
 */

/** Per-shape paint override; omitted fields fall back to the ctx state. */
export interface ShapeOpts {
    /** Fill color, or `null` for none. Filled shapes default to `fillStyle`. */
    fill?: string | null;
    /** Stroke color, or `null` (default) for none. */
    stroke?: string | null;
    /** Stroke width; defaults to `ctx.lineWidth`. */
    line?: number;
    /** Corner radius in px (`fillRect`/`strokeRect` only). */
    radius?: number;
}

export interface Ctx {
    fillStyle: string;
    strokeStyle: string;
    lineWidth: number;
    /** Used by `text()`; the host falls back to its own default. */
    fontSize: number;
    fontWeight: string;
    /** 0..1, multiplied into every color of a command. */
    globalAlpha: number;

    /** Drop the recorded list and the current path (start of a frame). */
    reset(): void;

    // --- immediate shapes ---
    fillRect(x: number, y: number, w: number, h: number, radius?: number): void;
    strokeRect(x: number, y: number, w: number, h: number, radius?: number): void;
    line(x1: number, y1: number, x2: number, y2: number, opts?: ShapeOpts): void;
    circle(cx: number, cy: number, r: number, opts?: ShapeOpts): void;
    ellipse(cx: number, cy: number, rx: number, ry: number, opts?: ShapeOpts): void;
    /** `pts` is `[[x, y], …]`; always closed. */
    poly(pts: number[][], opts?: ShapeOpts): void;
    /** `y` is the top of the line box, not the baseline. */
    text(t: string, x: number, y: number, opts?: ShapeOpts): void;

    // --- path API (Canvas2D-shaped) ---
    beginPath(): void;
    moveTo(x: number, y: number): void;
    lineTo(x: number, y: number): void;
    arc(cx: number, cy: number, r: number, a0?: number, a1?: number): void;
    closePath(): void;
    fill(opts?: ShapeOpts): void;
    stroke(opts?: ShapeOpts): void;
}

export interface CanvasHandle {
    /** The element to mount (`tag: "canvas"`). */
    el: El;
    ctx: Ctx;
    readonly width: number;
    readonly height: number;
    /** Ship the recorded display list to the host (one `setCanvas` op). */
    present(): void;
    /** Commands currently recorded — a redraw that draws nothing is a bug. */
    count(): number;
}

/**
 * Element style: a canvas has no content to shrink-wrap around, so the size must
 * be explicit. The caller's style comes second, so it can override either axis
 * (`{ width: "full" }` for a responsive chart).
 */
function canvasStyle(width: number, height: number, style?: Style): Style {
    const s: Style = { width: width, height: height };
    // Explicit field merge — Object.keys dynamic writes are refused (SC1090).
    if (style !== undefined && style !== null) mergeStyleInto(s, style);
    return s;
}

// SIN_TABLE[i] = sin(2*PI*i/N), i=0..N-1 — precomputed so the static
// (scriptc) build never calls Math.sin/cos (SC2012: no libm surface).
// arcPoints() indexes this with linear interpolation; 1/128 quantization
// of the angle is far below the 48-segment polyline resolution.
const SIN_TABLE: number[] = [
     0.0000000,  0.0490677,  0.0980171,  0.1467305,  0.1950903,  0.2429802,  0.2902847,  0.3368899,
     0.3826834,  0.4275551,  0.4713967,  0.5141027,  0.5555702,  0.5956993,  0.6343933,  0.6715590,
     0.7071068,  0.7409511,  0.7730105,  0.8032075,  0.8314696,  0.8577286,  0.8819213,  0.9039893,
     0.9238795,  0.9415441,  0.9569403,  0.9700313,  0.9807853,  0.9891765,  0.9951847,  0.9987955,
     1.0000000,  0.9987955,  0.9951847,  0.9891765,  0.9807853,  0.9700313,  0.9569403,  0.9415441,
     0.9238795,  0.9039893,  0.8819213,  0.8577286,  0.8314696,  0.8032075,  0.7730105,  0.7409511,
     0.7071068,  0.6715590,  0.6343933,  0.5956993,  0.5555702,  0.5141027,  0.4713967,  0.4275551,
     0.3826834,  0.3368899,  0.2902847,  0.2429802,  0.1950903,  0.1467305,  0.0980171,  0.0490677,
     0.0000000, -0.0490677, -0.0980171, -0.1467305, -0.1950903, -0.2429802, -0.2902847, -0.3368899,
    -0.3826834, -0.4275551, -0.4713967, -0.5141027, -0.5555702, -0.5956993, -0.6343933, -0.6715590,
    -0.7071068, -0.7409511, -0.7730105, -0.8032075, -0.8314696, -0.8577286, -0.8819213, -0.9039893,
    -0.9238795, -0.9415441, -0.9569403, -0.9700313, -0.9807853, -0.9891765, -0.9951847, -0.9987955,
    -1.0000000, -0.9987955, -0.9951847, -0.9891765, -0.9807853, -0.9700313, -0.9569403, -0.9415441,
    -0.9238795, -0.9039893, -0.8819213, -0.8577286, -0.8314696, -0.8032075, -0.7730105, -0.7409511,
    -0.7071068, -0.6715590, -0.6343933, -0.5956993, -0.5555702, -0.5141027, -0.4713967, -0.4275551,
    -0.3826834, -0.3368899, -0.2902847, -0.2429802, -0.1950903, -0.1467305, -0.0980171, -0.0490677,
];

const TAU = 6.2831853;

/** sin via table lookup + linear interpolation (static-build safe). */
function sinT(x: number): number {
    let t = x % TAU;
    if (t < 0) t = t + TAU;
    const fi = (t / TAU) * 128;
    const i0 = Math.floor(fi);
    const i1 = i0 + 1 === 128 ? 0 : i0 + 1;
    const frac = fi - i0;
    return SIN_TABLE[i0] * (1 - frac) + SIN_TABLE[i1] * frac;
}

/** cos via the same table (cos(x) = sin(x + PI/2) shifted by quarter turn). */
function cosT(x: number): number {
    return sinT(x + 1.5707963);
}
/** Sample an arc into line segments (the host draws polylines only). */
function arcPoints(cx: number, cy: number, r: number, a0: number, a1: number, pts: number[][]): void {
    let span = a1 - a0;
    if (span < 0) span = -span;
    const n = Math.max(4, Math.ceil((span / TAU) * 48));
    for (let i = 0; i <= n; i++) {
        const a = a0 + ((a1 - a0) * i) / n;
        pts.push([cx + r * cosT(a), cy + r * sinT(a)]);
    }
}

export function createCanvas(width: number, height: number, style?: Style): CanvasHandle {
    const el: El = h("canvas", { style: canvasStyle(width, height, style) });
    // Commands are built by assigning keys one by one (see `paintFor`), and
    // `Cmd`'s `k` is set by the call site — hence all-optional fields.
    let cmds: Cmd[] = [];
    let path: number[][] = [];
    let closed = false;

    /**
     * Paint channels for one command.
     *
     * `defaultFill` / `defaultStroke` are what the call site implies: a filled
     * shape defaults to `fillStyle`, a stroked one to `strokeStyle` and
     * `lineWidth`. `null` means "this channel is not part of this command",
     * which `draw.rs` represents by the key being absent.
     */
    function paintFor(
        opts: ShapeOpts | undefined,
        defaultFill: string | null,
        defaultStroke: string | null
    ): Cmd {
        const cmd: Cmd = {};
        const o: ShapeOpts = opts === undefined ? {} : opts;
        const fill = o.fill !== undefined ? o.fill : defaultFill;
        const stroke = o.stroke !== undefined ? o.stroke : defaultStroke;
        if (fill !== null && fill !== undefined) cmd.fill = fill;
        if (stroke !== null && stroke !== undefined) {
            cmd.stroke = stroke;
            cmd.line = o.line !== undefined ? o.line : ctx.lineWidth;
        }
        if (ctx.globalAlpha !== 1) cmd.a = ctx.globalAlpha;
        return cmd;
    }

    const ctx: Ctx = {
        fillStyle: "#4f8cff",
        strokeStyle: "#e8eaf2",
        lineWidth: 1,
        fontSize: 12,
        fontWeight: "normal",
        globalAlpha: 1,

        reset: function (): void {
            cmds = [];
            path = [];
            closed = false;
        },

        fillRect: function (x, y, w, h, radius) {
            const cmd: Cmd = paintFor(undefined, ctx.fillStyle, null);
            cmd.k = "rect";
            cmd.x = x;
            cmd.y = y;
            cmd.w = w;
            cmd.h = h;
            if (radius !== undefined && radius > 0) cmd.r = radius;
            cmds.push(cmd);
        },

        strokeRect: function (x, y, w, h, radius) {
            const cmd: Cmd = paintFor(undefined, null, ctx.strokeStyle);
            cmd.k = "rect";
            cmd.x = x;
            cmd.y = y;
            cmd.w = w;
            cmd.h = h;
            if (radius !== undefined && radius > 0) cmd.r = radius;
            cmds.push(cmd);
        },

        line: function (x1, y1, x2, y2, opts) {
            const cmd: Cmd = paintFor(opts, null, ctx.strokeStyle);
            cmd.k = "line";
            cmd.x1 = x1;
            cmd.y1 = y1;
            cmd.x2 = x2;
            cmd.y2 = y2;
            cmds.push(cmd);
        },

        circle: function (cx, cy, r, opts) {
            const cmd: Cmd = paintFor(opts, ctx.fillStyle, null);
            cmd.k = "ellipse";
            cmd.cx = cx;
            cmd.cy = cy;
            cmd.rx = r;
            cmd.ry = r;
            cmds.push(cmd);
        },

        ellipse: function (cx, cy, rx, ry, opts) {
            const cmd: Cmd = paintFor(opts, ctx.fillStyle, null);
            cmd.k = "ellipse";
            cmd.cx = cx;
            cmd.cy = cy;
            cmd.rx = rx;
            cmd.ry = ry;
            cmds.push(cmd);
        },

        poly: function (pts, opts) {
            if (pts.length < 2) return;
            const cmd: Cmd = paintFor(opts, ctx.fillStyle, null);
            cmd.k = "poly";
            cmd.pts = pts;
            cmd.close = true;
            cmds.push(cmd);
        },

        text: function (t, x, y, opts) {
            const cmd: Cmd = paintFor(opts, ctx.fillStyle, null);
            cmd.k = "text";
            cmd.x = x;
            cmd.y = y;
            cmd.t = t;
            cmd.size = ctx.fontSize;
            cmd.weight = ctx.fontWeight;
            cmds.push(cmd);
        },

        beginPath: function () {
            path = [];
            closed = false;
        },

        moveTo: function (x, y) {
            path.push([x, y]);
        },

        lineTo: function (x, y) {
            path.push([x, y]);
        },

        arc: function (cx, cy, r, a0, a1) {
            // Unlike a browser, an arc always appends to the current path — so
            // `moveTo(cx + r, cy)` + `arc(cx, cy, r, 0, τ)` is the seamless way
            // to close a circle (the duplicated point costs nothing in a
            // polyline). Starting a fresh sub-path would need sub-path support
            // the display list does not have.
            arcPoints(cx, cy, r, a0 === undefined ? 0 : a0, a1 === undefined ? TAU : a1, path);
        },

        closePath: function () {
            closed = true;
        },

        fill: function (opts) {
            if (path.length < 2) return;
            const cmd: Cmd = paintFor(opts, ctx.fillStyle, null);
            cmd.k = "poly";
            cmd.pts = path.slice(0);
            cmd.close = true;
            cmds.push(cmd);
        },

        stroke: function (opts) {
            if (path.length < 2) return;
            const cmd: Cmd = paintFor(opts, null, ctx.strokeStyle);
            cmd.k = "poly";
            cmd.pts = path.slice(0);
            cmd.close = closed;
            cmds.push(cmd);
        },
    };

    return {
        el: el,
        ctx: ctx,
        width: width,
        height: height,
        present: function (): void {
            // A copy: the op is flushed on the next microtask, and the caller may
            // already be drawing into `cmds` again (a `reset()` before the flush
            // would otherwise empty the array the host is about to read).
            setCanvas(el, cmds.slice(0));
        },
        count: function (): number {
            return cmds.length;
        },
    };
}
