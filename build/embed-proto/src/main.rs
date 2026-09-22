// 嵌入原型：把 perry staticlib（TS 前端）链进一个最小 Rust host。
// 验证三件事：
//   1) perry-app-static.lib + perry_runtime.lib + perry_stdlib.lib 能链接成功
//   2) SetStdHandle 重定向后 perry_module_init() 的 console.log(JSONL ops) 落到 host 的 pipe
//   3) host 用 perry_poll() / perry_has_work() 能驱动前端事件循环（喂 event 收 ops）
use std::fs::File;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::windows::io::{FromRawHandle, RawHandle};
use std::thread;
use std::time::Duration;

// perry staticlib 导出的 C ABI（--output-type staticlib / #1088）
extern "C" {
    fn perry_module_init();
    fn perry_poll() -> i32;
    fn perry_has_work() -> i32;
    fn perry_next_wake_ms() -> f64;
}

// 最小 Win32 声明（不引 windows-sys，避免版本 API 迁移噪音）
type Handle = *mut core::ffi::c_void;
const STD_INPUT_HANDLE: u32 = 0xFFFF_FFF6;
const STD_OUTPUT_HANDLE: u32 = 0xFFFF_FFF5;
const STD_ERROR_HANDLE: u32 = 0xFFFF_FFF4;
const INVALID_HANDLE_VALUE: Handle = -1isize as Handle;

extern "system" {
    fn CreatePipe(r: *mut Handle, w: *mut Handle, attrs: *const u32, size: u32) -> i32;
    fn GetStdHandle(which: u32) -> Handle;
    fn SetStdHandle(which: u32, h: Handle) -> i32;
}

fn make_pipe() -> (RawHandle, RawHandle) {
    let mut r: Handle = std::ptr::null_mut();
    let mut w: Handle = std::ptr::null_mut();
    // 默认 4KB 缓冲、阻塞模式
    let ok = unsafe { CreatePipe(&mut r, &mut w, std::ptr::null(), 0) };
    assert!(
        ok != 0 && r != INVALID_HANDLE_VALUE && w != INVALID_HANDLE_VALUE,
        "CreatePipe failed"
    );
    (r as RawHandle, w as RawHandle)
}

fn now_ms() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis()
}

fn main() {
    // 保留原始控制台 stderr，用于 host 自身日志（重定向不影响它）
    let orig_err = unsafe { GetStdHandle(STD_ERROR_HANDLE) };

    // ---- 1. 建管道并在 std 首次使用前重定向 std handles ----
    let (out_r, out_w) = make_pipe(); // perry 写 → host 读
    let (in_r, in_w) = make_pipe(); // host 写 → perry 读
    let (err_r, err_w) = make_pipe(); // 前端 stderr 也收掉
    unsafe {
        assert!(SetStdHandle(STD_OUTPUT_HANDLE, out_w) != 0);
        assert!(SetStdHandle(STD_ERROR_HANDLE, err_w) != 0);
        assert!(SetStdHandle(STD_INPUT_HANDLE, in_r) != 0);
    }

    let mut out_reader = BufReader::new(unsafe { File::from_raw_handle(out_r) });
    let mut in_writer = unsafe { File::from_raw_handle(in_w) };
    let err_w_file = unsafe { File::from_raw_handle(err_w) };
    drop(err_w_file); // host 侧不需要写前端 stderr
    let err_r_file = unsafe { File::from_raw_handle(err_r) }; // File 是 Send，先包再进线程

    // stderr 丢弃线程（排空防阻塞）
    thread::spawn(move || {
        let mut f = err_r_file;
        let mut buf = [0u8; 4096];
        loop {
            if f.read(&mut buf).unwrap_or(0) == 0 {
                break;
            }
        }
    });

    // ops 输出线程：逐行打印 perry 前端发来的 JSONL。
    // 注意：必须写原始 stderr（print!/std stdout 已被重定向到 out_w，
    // 用它会自环死锁）。
    // File 是 Send：先在主线程包好再 move 进线程（裸 handle 不 Send）
    let printer_out = unsafe { File::from_raw_handle(orig_err) };
    let printer = thread::spawn(move || {
        let mut out = printer_out;
        let mut line = String::new();
        loop {
            line.clear();
            match out_reader.read_line(&mut line) {
                Ok(0) => break,
                Ok(_) => {
                    let s = format!("[{}] {}\n", now_ms(), line.trim_end());
                    let _ = out.write_all(s.as_bytes());
                    let _ = out.flush();
                }
                Err(_) => break,
            }
        }
    });

    // host 自身日志走原始 stderr（规避重定向）
    let log = move |msg: &str| {
        let mut f = unsafe { File::from_raw_handle(orig_err) };
        let _ = f.write_all(msg.as_bytes());
        let _ = f.flush();
        std::mem::forget(f); // 原始 handle 不能被 close
    };

    // ---- 2. 初始化 perry 模块（跑 TS 顶层语句 → init ops 输出到 pipe）----
    // 诊断猜测：perry 的 stdin 子系统可能在 init 期间同步读一次 stdin，
    // 空 pipe 会永久阻塞 → 先预写一行合法 event 保证有数据可读。
    let pre = format!(
        "{{\"t\":\"event\",\"target\":9,\"kind\":\"click\",\"ts\":{}}}\n",
        now_ms()
    );
    let _ = in_writer.write_all(pre.as_bytes());
    let _ = in_writer.flush();
    log(&format!("== pre-wrote stdin probe: {}", pre.trim_end()));
    log("== calling perry_module_init()\n");
    let t0 = std::time::Instant::now();
    // 诊断：init 放子线程 + 超时打点（perry #8546 注释确认 module_init 可在自己线程跑）
    let init = thread::spawn(|| unsafe { perry_module_init() });
    for i in 0..30 {
        if init.is_finished() {
            break;
        }
        thread::sleep(Duration::from_millis(100));
        if i == 9 {
            log("== ...perry_module_init still running after 1s\n");
        }
        if i == 29 {
            log("== ...perry_module_init STILL RUNNING after 3s — blocked!\n");
        }
    }
    let _ = init.join();
    log(&format!("== perry_module_init() done in {:?}\n", t0.elapsed()));

    // ---- 3. host 驱动事件循环：写 event → perry_poll() ----
    let mut tick = 0;
    let start = std::time::Instant::now();
    while start.elapsed() < Duration::from_millis(3000) {
        tick += 1;
        // 模拟 gpui host 发点击事件（协议与 host.exe 相同）
        if tick % 10 == 1 && tick <= 31 {
            let ev = format!(
                "{{\"t\":\"event\",\"target\":9,\"kind\":\"click\",\"ts\":{}}}\n",
                now_ms()
            );
            let _ = in_writer.write_all(ev.as_bytes());
            let _ = in_writer.flush();
            log(&format!("== host -> perry: {}", ev.trim_end()));
        }
        let n = unsafe { perry_poll() };
        let work = unsafe { perry_has_work() };
        let wake = unsafe { perry_next_wake_ms() };
        if tick % 10 == 0 {
            log(&format!(
                "== poll#{tick} microtasks={n} has_work={work} next_wake_ms={wake:.1}\n"
            ));
        }
        thread::sleep(Duration::from_millis(10));
    }

    // 收尾：关闭 stdin 写端（前端会看到 end），等待输出线程排空
    drop(in_writer);
    thread::sleep(Duration::from_millis(300));
    log("== proto done\n");
    let _ = printer.join();
}
