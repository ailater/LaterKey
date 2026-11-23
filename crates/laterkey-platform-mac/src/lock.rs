//! 锁屏。首选私有 API `SACLockScreenImmediate`（立即锁），不可用时退到系统
//! 快捷键 ⌃⌘Q。
//!
//! 私有 API 在运行期用 dlsym 取：不在链接期依赖私有框架，将来系统移除它
//! 也只是退到兜底方案，而不是起不来。

use std::sync::OnceLock;

use laterkey_core::{flags, Chord};

type LockFn = unsafe extern "C" fn();

/// 取 `SACLockScreenImmediate` 的地址；取不到为 0。进程存活期内只查一次。
fn immediate_addr() -> usize {
    static ADDR: OnceLock<usize> = OnceLock::new();
    *ADDR.get_or_init(|| {
        // SAFETY: dlsym/dlopen 的常规用法；拿到的句柄不关（要一直用）
        unsafe {
            let name = c"SACLockScreenImmediate".as_ptr();
            let mut sym = libc::dlsym(libc::RTLD_DEFAULT, name);
            if sym.is_null() {
                let path = c"/System/Library/PrivateFrameworks/login.framework/login".as_ptr();
                let handle = libc::dlopen(path, libc::RTLD_LAZY);
                if !handle.is_null() {
                    sym = libc::dlsym(handle, name);
                }
            }
            sym as usize
        }
    })
}

/// 首选方案在这台系统上是否可用（只查符号，不锁屏）。
pub fn immediate_available() -> bool {
    immediate_addr() != 0
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LockMethod {
    /// SACLockScreenImmediate
    Immediate,
    /// 系统快捷键 ⌃⌘Q（用户在键盘设置里关掉的话就不生效）
    Hotkey,
}

/// 立即锁屏。
pub fn lock_screen() -> Result<LockMethod, String> {
    let addr = immediate_addr();
    if addr != 0 {
        // SAFETY: 符号来自系统 login.framework，签名为 void(void)
        let lock: LockFn = unsafe { std::mem::transmute::<usize, LockFn>(addr) };
        unsafe { lock() };
        return Ok(LockMethod::Immediate);
    }
    // 兜底：⌃⌘Q（键码 0x0C = Q）
    if super::post::chord(Chord {
        keycode: 0x0C,
        flags: flags::CONTROL | flags::COMMAND,
    }) {
        Ok(LockMethod::Hotkey)
    } else {
        Err("SACLockScreenImmediate 不可用，合成 ⌃⌘Q 也失败".into())
    }
}
