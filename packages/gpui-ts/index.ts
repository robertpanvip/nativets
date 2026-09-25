/**
 * Public surface of the `gpui-ts` package.
 *
 * Everything an application needs: the reactive runtime plus the JSX factory
 * (`h`, which the CLI compiles `<div/>` onto), the component kit, the Canvas 2D
 * recorder and the palette.
 *
 * These are the *same* sources the bundled demo in `ui/src` runs — the package
 * re-exports them rather than forking, so it cannot drift from what the repo
 * tests and screenshots cover.
 */
export * from "../../ui/src/runtime";
export * from "../../ui/src/kit";
export * from "../../ui/src/canvas2d";
export * from "../../ui/src/theme";
