// 嵌入模式开关：检测 perry staticlib → 链接 + 打开 cfg(embedded)。
// runtime/stdlib 路径从 linkdeps.json 解析（带 PERRY_WORKSPACE_ROOT 编译时
// 指向 auto-optimize 产物，体积小 66%；缺失回落 npm 包的全量库）。
// 缺 staticlib 或设 PERRY_NO_EMBED → 回落双进程（子进程 stdio）模式。
use std::path::PathBuf;

fn main() {
    // declare the conditional cfgs we emit so the linter stays quiet
    println!("cargo:rustc-check-cfg=cfg(quickjs)");
    println!("cargo:rustc-check-cfg=cfg(embedded)");
    println!("cargo:rustc-check-cfg=cfg(has_embedded_frontend)");
    println!("cargo:rustc-check-cfg=cfg(scriptc)");

    // These env vars must be tracked UNCONDITIONALLY (before the early return
    // below) so cargo always re-runs this script when they flip — otherwise it
    // caches the Perry-mode cfg and ignores QUICKJS_EMBED on later builds.
    println!("cargo:rerun-if-env-changed=QUICKJS_EMBED");
    println!("cargo:rerun-if-env-changed=QUICKJS_EMBED_BUNDLE");

    // quickjs.rs embeds this file with include_str!; without the declaration
    // cargo would not rebuild when the bootstrap JS changes.
    println!("cargo:rerun-if-changed=src/bootstrap.js");

    // ── scriptc backend ───────────────────────────────────────────────────
    // Two shapes, chosen by what ui/build-scriptc.mjs left in build/:
    //   * STATIC (preferred): build/scriptc/sc-main.lib.c — the program TU
    //     scriptc emits with profile `emission:"c"`. It is compiled together
    //     with the vendored MSVC runtime (vendor/scriptc-runtime) straight into
    //     THIS binary via the cc crate, so the scriptc backend shares rustc's
    //     UCRT instance — the same embedding shape as the QuickJS backend, and
    //     a single self-contained exe (no DLL).
    //   * DLL (legacy fallback): build/scriptc_fe.dll — a mingw-CRT shared
    //     library resolved at runtime with LoadLibraryExA.
    println!("cargo:rustc-check-cfg=cfg(scriptc_static)");
    println!("cargo:rustc-check-cfg=cfg(scriptc_dll)");
    println!("cargo:rerun-if-env-changed=GPUI_TS_NO_SCRIPTC");
    println!("cargo:rerun-if-changed=../build/scriptc");
    println!("cargo:rerun-if-changed=../build/scriptc_fe.dll");
    if std::env::var("GPUI_TS_NO_SCRIPTC").is_err() {
        if std::path::Path::new("../build/scriptc/sc-main.lib.c").exists() {
            build_scriptc_static();
            println!("cargo:rustc-cfg=scriptc");
            println!("cargo:rustc-cfg=scriptc_static");
        } else if std::path::Path::new("../build/scriptc_fe.dll").exists() {
            println!("cargo:rustc-cfg=scriptc");
            println!("cargo:rustc-cfg=scriptc_dll");
        }
    }

    // QuickJS-embedded mode: link the rquickjs engine instead of Perry's
    // staticlib. No perry archives needed. Set QUICKJS_EMBED=1 to force it.
    if std::env::var("QUICKJS_EMBED").is_ok() {
        println!("cargo:rustc-cfg=quickjs");
        println!("cargo:rustc-cfg=has_embedded_frontend");
        println!("cargo:rerun-if-env-changed=QUICKJS_EMBED");
        // The TS→JS bundle is compiled in via `include_str!` in quickjs.rs, so
        // no extra env wiring is needed here for a self-contained exe.
        return;
    }

    let build_dir = PathBuf::from("../build");

    // ── 选哪个 app archive ────────────────────────────────────────────────
    // 这里以前写死 "perry-app-static.lib"，而 `scripts/gpui-ts.mjs --backend
    // perry` 产出的是 `app-main.lib`（名字由入口文件推出来）。于是 CLI 每轮
    // 新编的 archive 被完全忽略，链接的始终是上一次遗留的那份——而且
    // /FORCE:MULTIPLE 会把它变成**无声**的：exe 正常生成、应用正常运行，
    // 只是静止在旧版本源码上。见 docs/gpui-ts-plan.md 坑 #7。
    //
    // 解析顺序：env 明确指定 → 否则取两份候选里**较新**的一份。
    let lib_base = std::env::var("PERRY_STATIC_LIB_BASE")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| {
            let cands = ["app-main", "perry-app-static"];
            let newest = cands
                .iter()
                .filter_map(|b| {
                    let p = build_dir.join(format!("{b}.lib"));
                    let t = std::fs::metadata(&p).ok()?.modified().ok()?;
                    Some((t, *b))
                })
                .max();
            match newest {
                Some((_, b)) => b.to_string(),
                None => cands[1].to_string(), // 都不存在时按旧名走，好让下面的 ok 判定回落
            }
        });
    let app_src = build_dir.join(format!("{lib_base}.lib"));
    let manifest = build_dir.join(format!("{lib_base}.linkdeps.json"));
    let fallback_lib_dir =
        PathBuf::from("../ui/node_modules/@perryts/perry-win32-x64/lib");

    let ok = app_src.exists()
        && fallback_lib_dir.join("perry_runtime.lib").exists()
        && std::env::var("PERRY_NO_EMBED").is_err();

    if ok {
        // 复制成无连字符名（rustc 的 link-lib 解析更稳）。这一步失败不能吞：
        // 吞掉就会退化成「链接上一轮的旧副本」，正是上面那条无声故障。
        let link_archive = build_dir.join("perry_app_static.lib");
        if let Err(e) = std::fs::copy(&app_src, &link_archive) {
            println!(
                "cargo:warning=复制 app archive 失败（{} → {}）: {e}",
                app_src.display(),
                link_archive.display()
            );
        }
        // 没显式指定 base 时（手工 cargo build）把选择结果说出来，不然只能猜
        if std::env::var("PERRY_STATIC_LIB_BASE").is_err() {
            let size = std::fs::metadata(&app_src).map(|m| m.len()).unwrap_or(0);
            println!(
                "cargo:warning=perry app archive: {lib_base}.lib（{size} B）— 用 PERRY_STATIC_LIB_BASE 可覆盖"
            );
        }
        println!("cargo:rustc-link-search=native={}", build_dir.display());

        // 从 linkdeps.json 取 runtime/stdlib 路径（role 字段）
        let mut runtime_lib: Option<PathBuf> = None;
        let mut stdlib_lib: Option<PathBuf> = None;
        if let Ok(txt) = std::fs::read_to_string(&manifest) {
            if let Some(v) = serde_json_lite(&txt) {
                for entry in v {
                    let role = entry.get("role").map(|s| s.as_str()).unwrap_or("");
                    let raw = entry.get("path").map(|s| s.as_str()).unwrap_or("");
                    // strip \\?\ verbatim prefix
                    let p = raw.trim_start_matches(r"\\?\");
                    match role {
                        "runtime" => runtime_lib = Some(PathBuf::from(p)),
                        "stdlib" => stdlib_lib = Some(PathBuf::from(p)),
                        _ => {}
                    }
                }
            }
        }
        let runtime_lib =
            runtime_lib.unwrap_or_else(|| fallback_lib_dir.join("perry_runtime.lib"));
        let stdlib_lib =
            stdlib_lib.unwrap_or_else(|| fallback_lib_dir.join("perry_stdlib.lib"));
        if !runtime_lib.exists() || !stdlib_lib.exists() {
            println!(
                "cargo:warning=perry runtime/stdlib archives missing ({} / {}) — falling back to child-process mode",
                runtime_lib.display(),
                stdlib_lib.display()
            );
            println!("cargo:rustc-cfg=has_embedded_frontend");
            return;
        }
        for p in [&runtime_lib, &stdlib_lib] {
            let dir = p.parent().unwrap().to_path_buf();
            println!("cargo:rustc-link-search=native={}", dir.display());
        }
        // 显式传 .lib 全路径给 linker（避免同名冲突）
        println!("cargo:rustc-link-lib=static=perry_app_static");
        println!(
            "cargo:rustc-link-arg={}",
            runtime_lib.display().to_string().replace('\\', "/")
        );
        println!(
            "cargo:rustc-link-arg={}",
            stdlib_lib.display().to_string().replace('\\', "/")
        );

        // 系统库清单与 perry 自身 windows 链接一致
        // （perry-src crates/perry/src/commands/compile/link/windows_link.rs）
        for l in [
            "user32", "gdi32", "gdiplus", "msimg32", "kernel32", "shell32", "shlwapi", "ole32",
            "comctl32", "uxtheme", "winspool", "rpcrt4", "advapi32", "comdlg32", "ws2_32",
            "dwmapi", "msvcrt", "vcruntime", "ucrt", "bcrypt", "ntdll", "userenv", "secur32",
            "oleaut32", "propsys", "runtimeobject", "iphlpapi", "winhttp",
        ] {
            println!("cargo:rustc-link-lib={l}");
        }
        // perry_runtime 内嵌一份 Rust std，与宿主 std 在 rust_eh_personality 等
        // 符号上重复定义（功能等价），/FORCE:MULTIPLE 取第一份
        println!("cargo:rustc-link-arg=/FORCE:MULTIPLE");
        println!("cargo:rustc-cfg=embedded");
        println!("cargo:rustc-cfg=has_embedded_frontend");
    } else {
        println!("cargo:rustc-cfg=has_embedded_frontend"); // 代码始终编译，运行时回落
    }

    println!("cargo:rerun-if-changed={}", app_src.display());
    println!("cargo:rerun-if-changed={}", manifest.display());
    println!("cargo:rerun-if-env-changed=PERRY_NO_EMBED");
    println!("cargo:rerun-if-env-changed=PERRY_STATIC_LIB_BASE");
}

