//! `laterkey` 命令行入口。
//!
//! ```text
//! laterkey run       启动重映射（原型阶段默认 5 分钟后自动退出）
//! laterkey doctor    只读诊断：权限、前台应用、Dock 图标、锁屏通路
//! laterkey lock      立即锁屏
//! ```

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("laterkey 只支持 macOS");
    std::process::exit(1);
}

#[cfg(target_os = "macos")]
mod app;
#[cfg(target_os = "macos")]
mod daemon;
#[cfg(target_os = "macos")]
mod duration;
#[cfg(target_os = "macos")]
mod instance;

#[cfg(target_os = "macos")]
fn main() {
    if let Err(err) = app::run() {
        eprintln!("[laterkey] {err}");
        std::process::exit(1);
    }
}
