//! Dock 模块：再点一次前台应用的图标 → 最小化它的窗口（Windows 任务栏的习惯）。
//!
//! 重活都放在回调之外：
//! - 后台线程定期读 Dock 的辅助功能树，缓存「图标矩形 → 应用」；
//! - 拦截回调在左键按下时只做纯内存命中；命中的是前台应用的图标，才去问
//!   窗口服务器两件事（一次调用）：这个点上是不是 Dock 自己的窗口、前台应用
//!   有没有可见窗口。
//!
//! Dock 的图标结构（本机 macOS 27.2.1 实测）：应用元素 → `AXList` → 一串
//! `AXDockItem`，其中 `AXSubrole == AXApplicationDockItem` 的是应用图标，
//! 带 `AXFrame` 和指向 .app 包的 `AXURL`。

use std::ffi::{c_void, CStr};

use super::cf::{self, Cf};
use super::ffi::*;
use super::focus::RunningApp;
use super::geometry::{CGPoint, CGRect};

/// 单次 AX 消息的超时（秒）。对方卡住时不至于把后台线程拖死。
const AX_TIMEOUT: f32 = 1.0;

/// 一个正在运行的应用的 Dock 图标。
#[derive(Clone, Debug, PartialEq)]
pub struct Tile {
    pub title: String,
    pub rect: CGRect,
    pub pid: i32,
    /// 应用的规范化 bundle 路径
    pub path: String,
}

/// 同一个应用只保留第一个图标。
///
/// 本机（macOS 27.2）实测：Dock 会对同一个运行中的应用出现第二个图标，属性
/// （类型、标题、URL）与第一个完全相同，没法区分哪个是真的。重复的一律不认，
/// 交给系统处理——宁可这个应用上功能失效，也不能把用户的一次点击吞错。
pub fn keep_first_per_app(tiles: Vec<Tile>) -> Vec<Tile> {
    let mut seen = Vec::new();
    tiles
        .into_iter()
        .filter(|tile| {
            if seen.contains(&tile.path) {
                false
            } else {
                seen.push(tile.path.clone());
                true
            }
        })
        .collect()
}

/// 窗口服务器给的窗口摘要（只取判断要用的字段）。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WinInfo {
    pub pid: i32,
    pub layer: i64,
    pub alpha: f64,
    pub bounds: CGRect,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DockDecision {
    /// 吞掉这次点击并最小化窗口
    Minimize,
    /// 交给系统
    Pass,
}

/// 落点所在的运行中应用图标。
pub fn tile_at(tiles: &[Tile], point: CGPoint) -> Option<&Tile> {
    tiles.iter().find(|t| t.rect.contains(point))
}

/// 点击已经落在前台应用的图标上，接下来决定吞不吞。
pub fn decide(point: CGPoint, front_pid: i32, dock_pid: i32, windows: &[WinInfo]) -> DockDecision {
    // 落点上必须有 Dock 自己的窗口。Dock 开了自动隐藏时图标还在缓存里，但那个
    // 位置露出来的是别的窗口，不能当成点在图标上。
    let on_dock = dock_pid != 0
        && windows
            .iter()
            .any(|w| w.pid == dock_pid && w.bounds.contains(point));
    if !on_dock {
        return DockDecision::Pass;
    }
    // 前台应用一个可见窗口都没有（全最小化了）：这次点击的意思是「还原」，交给系统
    let has_visible = windows
        .iter()
        .any(|w| w.pid == front_pid && w.layer == 0 && w.alpha > 0.01);
    if has_visible {
        DockDecision::Minimize
    } else {
        DockDecision::Pass
    }
}

