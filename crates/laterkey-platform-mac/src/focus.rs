//! 运行中应用的查询（AppKit 的 NSWorkspace）与本进程的宿主应用探测。
//!
//! 这里的函数只在后台线程调用，不在拦截回调里。

use std::ffi::c_void;
use std::os::raw::c_char;
use std::path::{Path, PathBuf};

use objc2::rc::autoreleasepool;
use objc2_app_kit::{NSApplicationActivationPolicy, NSRunningApplication, NSWorkspace};
use objc2_foundation::NSString;

use super::cf::{self, Cf};
use super::ffi::*;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FrontApp {
    pub pid: i32,
    pub bundle_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunningApp {
    pub pid: i32,
    pub bundle_id: String,
    /// bundle 的规范化路径（解析过符号链接），没有 bundle 的进程不会出现在这里
    pub path: String,
    /// 常规应用（有 Dock 图标、能当前台）。辅助进程是 Accessory/Prohibited。
    pub regular: bool,
}

/// 前台应用的 pid：窗口服务器里最前面的普通窗口（layer 0）的属主。
///
/// 不用 NSWorkspace::frontmostApplication：它的值靠激活通知维护，而通知由
/// 第一次创建 NSWorkspace 的线程的 RunLoop 派发；没有 RunLoop 的轮询线程上
/// 读数会永远停在第一次查询。真机已复现：后台线程里激活访达 3 秒后，它仍
/// 返回激活前的应用。CGWindowList 是对窗口服务器的无状态查询，每次都是实时值。
pub fn frontmost_pid() -> Option<i32> {
    // SAFETY: 只读查询；返回的数组带一个引用，交给 Cf 释放
    let list =
        unsafe { Cf::owned(CGWindowListCopyWindowInfo(LIST_ON_SCREEN_ONLY, 0) as *mut c_void) };
    if list.is_null() {
        return None;
    }
    let list = list.as_cf();
    // SAFETY: list 是有效的 CFArray，存活到函数结束
    unsafe {
        for i in 0..CFArrayGetCount(list) {
            let dict = CFArrayGetValueAtIndex(list, i);
            if dict.is_null() || !cf::is_dictionary(dict) {
                continue;
            }
            if let (Some(layer), Some(owner)) = (
                cf::dict_get(dict, kCGWindowLayer),
                cf::dict_get(dict, kCGWindowOwnerPID),
            ) {
                if cf::number_to_i64(layer) == Some(0) {
                    return cf::number_to_i64(owner).map(|pid| pid as i32);
                }
            }
        }
    }
    None
}

pub fn frontmost() -> Option<FrontApp> {
    let pid = frontmost_pid()?;
    autoreleasepool(|_pool| {
        // 按 pid 直查，不依赖任何激活状态缓存
        let app = NSRunningApplication::runningApplicationWithProcessIdentifier(pid)?;
        Some(FrontApp {
            pid,
            bundle_id: app
                .bundleIdentifier()
                .map(|s| s.to_string())
                .unwrap_or_default(),
        })
    })
}

pub fn dock_pid() -> Option<i32> {
    autoreleasepool(|_pool| {
        NSRunningApplication::runningApplicationsWithBundleIdentifier(&NSString::from_str(
            "com.apple.dock",
        ))
        .firstObject()
        .map(|app| app.processIdentifier())
    })
}

pub fn running_apps() -> Vec<RunningApp> {
    autoreleasepool(|_pool| {
        NSWorkspace::sharedWorkspace()
            .runningApplications()
            .iter()
            .filter_map(|app| {
                let path = app.bundleURL()?.path()?.to_string();
                Some(RunningApp {
                    pid: app.processIdentifier(),
                    bundle_id: app
                        .bundleIdentifier()
                        .map(|s| s.to_string())
                        .unwrap_or_default(),
                    path: canonical(&path),
                    regular: app.activationPolicy() == NSApplicationActivationPolicy::Regular,
                })
            })
            .collect()
    })
}

/// 规范化路径：解析符号链接（/Applications → /System/Applications 这类），
/// 去掉结尾的斜杠。解析不了就原样返回。
///
/// Dock 图标给的路径和应用自己报的路径可能写法不同，比较之前两边都过一遍。
pub fn canonical(path: &str) -> String {
    let trimmed = path.trim_end_matches('/');
    match std::fs::canonicalize(trimmed) {
        Ok(p) => p.to_string_lossy().trim_end_matches('/').to_string(),
        Err(_) => trimmed.to_string(),
    }
}

/// 启动本进程的宿主应用：沿父进程链往上，第一个位于 .app 包里的进程。
///
/// 从终端（包括 ZCode、VS Code 这类内嵌终端）里运行时，返回的就是那个应用：
/// 用户的 Ctrl+C 是按给它的。没有宿主（从 launchd 起的）返回 None。
pub fn host_app() -> Option<RunningApp> {
    let apps = running_apps();
    let mut pid = std::process::id() as i32;
    for _ in 0..32 {
        if let Some(bundle) =
            executable_path(pid).and_then(|exe| enclosing_app(&exe).map(Path::to_path_buf))
        {
            let bundle = canonical(&bundle.to_string_lossy());
            let matches: Vec<&RunningApp> = apps.iter().filter(|a| a.path == bundle).collect();
            // 同一个包路径下可能有辅助进程的条目，要的是那个常规应用
            return matches
                .iter()
                .find(|a| a.regular)
                .or_else(|| matches.first())
                .map(|a| (*a).clone());
        }
        pid = ppid(pid)?;
        if pid <= 1 {
            break;
        }
    }
    None
}

/// 可执行文件所属的 .app 包，取**最外层**那个。
///
/// 不能取最近的：ZCode、VS Code 这类应用的命令行工具跑在嵌套的辅助包里
/// （`ZCode.app/Contents/Frameworks/ZCode Helper.app/...`），最近的那个是辅助包，
/// 不是用户按 Ctrl+C 时前台的那个应用。
fn enclosing_app(exe: &Path) -> Option<&Path> {
    exe.ancestors()
        .filter(|p| p.extension().is_some_and(|e| e == "app"))
        .last()
}

fn executable_path(pid: i32) -> Option<PathBuf> {
    let mut buf = vec![0 as c_char; 4 * 1024];
    // SAFETY: 缓冲区大小与传入的一致
    let len = unsafe { libc::proc_pidpath(pid, buf.as_mut_ptr().cast(), buf.len() as u32) };
    if len <= 0 {
        return None;
    }
    // SAFETY: proc_pidpath 写入了 len 字节并以 NUL 结尾
    let cstr = unsafe { std::ffi::CStr::from_ptr(buf.as_ptr()) };
    Some(PathBuf::from(cstr.to_string_lossy().as_ref()))
}

fn ppid(pid: i32) -> Option<i32> {
    let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of::<libc::proc_bsdinfo>() as i32;
    // SAFETY: 传入的结构体大小与 flavor 要求一致
    let got = unsafe {
        libc::proc_pidinfo(
            pid,
            libc::PROC_PIDTBSDINFO,
            0,
            &mut info as *mut _ as *mut c_void,
            size,
        )
    };
    (got == size).then_some(info.pbi_ppid as i32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enclosing_app_finds_the_bundle() {
        let exe = Path::new("/Applications/ZCode.app/Contents/Resources/app/bin/zcode-cli");
        assert_eq!(
            enclosing_app(exe),
            Some(Path::new("/Applications/ZCode.app"))
        );
        // 包名里可以带点
        let exe = Path::new("/Applications/Visual Studio Code.app/Contents/MacOS/Electron");
        assert_eq!(
            enclosing_app(exe),
            Some(Path::new("/Applications/Visual Studio Code.app"))
        );
    }

    #[test]
    fn enclosing_app_takes_the_outermost_bundle() {
        // 命令行工具在嵌套的辅助包里：要的是外层的 ZCode.app
        let exe = Path::new("/Applications/ZCode.app/Contents/Frameworks/ZCode Helper.app/Contents/MacOS/ZCode Helper");
        assert_eq!(
            enclosing_app(exe),
            Some(Path::new("/Applications/ZCode.app"))
        );
    }

    #[test]
    fn enclosing_app_is_none_outside_a_bundle() {
        assert_eq!(enclosing_app(Path::new("/usr/local/bin/cargo")), None);
        assert_eq!(
            enclosing_app(Path::new("/opt/apple.swift/NotAnApp/Contents/x")),
            None
        );
    }

    #[test]
    fn canonical_trims_trailing_slash() {
        assert_eq!(
            canonical("/System/Library/CoreServices/Finder.app/"),
            "/System/Library/CoreServices/Finder.app"
        );
        assert_eq!(canonical("/does/not/exist/"), "/does/not/exist");
    }

    #[test]
    fn own_parent_chain_terminates() {
        let mut pid = std::process::id() as i32;
        let mut steps = 0;
        while let Some(parent) = ppid(pid) {
            assert!(parent > 0);
            pid = parent;
            steps += 1;
            assert!(steps < 64, "父进程链不该成环");
            if pid <= 1 {
                break;
            }
        }
        assert!(steps >= 1, "测试进程至少有父进程");
    }
}
