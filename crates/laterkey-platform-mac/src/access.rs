//! 权限与系统状态探测。除 `request_ax_access` 外都是只读、不弹窗。

use super::cf::Cf;
use super::ffi::*;

/// ListenEvent（监听事件）权限。**不是主动拦截的门槛**——主动 tap 需要的是辅助
/// 功能（AXIsProcessTrusted）。macOS 15+ 把 ListenEvent 拆成独立权限；这里曾被
/// 误用作启动检查：辅助功能已授权、ListenEvent 被拒时，程序误判「没有辅助功能
/// 权限」退出（2026-11-23 真机排查：TCC 里 Accessibility=allowed、ListenEvent=
/// denied，从有双项权限的终端里测试完全掩盖了这个问题）。仅供 doctor 诊断展示。
pub fn listen_allowed() -> bool {
    unsafe { CGPreflightListenEventAccess() }
}

/// 能否发送合成事件
pub fn post_allowed() -> bool {
    unsafe { CGPreflightPostEventAccess() }
}

/// 辅助功能 API 是否可用。**这才是主动事件拦截（CGEventTap 主动模式）、读 Dock
/// 图标、最小化窗口的门槛**。
pub fn ax_trusted() -> bool {
    unsafe { AXIsProcessTrusted() }
}

/// 安全输入是否开启。开启时拦截层收不到键盘事件（密码框、锁屏界面），
/// 这是系统有意为之，映射会随之暂停。
pub fn secure_input_enabled() -> bool {
    unsafe { IsSecureEventInputEnabled() }
}

/// 弹一次系统的辅助功能授权请求（把本应用带进系统设置的授权列表）。
///
/// 用 AX 的 prompt（AXIsProcessTrustedWithOptions），不是 CGRequestListenEventAccess
/// ——后者请求的是 ListenEvent，会在 TCC 里留下另一项权限的拒绝记录。
pub fn request_ax_access() -> bool {
    // SAFETY: kAXTrustedCheckOptionPrompt 与 kCFBooleanTrue 都是常量 CF 对象；
    // 字典临时创建，用完由 Cf 释放
    unsafe {
        let keys = [kAXTrustedCheckOptionPrompt];
        let values = [kCFBooleanTrue];
        let options = Cf::owned(CFDictionaryCreate(
            std::ptr::null(),
            keys.as_ptr(),
            values.as_ptr(),
            1,
            &kCFTypeDictionaryKeyCallBacks,
            &kCFTypeDictionaryValueCallBacks,
        ) as *mut std::ffi::c_void);
        AXIsProcessTrustedWithOptions(options.as_cf())
    }
}
