# Vendored: scriptc C runtime (MSVC build)

Source: <https://github.com/robertpanvip/scriptc> at merge commit `8b13e79`
("Merge vercel-labs/scriptc main (858f000) into MSVC embedding fork"),
path `vendor/scriptc-runtime/`. License: Apache-2.0 (see `LICENSE`).

Upstream is <https://github.com/vercel-labs/scriptc> (Apache-2.0).

## What this is

The C execution runtime of the scriptc (TS→native AOT) compiler, plus the
MSVC compatibility shims that let it build with `cl.exe`/UCRT instead of
zig-cc/MinGW. Compiled into the host binary by `host/build.rs` (via the `cc`
crate) so the scriptc backend's runtime shares the Rust binary's UCRT
instance — the same embedding shape as the QuickJS backend.

## Layout

| Path | Contents |
|---|---|
| `src/` | the 63 runtime C units (only the ones the graph reaches are compiled) |
| `shim/` | MSVC shims: `unistd.h`, `dirent.h`+`dirent-shim.c`, `scr_msvc_compat.h`, `scr_msvc_builtins.c`, `scr_msvc_clock.c` |
| `vendor/ryu/` | `d2s.c` — pull in by `scr_number.c` as `../vendor/ryu/d2s.c` (keep this relative layout) |
| `vendor-quickjs/` | `libregexp.c` / `libunicode.c` / `dtoa.c` — regex support (only compiled if the graph reaches regex) |
| `vendor-zlib/` | zlib core (only compiled if the graph reaches zlib) |

## Deliberately NOT vendored

- `vendor-mbedtls/` (9 MB) — TLS only; library mode is async-free and our
  graph does not reach `scr_tls.c`. Re-vendor it if TLS is ever needed.

## Compiled source set

`host/build.rs` mirrors the exact member list scriptc's library-mode archive
produces (verified with `zig ar t out.lib.a`): 18 units + `scr_win.c`.

## Maintenance

Per the fork's feasibility report (`msvc-probe/REPORT.md`), the upkeep model
is "re-vendor the runtime + re-run the shims" — the shim surface is only a
handful of files, so the diff against upstream stays small.

## Re-vendoring

```bash
git clone --depth 1 https://github.com/robertpanvip/scriptc /tmp/scriptc
S=/tmp/scriptc/vendor/scriptc-runtime
rm -rf vendor/scriptc-runtime/{src,shim,vendor,vendor-quickjs,vendor-zlib}
mkdir -p vendor/scriptc-runtime/vendor
cp -r $S/src vendor/scriptc-runtime/
cp -r $S/shim vendor/scriptc-runtime/
cp -r $S/vendor/ryu vendor/scriptc-runtime/vendor/
cp -r $S/vendor-quickjs $S/vendor-zlib vendor/scriptc-runtime/
cp $S/LICENSE vendor/scriptc-runtime/
```
