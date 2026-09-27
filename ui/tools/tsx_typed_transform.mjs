// Type-preserving TSX → TS lowering for the perry AOT pipeline.
//
// Why not esbuild: esbuild erases type annotations while lowering JSX. That
// is fine for the runtime (perry builds HIR from JS semantics), but perry
// ships a full TypeScript checker and its typed pipeline (verbatim .ts
// copies) is the one that historically produced known-good output — under
// the erased pipeline cross-module signal getters once resolved to
// `undefined` (repo 坑 #8). Giving perry typed .tsx output puts the whole
// gen/ tree on the typed pipeline.
//
// The transform: parse with the TypeScript compiler API, replace each
// JsxElement/JsxSelfClosing/JsxFragment with the equivalent `h(...)` call
// expression, and print the AST back to TS source. Everything else —
// type aliases, interfaces, annotations, generics — flows through
// untouched.
//
// Semantics mirror esbuild's classic transform exactly (verified by
// build/tsx_typed_check.mjs):
//   <div a={1} b="s" flag>            → h("div", { a: 1, b: "s", flag: true })
//   <Comp {...rest} k={2} />          → h(Comp, { ...rest, k: 2 })
//   <Foo.Bar />                       → h(Foo.Bar, null)
//   <><i>x</i></>                     → h(Fragment, null, h("i", null, "x"))
//   text {expr}                       → "text ", expr        (as trailing args)
//   <div>{cond && <span/>}</div>      → h("div", null, cond && h("span", null))
// Key rules learned from esbuild's output shape:
//   - JsxText is preserved verbatim as a string literal (whitespace and
//     newlines included — JSX text semantics are position-sensitive).
//   - A JsxExpression whose expression is null (`{}`) is dropped entirely.
//   - An JsxAttribute without initializer becomes `name: true`.
//   - Attribute names that are not valid identifiers are quoted.
//   - Attribute values that are string literals stay string literals;
//     everything else keeps its expression shape.
import typescript from "typescript";

/** Print an attribute-name node the way it appeared in source. */
function attrNameText(node) {
    if (typescript.isIdentifier(node)) return node.text;
    // JsxNamespacedName (attr with ns) and other shapes: use raw source text
    return node.getText();
}

/**
 * JSX text semantics (matches esbuild/Babel): strip leading whitespace that
 * contains a line break and trailing whitespace that contains a line break;
 * keep mid-line spacing verbatim. Returns null when nothing remains.
 */
function trimJsxText(raw) {
    let s = raw;
    if (/^\s*[\r\n]/.test(s)) s = s.replace(/^\s*[\r\n]+\s*/, "");
    if (/[\r\n]\s*$/.test(s)) s = s.replace(/[\r\n]+\s*$/, "");
    return s.length > 0 ? s : null;
}

