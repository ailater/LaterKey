//! 权限与系统状态探测。除 `request_listen_access` 外都是只读、不弹窗。

use super::ffi::*;

/// 能否安装主动事件拦截（等同于辅助功能权限）
pub fn listen_allowed() -> bool {
    unsafe { CGPreflightListenEventAccess() }
}

/// 能否发送合成事件
pub fn post_allowed() -> bool {
    unsafe { CGPreflightPostEventAccess() }
}

/// 辅助功能 API（读 Dock 图标、最小化窗口）是否可用
pub fn ax_trusted() -> bool {
    unsafe { AXIsProcessTrusted() }
}

/// 安全输入是否开启。开启时拦截层收不到键盘事件（密码框、锁屏界面），
/// 这是系统有意为之，映射会随之暂停。
pub fn secure_input_enabled() -> bool {
    unsafe { IsSecureEventInputEnabled() }
}

/// 弹一次系统的辅助功能授权请求。返回当前是否已有权限。
pub fn request_listen_access() -> bool {
    unsafe { CGRequestListenEventAccess() }
}
