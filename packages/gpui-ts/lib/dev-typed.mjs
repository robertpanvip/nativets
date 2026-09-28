// Typed dev bundle — `nativets dev` 的编译核心（无 esbuild）。
//
// 全 typed 管线：TypeScript compiler API 两步走
//   1. tsx_typed_transform：JSX → h() 调用（类型全保留）
//   2. transpileModule：擦除类型注解，产出 ESM JS
// 然后把 ESM 包成浏览器风格 registry IIFE：
//   - import 依赖解析为 registry 内的模块 id（拓扑序加载）
//   - bare specifier "nativets" 映射到包内 runtime/index.js（同 esbuild 插件）
//   - 产物是一段可直接 (0,eval) 的 IIFE，重复 eval 即整树重建（热重载）
//
// 与 build-package.mjs 的 esbuild 产物语义一致（IIFE、charset ascii），
// 只是编译器换成了 tsc —— perry 验证过的 typed 管线，跨模块 signal getter
// 不会被误降级（repo 坑 #8）。
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

/** 与 host/src/quickjs.rs TAIL_MARKER / build.mjs TAIL_MARKER 相同约定。 */
export const DEV_MODE_ENV = "GPUI_TS_DEV_BUNDLE";

const thisFile = fileURLToPath(import.meta.url);
const jsxLowerRoot = path.resolve(thisFile, "..", "..", "..", "..", "ui", "tools", "tsx_typed_transform.mjs");

/**
 * Resolve the TSX lowerer from the monorepo checkout (`ui/tools`), falling
 * back to a sibling location inside the npm package. Returns null when the
 * package install has no toolchain (dev-typed is a checkout-only feature).
 */
async function loadLowerer() {
    for (const candidate of [
        jsxLowerRoot,
        path.join(path.dirname(thisFile), "tsx_typed_transform.mjs"),
    ]) {
        if (fs.existsSync(candidate)) {
            return (await import(pathToFileURL(candidate).href)).transformTsxTyped;
        }
    }
    return null;
}

/**
 * Collect the transitive module graph of `entry` by walking static imports.
 * Handles relative specifiers (with/without extension) and the bare
 * `nativets` runtime. Cycles are tolerated (visited set).
 */
export function collectGraph(entry) {
    const files = []; // ordered, entry first
    const seen = new Set();
    const nativetsRuntime = null; // resolved by caller when needed

    function resolveRelative(spec, fromDir) {
        let p = path.resolve(fromDir, spec);
        const tries = [
            p,
            p + ".ts", p + ".tsx",
            path.join(p, "index.ts"), path.join(p, "index.tsx"),
        ];
        for (const t of tries) {
            if (fs.existsSync(t) && fs.statSync(t).isFile()) return t;
        }
        return null;
    }

    function walk(file) {
        if (seen.has(file)) return;
        seen.add(file);
        files.push(file);
        const src = fs.readFileSync(file, "utf8");
        const re = /(?:^|\n)\s*import\s+(?:[\s\S]*?from\s+)?["']([^"']+)["']/g;
        let m;
        const dir = path.dirname(file);
        while ((m = re.exec(src)) !== null) {
            const spec = m[1];
            if (spec === "nativets") continue; // runtime injected separately
            if (spec.startsWith(".")) {
                const r = resolveRelative(spec, dir);
                if (r) walk(r);
                else {
                    throw new Error(
                        `nativets dev: cannot resolve import "${spec}" from ${file}`,
                    );
                }
            } else {
                // Other bare specifiers (e.g. "typescript" in tools) are not
                // part of an app graph — fail loudly so the app fixes its
                // imports instead of silently shipping an empty module.
                throw new Error(
                    `nativets dev: bare import "${spec}" (from ${file}) is not supported — ` +
                    `only relative paths and "nativets" are bundled`,
                );
            }
        }
    }
    walk(entry);
    return files;
}

/**
 * Bundle `entry` (tsx/tsx graph) into one IIFE string via the typed pipeline.
 *
 * @param {string} entry absolute path to the app entry (.ts/.tsx)
 * @param {{nativetsRuntime?: string}} opts  path to the nativets runtime
 *        entry module (its own ESM graph gets the same treatment).
 * @returns {Promise<string>} JS IIFE source (ascii-escaped)
 */