/// 屏幕上当前可见的窗口。一次窗口服务器调用，只在点中 Dock 图标时才做。
pub fn on_screen_windows() -> Vec<WinInfo> {
    // SAFETY: 只读查询；返回的数组带一个引用，交给 Cf 释放
    let list =
        unsafe { Cf::owned(CGWindowListCopyWindowInfo(LIST_ON_SCREEN_ONLY, 0) as *mut c_void) };
    if list.is_null() {
        return Vec::new();
    }
    let list = list.as_cf();
    // SAFETY: list 是有效的 CF 对象
    if !unsafe { cf::is_array(list) } {
        return Vec::new();
    }
    // SAFETY: list 是有效的 CFArray，存活到函数结束；取出的元素由它持有
    unsafe {
        let n = CFArrayGetCount(list);
        let mut out = Vec::with_capacity(n as usize);
        for i in 0..n {
            let dict = CFArrayGetValueAtIndex(list, i);
            if dict.is_null() || !cf::is_dictionary(dict) {
                continue;
            }
            let mut info = WinInfo {
                pid: 0,
                layer: 0,
                alpha: 1.0,
                bounds: CGRect::default(),
            };
            if let Some(v) = cf::dict_get(dict, kCGWindowOwnerPID) {
                info.pid = cf::number_to_i64(v).unwrap_or(i64::MAX) as i32;
            }
            if let Some(v) = cf::dict_get(dict, kCGWindowLayer) {
                info.layer = cf::number_to_i64(v).unwrap_or(0);
            }
            if let Some(v) = cf::dict_get(dict, kCGWindowAlpha) {
                info.alpha = cf::number_to_f64(v).unwrap_or(1.0);
            }
            if let Some(v) = cf::dict_get(dict, kCGWindowBounds) {
                CGRectMakeWithDictionaryRepresentation(v, &mut info.bounds);
            }
            out.push(info);
        }
        out
    }
}

/// 把前台应用所有没最小化的窗口最小化，返回处理了的窗口数。
pub fn minimize_app(pid: i32) -> usize {
    // SAFETY: 只是创建 AX 元素句柄
    let app = unsafe { Cf::owned(AXUIElementCreateApplication(pid)) };
    if app.is_null() {
        return 0;
    }
    // SAFETY: app 是有效的 AXUIElement
    unsafe { AXUIElementSetMessagingTimeout(app.0, AX_TIMEOUT) };
    // SAFETY: app 是有效的 AXUIElement
    let Some(windows) = (unsafe { ax_attr(app.0, c"AXWindows") }) else {
        return 0;
    };
    let minimized = cf::cfstr(c"AXMinimized");
    let windows = windows.as_cf();
    if !unsafe { cf::is_array(windows) } {
        return 0;
    }
    let mut count = 0;
    // SAFETY: windows 是有效的 CFArray，存活到函数结束
    unsafe {
        for i in 0..CFArrayGetCount(windows) {
            let window = CFArrayGetValueAtIndex(windows, i) as AXUIElementRef;
            if window.is_null() || ax_bool(window, c"AXMinimized") != Some(false) {
                continue;
            }
            // 最小化全屏窗口会把用户从当前空间拽出来，不动
            if ax_bool(window, c"AXFullScreen") == Some(true) {
                continue;
            }
            // SAFETY: window 是有效的 AXUIElement，kCFBooleanTrue 是常量对象
            if AXUIElementSetAttributeValue(window, minimized.as_cf(), kCFBooleanTrue) == 0 {
                count += 1;
            }
        }
    }
    count
}

/// 读 Dock 当前的应用图标。Dock 的辅助功能树读不出来时返回空表（功能自然失效，
/// 不影响按键重映射）。
pub fn read_tiles(dock_pid: i32, apps: &[RunningApp]) -> Vec<Tile> {
    // SAFETY: 只是创建 AX 元素句柄
    let dock = unsafe { Cf::owned(AXUIElementCreateApplication(dock_pid)) };
    if dock.is_null() {
        return Vec::new();
    }
    // SAFETY: dock 是有效的 AXUIElement
    unsafe { AXUIElementSetMessagingTimeout(dock.0, AX_TIMEOUT) };
    // SAFETY: dock 是有效的 AXUIElement
    let Some(list) = (unsafe { ax_list(dock.0) }) else {
        return Vec::new();
    };
    let Some(items) = (unsafe { ax_attr(list.0, c"AXChildren") }) else {
        return Vec::new();
    };
    let items = items.as_cf();
    if !unsafe { cf::is_array(items) } {
        return Vec::new();
    }
    let mut tiles = Vec::new();
    // SAFETY: items 是有效的 CFArray，存活到函数结束；元素由它持有
    unsafe {
        for i in 0..CFArrayGetCount(items) {
            let item = CFArrayGetValueAtIndex(items, i) as AXUIElementRef;
            if item.is_null()
                || ax_string(item, c"AXSubrole").as_deref() != Some("AXApplicationDockItem")
            {
                continue;
            }
            let (Some(rect), Some(path)) = (ax_rect(item), ax_url_path(item)) else {
                continue;
            };
            let path = super::focus::canonical(&path);
            let Some(pid) = apps.iter().find(|a| a.path == path).map(|a| a.pid) else {
                continue;
            };
            tiles.push(Tile {
                title: ax_string(item, c"AXTitle").unwrap_or_default(),
                rect,
                pid,
                path,
            });
        }
    }
    keep_first_per_app(tiles)
}

