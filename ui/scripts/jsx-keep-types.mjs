/**
 * JSX-only desugar: TSX -> TS keeping ALL TypeScript syntax (annotations,
 * interfaces, generics), transforming only JSX elements into h() calls.
 *
 * This is the piece esbuild cannot do (it's a transpiler — types always get
 * erased) and the piece scriptc needs (its checker refuses untyped graphs
 * with SC4005 and does not read tsconfig jsx settings in 0.1.3).
 *
 * Uses the TypeScript compiler API: parse -> transform JSX nodes only ->
 * print back as .ts.
 *
 * Run: node scripts/jsx-keep-types.mjs   (from ui/)
 */
import ts from "typescript";
import { mkdirSync, readFileSync, readdirSync, rmSync, writeFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const here = path.dirname(fileURLToPath(import.meta.url)); // ui/scripts
const uiDir = path.dirname(here);
const outDir = path.join(uiDir, "build", "sc-graph");
rmSync(outDir, { recursive: true, force: true });
mkdirSync(outDir, { recursive: true });

// The typed graph: every .ts/.tsx module the UI is made of.
const srcDir = path.join(uiDir, "src");
const files = readdirSync(srcDir).filter((f) => /\.(tsx|ts)$/.test(f) && !f.endsWith(".d.ts"));

function desugar(sourceText, fileName) {
    const sf = ts.createSourceFile(fileName, sourceText, ts.ScriptTarget.ES2022, true, ts.ScriptKind.TSX);

    const transformer = (ctx) => {
        const visit = (node) => {
            // JSX -> h(...) calls; fragments -> Fragment(...) calls
            if (ts.isJsxElement(node) || ts.isJsxSelfClosingElement(node) || ts.isJsxFragment(node)) {
                return desugarJsx(node, ctx);
            }
            return ts.visitEachChild(node, visit, ctx);
        };
        visitRef.visit = visit;
        return (root) => ts.visitNode(root, visit);
    };
    const visitRef = { visit: null };

    // Desugar one JSX node into a h(tag, props, ...children) expression.
    function desugarJsx(node, ctx) {
        if (node === undefined || node === null) {
            return ts.factory.createStringLiteral("");
        }
        if (ts.isJsxFragment(node)) {
            const args = node.children.map((c) => desugarChild(c, ctx));
            return ts.factory.createCallExpression(ts.factory.createIdentifier("Fragment"), undefined, args);
        }
        const opening = ts.isJsxSelfClosingElement(node)
            ? node
            : (ts.isJsxElement(node) ? node.openingElement : undefined);
        if (opening === undefined) {
            // Not a real JSX element (JsxClosingElement etc.) — keep as-is.
            return node;
        }
        const tagName = opening.tagName;
        const tagExpr = ts.isIdentifier(tagName)
            ? (isCapitalized(tagName.text)
                ? ts.factory.createIdentifier(tagName.text)
                : ts.factory.createStringLiteral(tagName.text))
            : tagName; // member expressions pass through

        const props = buildProps(opening.attributes);
        const children = (ts.isJsxSelfClosingElement(node) ? [] : node.children.map((c) => desugarChild(c, ctx)))
            .filter((n) => !(ts.isStringLiteral(n) && n.text === ""));
        return ts.factory.createCallExpression(ts.factory.createIdentifier("h"), undefined, [tagExpr, props, ...children]);
    }

    function desugarChild(child, ctx) {
        if (ts.isJsxText(child)) {
            const raw = child.text.trim();
            return raw ? ts.factory.createCallExpression(
                ts.factory.createIdentifier("text"),
                undefined,
                [ts.factory.createStringLiteral(raw)],
            ) : ts.factory.createStringLiteral(""); // filtered out below
        }
        if (ts.isJsxExpression(child)) {
            if (child.expression === undefined) return ts.factory.createStringLiteral("");
            // The inner expression may itself contain JSX (e.g. arrow bodies in
            // For(...)) — re-run the full visitor over it, or raw JSX leaks.
            return ts.visitNode(child.expression, visitRef.visit);
        }
        if (ts.isJsxElement(child) || ts.isJsxSelfClosingElement(child) || ts.isJsxFragment(child)) {
            return desugarJsx(child, ctx);
        }
        // Unknown child kind — drop it rather than poison the printer.
        return ts.factory.createStringLiteral("");
    }

    function unwrapExpr(init) {
        // prop={expr} wraps expr in JsxExpression — print needs the inner one.
        if (init && ts.isJsxExpression(init)) {
            const inner = init.expression ?? ts.factory.createTrue();
            return ts.visitNode(inner, visitRef.visit); // attribute values may contain JSX too
        }
        return init;
    }

    function buildProps(attrs) {
        const propAssignments = [];
        const spread = [];
        for (const a of attrs.properties) {
            if (ts.isJsxSpreadAttribute(a)) {
                spread.push(a.expression);
                continue;
            }
            const name = a.name;
            const init = unwrapExpr(a.initializer ?? ts.factory.createTrue());
            const key = ts.isIdentifier(name) ? ts.factory.createStringLiteral(name.text) : name;
            propAssignments.push(ts.factory.createPropertyAssignment(key, init));
        }
        if (spread.length === 0) {
            return ts.factory.createObjectLiteralExpression(propAssignments, true);
        }
        // { ...a, k: v } -> Object.assign({}, a, { k: v })
        return ts.factory.createCallExpression(
            ts.factory.createPropertyAccessExpression(ts.factory.createIdentifier("Object"), "assign"),
            undefined,
            [ts.factory.createObjectLiteralExpression(), ...spread, ts.factory.createObjectLiteralExpression(propAssignments, true)],
        );
    }

    function isCapitalized(s) {
        return /^[A-Z]/.test(s);
    }

    const result = ts.transform(sf, [transformer]);
    const printer = ts.createPrinter({ newLine: ts.NewLineKind.LineFeed });
    return printer.printFile(result.transformed[0]);
}

let ok = 0;
for (const f of files) {
    const src = readFileSync(path.join(srcDir, f), "utf8");
    const out = desugar(src, f);
    const outName = f.replace(/\.tsx$/, ".ts");
    writeFileSync(path.join(outDir, outName), out);
    ok++;
}
console.log(`desugared ${ok} files -> ${outDir}`);

// sanity: types kept?
const app = readFileSync(path.join(outDir, "app.ts"), "utf8");
console.log("App signature:", app.match(/export function App\([^)]*\)/)?.[0] ?? "NOT FOUND");