export async function bundleTyped(entry, opts = {}) {
    const lower = await loadLowerer();
    if (!lower) {
        throw new Error("nativets dev: tsx_typed_transform.mjs not found (checkout required)");
    }
    // eslint-disable-next-line no-undef
    let ts = null;
    try {
        ts = await import("typescript");
    } catch {
        // Fallback: the monorepo keeps typescript next to the ui sources
        // (packages/gpui-ts has no node_modules of its own).
        const candidates = [
            path.resolve(thisFile, "..", "..", "..", "..", "ui", "node_modules", "typescript", "lib", "typescript.js"),
            path.resolve(path.dirname(entry), "..", "node_modules", "typescript", "lib", "typescript.js"),
        ];
        for (const c of candidates) {
            if (fs.existsSync(c)) {
                ts = await import(pathToFileURL(c).href);
                break;
            }
        }
    }
    if (!ts) {
        throw new Error("nativets dev: typescript module not found — run npm install in ui/");
    }

    const runtimeEntry = opts.nativetsRuntime ?? null;
    const appFiles = collectGraph(entry);
    const runtimeFiles = runtimeEntry ? collectGraph(runtimeEntry) : [];

    // module registry: id → transpiled CJS-ish factory source
    const modules = new Map();

    function toId(p) { return "m" + modules.size + "__" + path.basename(p).replace(/\W/g, "_"); }

    function moduleSource(file, isEntry = false) {
        let src = fs.readFileSync(file, "utf8");
        if (file.endsWith(".tsx")) src = lower(src);
        // 类型擦除 + target 降级（registry 需要无 import 语句的模块体）
        const out = ts.transpileModule(src, {
            compilerOptions: {
                module: ts.ModuleKind.CommonJS,
                target: ts.ScriptTarget.ES2020,
                jsx: ts.JsxEmit.Preserve, // transform 已做，Preserve 保持
                esModuleInterop: true,
            },
            fileName: file,
        }).outputText;
        return out;
    }

    // Runtime modules first (so app imports resolve), then the app graph.
    for (const f of runtimeFiles) {
        const id = toId(f);
        modules.set(f, { id, source: moduleSource(f) });
    }
    for (const f of appFiles) {
        if (modules.has(f)) continue;
        modules.set(f, { id: toId(f), source: moduleSource(f, f === entry) });
    }

    // Rewrite each module's require() specifiers to registry ids.
    function moduleRecord(file) {
        const { id, source } = modules.get(file);
        const dir = path.dirname(file);
        const rewritten = source.replace(
            /require\((["'])([^"']+)\1\)/g,
            (whole, q, spec) => {
                if (spec === "nativets") {
                    const rt = runtimeEntry ? modules.get(runtimeEntry) : null;
                    if (rt) return `require("${rt.id}")`;
                    return whole; // no runtime injected: leave (will throw at runtime)
                }
                let p = path.resolve(dir, spec);
                const tries = [p, p + ".ts", p + ".tsx",
                    path.join(p, "index.ts"), path.join(p, "index.tsx")];
                const hit = tries.find((t) => modules.has(t));
                if (!hit) {
                    throw new Error(`nativets dev: unresolved require("${spec}") in ${file}`);
                }
                return `require("${modules.get(hit).id}")`;
            },
        );
        return { id, source: rewritten };
    }

    const records = [...modules.keys()].map((f) => moduleRecord(f));
    const entryId = modules.get(entry).id;

    // IIFE shell: CommonJS-style registry (same shape esbuild's IIFE emits).
    const ascii = (s) => s.replace(/[^\x00-\x7F]/g, (ch) => {
        const h = ch.codePointAt(0).toString(16).padStart(4, "0");
        return `\\u${h}`;
    });
    const body = records
        .map((r) => `"${r.id}": function (exports, require, module) {\n${r.source}\n},`)
        .join("\n");
    const iife = `\
(function () {
var __mods = {
${body}
};
var __cache = {};
function __require(id) {
    if (__cache[id]) return __cache[id].exports;
    var mod = { exports: {} };
    __cache[id] = mod;
    var fn = __mods[id];
    if (!fn) throw new Error("nativets dev: module not found: " + id);
    fn(mod.exports, __require, mod);
    return mod.exports;
}
__require("${entryId}");
})();
`;
    return ascii(iife);
}