/** Lower one JSX element/fragment to a CallExpression node. */
function lowerJsx(node, visit) {
    const factory = typescript.factory;

    let tagExpr;
    let attrsNode;
    let children;

    if (typescript.isJsxElement(node)) {
        tagExpr = node.openingElement.tagName;
        attrsNode = node.openingElement.attributes;
        children = node.children;
    } else if (typescript.isJsxSelfClosingElement(node)) {
        tagExpr = node.tagName;
        attrsNode = node.attributes;
        children = [];
    } else if (typescript.isJsxFragment(node)) {
        tagExpr = factory.createIdentifier("Fragment");
        attrsNode = undefined;
        children = node.children;
    } else {
        throw new Error(`lowerJsx: unexpected node kind ${node.kind}`);
    }

    // ---- props object -----------------------------------------------------
    const props = [];
    if (attrsNode) {
        for (const a of attrsNode.properties) {
            if (typescript.isJsxAttribute(a)) {
                const name = attrNameText(a.name);
                const needsQuote = !/^[A-Za-z_$][A-Za-z0-9_$]*$/.test(name);
                const nameNode = needsQuote
                    ? factory.createStringLiteral(name)
                    : factory.createIdentifier(name);
                let valueNode;
                if (a.initializer === undefined) {
                    valueNode = factory.createTrue();
                } else if (typescript.isStringLiteral(a.initializer)) {
                    valueNode = factory.createStringLiteral(a.initializer.text);
                } else if (typescript.isJsxExpression(a.initializer) && a.initializer.expression) {
                    // Nested JSX inside an attribute value must be lowered
                    // too — run it through the same visitor.
                    valueNode = a.initializer.expression;
                    if (typescript.isJsxElement(valueNode) ||
                        typescript.isJsxSelfClosingElement(valueNode) ||
                        typescript.isJsxFragment(valueNode)) {
                        valueNode = visit(valueNode);
                    }
                } else if (typescript.isJsxExpression(a.initializer)) {
                    valueNode = factory.createTrue(); // attr={}: degenerate, treat as true
                } else {
                    // JsxSpreadAttribute handled below; anything else: raw
                    valueNode = a.initializer;
                }
                props.push(factory.createPropertyAssignment(nameNode, valueNode));
            } else if (typescript.isJsxSpreadAttribute(a)) {
                props.push(factory.createSpreadAssignment(a.expression));
            } else {
                throw new Error(`lowerJsx: unexpected attribute kind ${a.kind}`);
            }
        }
    }
    const propsObject = props.length > 0
        ? factory.createObjectLiteralExpression(props, false)
        : factory.createNull();

    // ---- children ----------------------------------------------------------
    let childArgs = [];
    for (const c of children) {
        if (typescript.isJsxText(c)) {
            const text = trimJsxText(c.text);
            if (text !== null) childArgs.push(factory.createStringLiteral(text));
        } else if (typescript.isJsxExpression(c)) {
            if (c.expression) {
                // Expressions may contain nested JSX (e.g. `cond && <span/>`):
                // lower those with the visitor as well.
                childArgs.push(visit(c.expression));
            }
            // expression === undefined → `{}` or comment container: drop
        } else {
            childArgs.push(lowerJsx(c, visit));
        }
    }
    // Intrinsic tags (lowercase identifiers) become string literals;
    // component references (identifiers with uppercase, member exprs like
    // Foo.Bar) stay as expressions.
    let tagArg;
    if (typescript.isJsxFragment(node)) {
        tagArg = factory.createIdentifier("Fragment");
    } else if (typescript.isIdentifier(tagExpr) && /^[a-z]/.test(tagExpr.text)) {
        tagArg = factory.createStringLiteral(tagExpr.text);
    } else {
        tagArg = tagExpr;
    }

    const call = factory.createCallExpression(
        factory.createIdentifier("h"),
        undefined,
        [tagArg, propsObject, ...childArgs],
    );
    return call;
}

/**
 * Lower all JSX in `source` to h()/Fragment calls, preserving every type
 * annotation and non-JSX construct.
 *
 * @param {string} source TSX source text
 * @returns {string} TS source text with JSX lowered, types intact
 */
export function transformTsxTyped(source) {
    const sf = typescript.createSourceFile(
        "app.tsx", source, typescript.ScriptTarget.ES2020,
        /* setParentNodes */ true, typescript.ScriptKind.TSX,
    );

    const transformer = (context) => (rootNode) => {
        const visit = (node) => {
            if (typescript.isJsxElement(node) ||
                typescript.isJsxSelfClosingElement(node) ||
                typescript.isJsxFragment(node)) {
                return lowerJsx(node, visit);
            }
            return typescript.visitEachChild(node, visit, context);
        };
        return typescript.visitNode(rootNode, visit);
    };

    const result = typescript.transform(sf, [transformer], {
        target: typescript.ScriptTarget.ES2020,
        jsx: typescript.JsxEmit.Preserve, // transformer handles JSX; printer emits calls
    });

    const printer = typescript.createPrinter({ newLine: typescript.NewLineKind.LineFeed });
    return printer.printFile(result.transformed[0]);
}