// ---------------------------------------------------------------- AX 小工具

/// 读一个属性，返回带引用计数的值；不支持或读不到返回 None。
///
/// # Safety
///
/// `element` 必须是有效的 AXUIElement。
unsafe fn ax_attr(element: AXUIElementRef, attr: &CStr) -> Option<Cf> {
    let name = cf::cfstr(attr);
    let mut value: CFTypeRef = std::ptr::null();
    let err = AXUIElementCopyAttributeValue(element, name.as_cf(), &mut value);
    if err != 0 || value.is_null() {
        return None;
    }
    // Copy 系列返回值带一个引用，交给 Cf 释放
    Some(Cf::owned(value as *mut c_void))
}

/// # Safety
///
/// `element` 必须是有效的 AXUIElement。
unsafe fn ax_string(element: AXUIElementRef, attr: &CStr) -> Option<String> {
    let v = ax_attr(element, attr)?;
    cf::is_string(v.as_cf())
        .then(|| cf::to_rust(v.as_cf()))
        .flatten()
}

/// # Safety
///
/// `element` 必须是有效的 AXUIElement。
unsafe fn ax_bool(element: AXUIElementRef, attr: &CStr) -> Option<bool> {
    let v = ax_attr(element, attr)?;
    cf::is_boolean(v.as_cf()).then(|| CFBooleanGetValue(v.as_cf()))
}

/// # Safety
///
/// `element` 必须是有效的 AXUIElement。
unsafe fn ax_rect(element: AXUIElementRef) -> Option<CGRect> {
    let v = ax_attr(element, c"AXFrame")?;
    if CFGetTypeID(v.as_cf()) != AXValueGetTypeID() {
        return None;
    }
    let mut rect = CGRect::default();
    let ok = AXValueGetValue(
        v.0,
        AX_VALUE_CGRECT,
        &mut rect as *mut CGRect as *mut c_void,
    );
    ok.then_some(rect)
}

/// 应用图标指向的 .app 包路径。
///
/// # Safety
///
/// `element` 必须是有效的 AXUIElement。
unsafe fn ax_url_path(element: AXUIElementRef) -> Option<String> {
    let v = ax_attr(element, c"AXURL")?;
    if !cf::is_url(v.as_cf()) {
        return None;
    }
    // kCFURLPOSIXPathStyle = 0；返回值带一个引用
    let path = Cf::owned(CFURLCopyFileSystemPath(v.as_cf(), 0) as *mut c_void);
    (!path.is_null())
        .then(|| cf::to_rust(path.as_cf()))
        .flatten()
}

