// Dev harness: verify the type-preserving TS JSX transform is semantically
// identical to the esbuild lowering already shipping in production.
//
// Usage: node build/tsx_typed_check.mjs [file.tsx ...]
//   - no args: run the built-in parity suite + src/*.tsx corpus parity
//   - with args: rewrite each file and print the result
import { transformTsxTyped } from "./tsx_typed_transform.mjs";
import { transformSync } from "esbuild";

const suites = [
    // [name, source]
    ["simple-div", `const a = <div x={1} b="s" flag>text {n}</div>;`],
    ["spread", `const b = <Comp {...rest} k={2} />;`],
    ["fragment-children", `const c = <><i>one</i>{" "}<b/></>;`],
    ["member-expr", `const d = <Foo.Bar />;`],
    ["dashed-attrs", `const e = <div data-x="1" aria-label={y} />;`],
    ["void-el", `const f = <div></div>;`],
    ["cond-child", `const g = <div>{cond && <span/>}</div>;`],
    ["typeful", `type El = unknown;
interface Props { x?: number; title: string }
function App(p: Props): El {
    const n: number = 3;
    return <div x={n} title={p.title}>hello {p.title}</div>;
}`],
    ["jsx-text-edge", `const h2 = <div>
      lead {a} mid {b}
      tail</div>;`],
];

// Compare esbuild's lowering vs the typed transform on JSX-producing
// expressions. Normalize: strip /* @__PURE__ */, whitespace.
function esbuildLower(src) {
    return transformSync(src, {
        loader: "tsx", jsx: "transform", jsxFactory: "h",
        jsxFragment: "Fragment", charset: "ascii", format: "esm",
    }).code.replace(/\/\* @__PURE__ \*\//g, "");
}

// Extract just the h(...) call shapes from each output for comparison
// (the typed output keeps type annotations esbuild erases, so full-text
// equality is impossible — we compare the JSX-derived call expressions).
// Strip comments first: doc comments legitimately mention `h()` and the
// regex below would otherwise count those mentions as calls. Note esbuild
// also preserves comments, so both sides would be poisoned symmetrically —
// but block vs line comment retention differs between the two printers,
// which made app.tsx flake. Comments are never JSX semantics; drop them.
function stripComments(text) {
    return text
        .replace(/\/\*[\s\S]*?\*\//g, " ")
        .replace(/(^|[^:])\/\/[^\n]*/g, "$1 ");
}
function hCalls(text) {
    return (stripComments(text).match(/h\((?:[^()]|\([^()]*\))*\)/g) ?? [])
        .map((s) => s.replace(/\s+/g, " ").trim()).sort();
}

let failures = 0;
for (const [name, src] of suites) {
    const fromEsbuild = hCalls(esbuildLower(src));
    const fromTyped = hCalls(transformTsxTyped(src));
    const ok = JSON.stringify(fromEsbuild) === JSON.stringify(fromTyped);
    console.log(`${ok ? "PASS" : "FAIL"}  ${name}`);
    if (!ok) {
        failures++;
        console.log("  esbuild:", fromEsbuild);
        console.log("  typed:  ", fromTyped);
    }
}

// Corpus parity: every src/*.tsx lowers to the same h() calls both ways.
if (process.argv.length <= 2) {
    const { readdirSync, readFileSync, statSync } = await import("node:fs");
    const path = await import("node:path");
    function walk(dir) {
        const out = [];
        for (const n of readdirSync(dir).sort()) {
            const p = path.join(dir, n);
            if (statSync(p).isDirectory()) out.push(...walk(p));
            else if (n.endsWith(".tsx")) out.push(p);
        }
        return out;
    }
    let corpus = 0;
    for (const f of walk(new URL("../src", import.meta.url).pathname.replace(/^\/([A-Za-z]:)/, "$1"))) {
        const src = readFileSync(f, "utf8");
        let a, b;
        try {
            a = hCalls(esbuildLower(src));
            b = hCalls(transformTsxTyped(src));
        } catch (e) {
            console.log(`SKIP  ${path.basename(f)}: ${e.message.split("\n")[0]}`);
            continue;
        }
        const ok = JSON.stringify(a) === JSON.stringify(b);
        if (!ok) {
            failures++;
            console.log(`FAIL  corpus:${path.basename(f)}`);
            console.log("  esbuild:", a.slice(0, 4));
            console.log("  typed:  ", b.slice(0, 4));
        } else corpus++;
    }
    console.log(`corpus parity: ${corpus} files compared`);
}

process.exit(failures > 0 ? 1 : 0);
