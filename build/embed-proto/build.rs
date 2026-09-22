// 链接 perry staticlib + perry runtime/stdlib + Windows 系统库
// （系统库清单来自 perry-src/crates/perry/src/commands/compile/link/windows_link.rs）
use std::path::PathBuf;

fn main() {
    // 所有路径用绝对路径（linker cwd 是 target/debug/deps，相对路径会错位）
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let build_dir = manifest.join(".."); // E:/AI-workspace/gpui-perryts/build
    let lib_dir = PathBuf::from("E:/AI-workspace/gpui-perryts/ui/node_modules/@perryts/perry-win32-x64/lib");

    // perry-app-static.lib 复制成无连字符名，rustc link-lib 解析更稳
    let src = build_dir.join("perry-app-static.lib");
    let dst = build_dir.join("perry_app_static.lib");
    assert!(src.exists(), "missing {}", src.display());
    let _ = std::fs::copy(&src, &dst);

    println!("cargo:rustc-link-search=native={}", build_dir.display());
    println!("cargo:rustc-link-search=native={}", lib_dir.display());
    println!("cargo:rustc-link-lib=static=perry_app_static");
    println!("cargo:rustc-link-lib=static=perry_runtime");
    println!("cargo:rustc-link-lib=static=perry_stdlib");
    // perry_runtime 自带一份 Rust std（rust_eh_personality 等与宿主 std 重复定义）
    println!("cargo:rustc-link-arg=/FORCE:MULTIPLE");

    for l in [
        "user32", "gdi32", "gdiplus", "msimg32", "kernel32", "shell32", "shlwapi", "ole32",
        "comctl32", "uxtheme", "winspool", "rpcrt4", "advapi32", "comdlg32", "ws2_32", "dwmapi",
        "msvcrt", "vcruntime", "ucrt", "bcrypt", "ntdll", "userenv", "secur32", "oleaut32",
        "propsys", "runtimeobject", "iphlpapi", "winhttp",
    ] {
        println!("cargo:rustc-link-lib={l}");
    }
    println!("cargo:rerun-if-changed={}", src.display());
}
