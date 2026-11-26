//! LaterKey 的 macOS 平台层：事件拦截、前台应用跟踪、合成按键、锁屏、Dock。
//!
//! 全部是裸 FFI（CoreGraphics / CoreFoundation / ApplicationServices）加一小块
//! objc2（NSWorkspace）。只在 macOS 上有内容，其他平台编译成空 crate。

#[cfg(target_os = "macos")]
mod access;
#[cfg(target_os = "macos")]
mod cf;
#[cfg(target_os = "macos")]
mod dock;
#[cfg(target_os = "macos")]
mod ffi;
#[cfg(target_os = "macos")]
mod focus;
#[cfg(target_os = "macos")]
mod geometry;
#[cfg(target_os = "macos")]
mod lock;
#[cfg(target_os = "macos")]
mod pasteboard;
#[cfg(target_os = "macos")]
mod post;
#[cfg(target_os = "macos")]
mod shortcuts;
#[cfg(target_os = "macos")]
mod tap;
#[cfg(target_os = "macos")]
mod worker;

#[cfg(target_os = "macos")]
pub use access::*;
#[cfg(target_os = "macos")]
pub use dock::Tile;
#[cfg(target_os = "macos")]
pub use focus::{
    activate_pid, app_name_for_bundle, bundle_id_for_pid, dock_pid, frontmost, host_app,
    running_apps, FrontApp, RunningApp,
};
#[cfg(target_os = "macos")]
pub use lock::{immediate_available, lock_screen, LockMethod};
#[cfg(target_os = "macos")]
pub use post::trigger_chord;
#[cfg(target_os = "macos")]
pub use shortcuts::{read_cheatsheet, CheatSheet};
#[cfg(target_os = "macos")]
pub use tap::{decide_key, Effect, EventTap, Job, Outcome, Shared, Stats, TapOptions, Trace};
#[cfg(target_os = "macos")]
pub use worker::{PanelCommand, Worker};

/// 当前 Dock 上的应用图标（诊断用）。
#[cfg(target_os = "macos")]
pub fn dock_tiles() -> Vec<Tile> {
    match focus::dock_pid() {
        Some(pid) => dock::read_tiles(pid, &focus::running_apps()),
        None => Vec::new(),
    }
}
