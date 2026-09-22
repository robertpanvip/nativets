/**
 * Ambient declarations for editor / `tsc` only — nothing here reaches the
 * bundle (esbuild erases types, and the host provides these globals at runtime).
 *
 * 1. `process` — the host's process shim (QuickJS has no such global; the
 *    Perry/node runtimes do). Declared by hand because this project deliberately
 *    has no `@types/node` dependency.
 * 2. The global `JSX` namespace used by the classic transform
 *    (`jsxFactory: h`, `jsxFragmentFactory: Fragment` in tsconfig.json).
 */

interface HostProcess {
    env: { [key: string]: string | undefined };
    argv: string[];
    stdout: { write(s: string): void };
    stderr: { write(s: string): void };
    stdin: {
        setEncoding(encoding: string): void;
        on(event: string, handler: (chunk: any) => void): void;
    };
    exit(code?: number): void;
}

declare const process: HostProcess;

/** Emitted by the host in `direct` transport mode (QuickJS in-process). */
declare function __hostEmit(line: string): void;

declare namespace JSX {
    /** A JSX expression evaluates to a host element handle. */
    type Element = import("./runtime").El;

    type Style = import("./runtime").Style;

    /**
     * The only props an intrinsic (lowercase) tag understands. Styling goes in
     * `style` — because this interface has no index signature, writing a style
     * key at the top level (e.g. `<div padding={16}>`) is a type error rather
     * than a silently ignored attribute.
     */
    interface IntrinsicProps {
        style?: Style;
        onClick?: () => void;
        /** Initial text content; same as passing a string child. */
        text?: string;
        children?: any;
    }

    interface IntrinsicElements {
        [tag: string]: IntrinsicProps;
    }

    /** Tells the compiler which prop carries JSX children. */
    interface ElementChildrenAttribute {
        children: {};
    }
}
