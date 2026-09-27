// Minimal NativeTS example — the "hello world" of the Go-like dev loop.
//
//   nativets dev  examples/counter.tsx
//   nativets build examples/counter.tsx -o counter.exe
//
// Edit the file while `dev` is running: the CLI rebuilds and restarts the
// window automatically.
import { createRoot, h, createSignal } from "nativets";

function Counter() {
    const [n, setN] = createSignal(0);
    const inc = () => setN(n() + 1);
    const reset = () => setN(0);

    return (
        <div
            style={{
                flexDirection: "column",
                gap: 14,
                padding: 28,
                justifyContent: "center",
                alignItems: "center",
                width: 1,
                height: 1,
                background: "#14171f",
            }}
        >
            <div style={{ fontSize: 15, color: "#8b93a7" }}>
                NativeTS · native TypeScript, zero JS runtime to install
            </div>
            <div style={{ fontSize: 52, color: "#4cc2ff", fontWeight: "bold" }}>
                {n()}
            </div>
            <div style={{ flexDirection: "row", gap: 10 }}>
                <div
                    style={{
                        padding: "8px 20px",
                        borderRadius: 8,
                        background: "#2f81f7",
                        fontSize: 14,
                        color: "#ffffff",
                    }}
                    onClick={inc}
                >
                    +1
                </div>
                <div
                    style={{
                        padding: "8px 20px",
                        borderRadius: 8,
                        background: "#30363d",
                        fontSize: 14,
                        color: "#c9d1d9",
                    }}
                    onClick={reset}
                >
                    reset
                </div>
            </div>
        </div>
    );
}

createRoot({ title: "NativeTS Counter" }, Counter);