/// Dock 的图标都挂在角色为 AXList 的元素下。本机它在应用元素的下一层；这里往下
/// 多搜一层，结构微调时不至于直接失效。
///
/// # Safety
///
/// `app` 必须是有效的 AXUIElement。
unsafe fn ax_list(app: AXUIElementRef) -> Option<Cf> {
    let children = ax_attr(app, c"AXChildren")?;
    let children = children.as_cf();
    if !cf::is_array(children) {
        return None;
    }
    for i in 0..CFArrayGetCount(children) {
        let child = CFArrayGetValueAtIndex(children, i) as AXUIElementRef;
        if child.is_null() {
            continue;
        }
        if ax_string(child, c"AXRole").as_deref() == Some("AXList") {
            // 返回值要独立于 children 存活：自己持有一份引用
            return Some(Cf::owned(CFRetain(child) as *mut c_void));
        }
        let grandchildren = match ax_attr(child, c"AXChildren") {
            Some(g) => g,
            None => continue,
        };
        let grandchildren = grandchildren.as_cf();
        if !cf::is_array(grandchildren) {
            continue;
        }
        for j in 0..CFArrayGetCount(grandchildren) {
            let gc = CFArrayGetValueAtIndex(grandchildren, j) as AXUIElementRef;
            if !gc.is_null() && ax_string(gc, c"AXRole").as_deref() == Some("AXList") {
                return Some(Cf::owned(CFRetain(gc) as *mut c_void));
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tile(x: f64, y: f64, pid: i32, path: &str) -> Tile {
        Tile {
            title: String::new(),
            rect: CGRect::new(x, y, 59.0, 75.0),
            pid,
            path: path.into(),
        }
    }

    const DOCK: i32 = 796;

    fn on_dock(bounds: CGRect) -> WinInfo {
        WinInfo {
            pid: DOCK,
            layer: 20,
            alpha: 1.0,
            bounds,
        }
    }

    fn app_window(pid: i32) -> WinInfo {
        WinInfo {
            pid,
            layer: 0,
            alpha: 1.0,
            bounds: CGRect::new(0.0, 0.0, 1200.0, 900.0),
        }
    }

    #[test]
    fn tile_at_matches_the_running_app_under_the_point() {
        let tiles = [
            tile(166.0, 1000.0, 1, "/A.app"),
            tile(225.0, 1000.0, 2, "/B.app"),
        ];
        let hit = tile_at(
            &tiles,
            CGPoint {
                x: 240.0,
                y: 1030.0,
            },
        );
        assert_eq!(hit.map(|t| t.pid), Some(2));
        assert!(tile_at(
            &tiles,
            CGPoint {
                x: 150.0,
                y: 1030.0
            }
        )
        .is_none());
        assert!(tile_at(&tiles, CGPoint { x: 240.0, y: 500.0 }).is_none());
    }

    #[test]
    fn duplicated_tile_of_the_same_app_is_dropped() {
        let all = vec![
            tile(1520.0, 1000.0, 7, "/Apps/LaterScreen.app"),
            tile(1580.0, 1000.0, 8, "/Apps/LaterTerm.app"),
            tile(1638.0, 1000.0, 7, "/Apps/LaterScreen.app"),
        ];
        let kept = keep_first_per_app(all);
        assert_eq!(
            kept.iter().map(|t| t.path.as_str()).collect::<Vec<_>>(),
            ["/Apps/LaterScreen.app", "/Apps/LaterTerm.app"]
        );
        assert!(
            kept.iter().all(|t| t.rect.origin.x < 1600.0),
            "丢掉的是后面那个重复图标"
        );
    }

    #[test]
    fn click_on_front_app_icon_minimizes() {
        let windows = [
            on_dock(CGRect::new(158.0, 995.0, 1604.0, 75.0)),
            app_window(42),
        ];
        assert_eq!(
            decide(
                CGPoint {
                    x: 190.0,
                    y: 1030.0
                },
                42,
                DOCK,
                &windows
            ),
            DockDecision::Minimize
        );
    }

    #[test]
    fn hidden_dock_window_passes_through() {
        // Dock 自动隐藏：那个位置是别的应用的窗口
        let windows = [app_window(42)];
        assert_eq!(
            decide(
                CGPoint {
                    x: 190.0,
                    y: 1030.0
                },
                42,
                DOCK,
                &windows
            ),
            DockDecision::Pass
        );
        assert_eq!(
            decide(
                CGPoint {
                    x: 190.0,
                    y: 1030.0
                },
                42,
                0,
                &windows
            ),
            DockDecision::Pass
        );
    }

    #[test]
    fn front_app_without_visible_window_is_a_restore_click() {
        let windows = [on_dock(CGRect::new(158.0, 995.0, 1604.0, 75.0))];
        assert_eq!(
            decide(
                CGPoint {
                    x: 190.0,
                    y: 1030.0
                },
                42,
                DOCK,
                &windows
            ),
            DockDecision::Pass
        );
        // 全透明或浮在别的层上的窗口不算可见
        let ghost = [
            on_dock(CGRect::new(158.0, 995.0, 1604.0, 75.0)),
            WinInfo {
                pid: 42,
                layer: 0,
                alpha: 0.0,
                bounds: CGRect::default(),
            },
            WinInfo {
                pid: 42,
                layer: 25,
                alpha: 1.0,
                bounds: CGRect::default(),
            },
        ];
        assert_eq!(
            decide(
                CGPoint {
                    x: 190.0,
                    y: 1030.0
                },
                42,
                DOCK,
                &ghost
            ),
            DockDecision::Pass
        );
    }
}