/// 轻量解析 link_archives 数组（避免给 build.rs 引入 serde 依赖）：
/// 返回 [{"role": "...", "path": "..."}, ...]
fn serde_json_lite(txt: &str) -> Option<Vec<std::collections::HashMap<String, String>>> {
    let start = txt.find("\"link_archives\"")?;
    let arr_start = txt[start..].find('[')? + start;
    let arr_end = txt[arr_start..].find(']')? + arr_start;
    let arr = &txt[arr_start + 1..arr_end];
    let mut out = Vec::new();
    for obj in arr.split('{').skip(1) {
        let body = match obj.find('}') {
            Some(i) => &obj[..i],
            None => continue,
        };
        let mut map = std::collections::HashMap::new();
        for kv in body.split(',') {
            let mut it = kv.splitn(2, ':');
            let k = it.next()?.trim().trim_matches('"').to_string();
            // JSON unescape: \\ -> \  then strip the \\?\ verbatim prefix
            let v = it
                .next()?
                .trim()
                .trim_matches('"')
                .replace("\\\\", "\\")
                .trim_start_matches(r"\\?\")
                .to_string();
            if !k.is_empty() {
                map.insert(k, v);
            }
        }
        out.push(map);
    }
    Some(out)
}

/// Compile the vendored scriptc runtime + the generated program TU into this
/// binary with the MSVC toolchain (`cc` crate → `cl.exe`), so the scriptc
/// backend shares rustc's UCRT instance instead of crossing a mingw-CRT DLL
/// boundary.
///
/// The runtime file list is exactly the set scriptc's library-mode archive
/// contains — verified with `zig ar t entry.lib.a` on a real app graph. Library
/// mode is async-free by construction, so `scr_async` / `scr_crypto_async` /
/// `scr_child` / `scr_ffi` are absent, as are the zlib/regex/TLS gated units.
/// Keep this list in sync if a frontend graph ever reaches a new capability.
fn build_scriptc_static() {
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let root = manifest.join("../vendor/scriptc-runtime");
    let src = root.join("src");
    let shim = root.join("shim");
    let program_c = manifest.join("../build/scriptc/sc-main.lib.c");

    // cc-rs normally injects the MSVC INCLUDE/LIB/PATH itself, but when the
    // build starts from a bare shell (no vcvars) the Windows SDK include dirs
    // can be missing, and the vendored shim needs UCRT headers (direct.h …).
    // Apply the discovered toolchain env explicitly so the build is
    // independent of the caller's shell.
    let msvc_env = msvc_tool_env();
    // cc-rs's `.env()` does not reliably reach cl.exe here, so also hand the
    // toolchain's INCLUDE dirs to the compiler as explicit include flags
    // (parsed from the very same env value).
    let sys_includes: Vec<PathBuf> = msvc_env
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("INCLUDE"))
        .map(|(_, v)| {
            v.to_string_lossy()
                .split(';')
                .filter(|s| !s.trim().is_empty())
                .map(|s| PathBuf::from(s.trim()))
                .collect()
        })
        .unwrap_or_default();

    // The library-mode runtime: the executable lane's base set minus the
    // fiber/loop + child + outbound-FFI units, plus scr_library.c (sink, arena,
    // reset registry, the host-pull funnel) and the Win32 arm.
    let runtime = [
        "scr_number.c",
        "scr_string.c",
        "scr_array.c",
        "scr_bytes.c",
        "scr_bytes_io.c",
        "scr_map.c",
        "scr_closure.c",
        "scr_object.c",
        "scr_union.c",
        "scr_exception.c",
        "scr_error.c",
        "scr_console.c",
        "scr_lib.c",
        "scr_path.c",
        "scr_url.c",
        "scr_json.c",
        "scr_cycle.c",
        "scr_library.c",
        "scr_win.c",
    ];

    let mut build = cc::Build::new();
    for (k, v) in &msvc_env {
        build.env(k, v);
    }
    for d in &sys_includes {
        build.include(d);
    }
    for f in runtime {
        build.file(src.join(f));
    }
    // MSVC shims the vendored runtime needs: unistd/dirent/clock + the
    // GCC/Clang builtin shims (popcount/clz/ctz) scr_string.c et al. call.
    for f in ["scr_msvc_builtins.c", "scr_msvc_clock.c", "dirent-shim.c"] {
        build.file(shim.join(f));
    }
    build
        .include(&src)
        .include(&shim)
        .include(root.join("vendor/ryu")) // scr_number.c pulls ../vendor/ryu/d2s.c
        .std("c17")
        .define("SCR_LIB", None) // library flavor discipline (scr_runtime.h)
        .define("_CRT_SECURE_NO_WARNINGS", None)
        .define("NOMINMAX", None)
        .define("WIN32_LEAN_AND_MEAN", None)
        .define("_WIN32_WINNT", "0x0A00")
        .define("QUICKJS_NG_BUILD", None)
        .define("_GNU_SOURCE", None)
        .flag("/utf-8") // generated C carries UTF-8 string literals
        .warnings(false)
        .opt_level(2)
        .compile("scriptc_runtime");

    // The generated program TU — defines gpts_init / gpts_tick / gpts_poll /
    // gpts_event / gpts_set_panic_sink / gpts_reset. No main() to rename: the
    // library ABI's entry IS gpts_init.
    let mut prog = cc::Build::new();
    for (k, v) in &msvc_env {
        prog.env(k, v);
    }
    for d in &sys_includes {
        prog.include(d);
    }
    prog.file(&program_c)
        .include(&src)
        .include(&shim)
        .std("c17")
        .define("SCR_LIB", None)
        .define("_CRT_SECURE_NO_WARNINGS", None)
        .define("NOMINMAX", None)
        .define("WIN32_LEAN_AND_MEAN", None)
        .define("_WIN32_WINNT", "0x0A00")
        .flag("/utf-8")
        .warnings(false)
        .opt_level(2)
        .compile("scriptc_program");

    for l in ["advapi32", "iphlpapi", "ws2_32", "bcrypt", "userenv"] {
        println!("cargo:rustc-link-lib={l}");
    }
    println!("cargo:rerun-if-changed={}", src.display());
    println!("cargo:rerun-if-changed={}", shim.display());
    println!("cargo:rerun-if-changed={}", program_c.display());
}

/// The MSVC toolchain's INCLUDE/LIB/PATH, discovered through the same registry
/// lookup cc-rs performs for cl.exe. Returned as owned pairs so they can be
/// applied to a `cc::Build` via `.env()`. Empty off-Windows / when no VS is
/// found (then cc-rs's own detection is all we have).
fn msvc_tool_env() -> Vec<(std::ffi::OsString, std::ffi::OsString)> {
    #[cfg(windows)]
    {
        let target =
            std::env::var("TARGET").unwrap_or_else(|_| "x86_64-pc-windows-msvc".to_string());
        if let Some(tool) = cc::windows_registry::find_tool(&target, "cl.exe") {
            return tool
                .env()
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
        }
    }
    Vec::new()
}
