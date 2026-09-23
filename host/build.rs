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

    // scriptc DLL backend: cfg enabled purely by the DLL's presence at build
    // time (runtime resolution re-checks both locations and can also be forced
    // with `host.exe scriptc`). LoadLibrary means nothing is linked here.
    println!("cargo:rerun-if-changed=../build/scriptc_fe.dll");
    if std::env::var("GPUI_TS_NO_SCRIPTC").is_err() && std::path::Path::new("../build/scriptc_fe.dll").exists() {
        println!("cargo:rustc-cfg=scriptc");
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
