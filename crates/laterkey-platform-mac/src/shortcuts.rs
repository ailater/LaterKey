//! 快捷键速览的数据源（一次性读取，跑在 GUI 线程派生的工作线程上）：
//!
//! 1. 当前应用菜单：辅助功能 API 遍历 `AXMenuBar → 菜单 → 菜单项`，读
//!    `AXTitle / AXMenuItemCmdChar / AXMenuItemCmdModifiers / AXMenuItemCmdVirtualKey`。
//!    修饰键掩码的本机实测含义见 core 的 [`laterkey_core::ax_modifiers_to_flags`]。
//! 2. 系统快捷键：`com.apple.symbolichotkeys` 的 `AppleSymbolicHotKeys`
//!    （CFPreferences 实时值，不是落盘 plist——用户改过键位后旧值会迟到）。
//!    id→名称对照表内置；表里没有的 id 跳过，宁可少列不错列。
//!
//! 任何一步读不到都返回空表：速览是锦上添花，不能因为它拖垮或报错退出。

use std::ffi::{c_void, CStr, CString};

use laterkey_core::{
    ax_modifiers_to_flags,
    flags::{ALTERNATE, COMMAND, CONTROL, SECONDARY_FN, SHIFT},
    keys, ShortcutEntry, ShortcutGroup, ShortcutKey,
};

use super::cf::{self, Cf};
use super::ffi::*;
use super::focus;

/// 单次 AX 消息的超时（秒），同 dock.rs 的口径。
const AX_TIMEOUT: f32 = 1.0;
/// 一次读取的条目总量上限：巨型应用（Electron 类）菜单再大也不能拖死读取线程。
const MAX_ENTRIES: usize = 300;
/// 子菜单只展开一层：`文件 › 导出`，更深的不递归。
const MAX_MENU_DEPTH: usize = 1;

/// 一份完整的速览数据：当前应用菜单 + 系统快捷键。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CheatSheet {
    /// 前台应用显示名（读不到为空，界面自己兜底）
    pub app_name: String,
    pub app_groups: Vec<ShortcutGroup>,
    pub system_groups: Vec<ShortcutGroup>,
}

/// 读前台应用的菜单快捷键 + 系统快捷键。`pid` 是打开速览那一刻的前台应用。
pub fn read_cheatsheet(pid: i32) -> CheatSheet {
    let (app_name, app_groups) = read_menu_shortcuts(pid);
    CheatSheet {
        app_name,
        app_groups,
        system_groups: read_system_shortcuts(),
    }
}

// ---------------------------------------------------------------- 应用菜单

/// 读一个应用的菜单栏。读不到菜单（权限缺失、特殊进程）返回空组列表。
pub fn read_menu_shortcuts(pid: i32) -> (String, Vec<ShortcutGroup>) {
    let name = focus::bundle_id_for_pid(pid)
        .and_then(|bundle| focus::app_name_for_bundle(&bundle))
        .unwrap_or_default();
    // SAFETY: 只是创建 AX 元素句柄
    let app = unsafe { Cf::owned(AXUIElementCreateApplication(pid)) };
    if app.is_null() {
        return (name, Vec::new());
    }
    // SAFETY: app 是有效的 AXUIElement
    unsafe { AXUIElementSetMessagingTimeout(app.0, AX_TIMEOUT) };
    // Electron 应用默认不暴露辅助功能树（ZCode/VS Code 实测 AXMenuBar 读不到），
    // 必须先置 AXManualAccessibility 才肯交出菜单；原生应用设置会失败，忽略即可。
    // 开关打开后树是异步构建的，首次读取失败就等一拍再试一次。
    let manual = cf::cfstr(c"AXManualAccessibility");
    // SAFETY: app 是有效的 AXUIElement，kCFBooleanTrue 是常量对象
    unsafe { AXUIElementSetAttributeValue(app.0, manual.as_cf(), kCFBooleanTrue) };
    // SAFETY: app 是有效的 AXUIElement
    let mut menubar = unsafe { ax_attr(app.0, c"AXMenuBar") };
    if menubar.is_none() {
        std::thread::sleep(std::time::Duration::from_millis(300));
        // SAFETY: app 是有效的 AXUIElement
        menubar = unsafe { ax_attr(app.0, c"AXMenuBar") };
    }
    let Some(menubar) = menubar else {
        return (name, Vec::new());
    };
    let mut budget = MAX_ENTRIES;
    let groups = unsafe { collect_menu_groups(menubar.0, &mut budget) };
    (name, groups)
}

/// 遍历菜单栏的直接子菜单；每个菜单一组，子菜单单独成组（`父菜单 › 子菜单`）。
///
/// # Safety
///
/// `menubar` 必须是有效的 AXUIElement。
unsafe fn collect_menu_groups(menubar: AXUIElementRef, budget: &mut usize) -> Vec<ShortcutGroup> {
    let Some(children) = (unsafe { ax_attr(menubar, c"AXChildren") }) else {
        return Vec::new();
    };
    let children = children.as_cf();
    if !unsafe { cf::is_array(children) } {
        return Vec::new();
    }
    let mut groups = Vec::new();
    // SAFETY: children 是有效的 CFArray，存活到函数结束；元素由它持有
    unsafe {
        for i in 0..CFArrayGetCount(children) {
            let bar_item = CFArrayGetValueAtIndex(children, i) as AXUIElementRef;
            if bar_item.is_null() {
                continue;
            }
            let title = ax_string(bar_item, c"AXTitle").unwrap_or_default();
            // 空标题多为分隔结构；Apple 菜单没有 AXTitle 时给个固定名
            let title = if title.is_empty() {
                "应用菜单".to_string()
            } else {
                title
            };
            // 菜单栏子项（如「文件」）下面挂的是 AXMenu 元素，菜单项在
            // AXMenu 的 AXChildren 里——本机逐层探针实测（2026-11-25）
            let Some(menu) = ax_menu_child(bar_item) else {
                continue;
            };
            let mut entries = Vec::new();
            collect_menu_items(menu.0, &title, 0, &mut entries, budget);
            if !entries.is_empty() {
                groups.push(ShortcutGroup { title, entries });
            }
        }
    }
    groups
}

/// 元素的第一个 AXMenu 子元素（菜单栏项 → 弹出菜单；菜单项 → 子菜单）。
/// 返回独立持有的引用（CFRetain），不受临时数组存活期限制。
///
/// # Safety
///
/// `element` 必须是有效的 AXUIElement。
unsafe fn ax_menu_child(element: AXUIElementRef) -> Option<Cf> {
    let children = unsafe { ax_attr(element, c"AXChildren") }?;
    let children = children.as_cf();
    if !unsafe { cf::is_array(children) } || unsafe { CFArrayGetCount(children) } == 0 {
        return None;
    }
    // SAFETY: 第一个子元素由 children 持有
    let first = unsafe { CFArrayGetValueAtIndex(children, 0) } as AXUIElementRef;
    if first.is_null() || unsafe { ax_string(first, c"AXRole") }.as_deref() != Some("AXMenu") {
        return None;
    }
    // SAFETY: first 是别人持有的对象，这里 retain 一份独立引用
    Some(unsafe { Cf::owned(CFRetain(first) as *mut c_void) })
}

/// 收集一个菜单（或子菜单）下的菜单项。
///
/// # Safety
///
/// `menu` 必须是有效的 AXUIElement（角色 AXMenu 或菜单栏）。
unsafe fn collect_menu_items(
    menu: AXUIElementRef,
    group_title: &str,
    depth: usize,
    entries: &mut Vec<ShortcutEntry>,
    budget: &mut usize,
) {
    if *budget == 0 || depth > MAX_MENU_DEPTH {
        return;
    }
    let Some(children) = (unsafe { ax_attr(menu, c"AXChildren") }) else {
        return;
    };
    let children = children.as_cf();
    if !unsafe { cf::is_array(children) } {
        return;
    }
    // SAFETY: children 是有效的 CFArray，存活到函数结束；元素由它持有
    unsafe {
        for i in 0..CFArrayGetCount(children) {
            if *budget == 0 {
                return;
            }
            let item = CFArrayGetValueAtIndex(children, i) as AXUIElementRef;
            if item.is_null() {
                continue;
            }
            let title = ax_string(item, c"AXTitle").unwrap_or_default();
            if title.is_empty() {
                continue; // 分隔线等无标题结构
            }
            // 子菜单：菜单项下挂 AXMenu 元素。条目平铺进父组，
            // 组标题带「父 › 子」上下文，不再往下递归
            if depth < MAX_MENU_DEPTH {
                if let Some(sub) = ax_menu_child(item) {
                    let sub_title = format!("{group_title} › {title}");
                    collect_menu_items(sub.0, &sub_title, depth + 1, entries, budget);
                    continue;
                }
            }
            let Some(key) = menu_item_key(item) else {
                continue; // 没有快捷键的普通菜单项
            };
            let modifiers =
                ax_modifiers_to_flags(ax_number(item, c"AXMenuItemCmdModifiers").unwrap_or(0));
            entries.push(ShortcutEntry {
                title,
                key,
                modifiers,
            });
            *budget -= 1;
        }
    }
}

/// 菜单项的主键：优先命令字符，其次虚拟键码；都没有就不是快捷键项。
///
/// # Safety
///
/// `item` 必须是有效的 AXUIElement。
unsafe fn menu_item_key(item: AXUIElementRef) -> Option<ShortcutKey> {
    if let Some(ch) = unsafe { ax_string(item, c"AXMenuItemCmdChar") } {
        if !laterkey_core::normalize_char(&ch).is_empty() {
            return Some(ShortcutKey::Char(ch));
        }
    }
    let vkey = unsafe { ax_number(item, c"AXMenuItemCmdVirtualKey") }.unwrap_or(0);
    (vkey > 0).then_some(ShortcutKey::VirtualKey(vkey))
}

// ---------------------------------------------------------------- 系统快捷键

const HOTKEY_APP_ID: &CStr = c"com.apple.symbolichotkeys";
const HOTKEY_KEY: &CStr = c"AppleSymbolicHotKeys";

/// 内置的 id →（分组，名称）对照。plist 里只有 id；不在表里的 id 跳过。
/// 无出厂默认值的哨兵：这类 id 只在用户 plist 里有条目时才显示。
const NO_DEFAULT: u64 = u64::MAX;

/// 已知 symbolic hotkey 的内置表：id →（分组，名称，出厂默认修饰键，出厂默认虚拟键码）。
/// 顺序即显示顺序。用户 plist 里有条目且启用 → 用实时值；没有条目或读不出
/// 键位 → 用出厂默认兜底（截屏组等 macOS 默认不落盘，参考工具也是这么列的）；
/// 条目存在但被用户禁用 → 不显示（再按出厂值列出来就是说谎）。
/// 出厂值取自 Apple 公开默认键位；分组归属沿用本机实测修正（60/61=输入法）。
const KNOWN_HOTKEYS: &[(i64, &str, &str, u64, i64)] = &[
    (
        52,
        "启动台和程序坞",
        "打开或关闭隐藏程序坞",
        ALTERNATE | COMMAND,
        0x02,
    ),
    (160, "启动台和程序坞", "启动台", NO_DEFAULT, 0),
    (32, "调度中心", "调度中心", CONTROL, keys::UP),
    (33, "调度中心", "应用窗口", CONTROL, keys::DOWN),
    (36, "调度中心", "显示桌面", 0, 0x67), // F11
    (79, "调度中心", "向左移动一个空间", CONTROL, keys::LEFT),
    (81, "调度中心", "向右移动一个空间", CONTROL, keys::RIGHT),
    (118, "调度中心", "切换到桌面 1", CONTROL, 0x12),
    (119, "调度中心", "切换到桌面 2", CONTROL, 0x13),
    (120, "调度中心", "切换到桌面 3", CONTROL, 0x14),
    (121, "调度中心", "切换到桌面 4", CONTROL, 0x15),
    // 2026-11-25 本机实测修正：60/61 是输入法切换（ROADMAP 旧对照写成了移动空间）
    (60, "输入法", "选择上一个输入法", CONTROL, keys::SPACE),
    (
        61,
        "输入法",
        "选择下一个输入法",
        CONTROL | ALTERNATE,
        keys::SPACE,
    ),
    (
        28,
        "屏幕快照",
        "将屏幕图片存储为文件",
        SHIFT | COMMAND,
        0x14,
    ), // 3
    (
        29,
        "屏幕快照",
        "将屏幕图片拷贝到剪贴板",
        CONTROL | SHIFT | COMMAND,
        0x14,
    ),
    (
        30,
        "屏幕快照",
        "将所选区域的图片存储为文件",
        SHIFT | COMMAND,
        0x15,
    ), // 4
    (
        31,
        "屏幕快照",
        "将所选区域的图片拷贝到剪贴板",
        CONTROL | SHIFT | COMMAND,
        0x15,
    ),
    (184, "屏幕快照", "截屏和录制选项", SHIFT | COMMAND, 0x17), // 5
    (64, "聚焦", "聚焦搜索", COMMAND, keys::SPACE),
    (65, "聚焦", "访达搜索", ALTERNATE | COMMAND, keys::SPACE),
    // 参考工具中的系统键盘/辅助功能组：部分 id 不在 symbolic hotkeys plist，
    // 按 macOS 默认键位兜底展示，避免系统区因配置未落盘而出现大块空白。
    (201, "键盘", "将焦点移到菜单栏", CONTROL, 0x7A),
    (202, "键盘", "将焦点移到 Dock", CONTROL, 0x63),
    (203, "键盘", "将焦点移到活跃窗口或新窗口", CONTROL, 0x76),
    (204, "键盘", "将焦点移到状态菜单", COMMAND, 0x64),
    (
        205,
        "键盘",
        "显示/隐藏手写输入",
        SHIFT | CONTROL,
        keys::SPACE,
    ),
    (206, "辅助功能", "打开或关闭缩放", ALTERNATE | COMMAND, 0x67),
    (
        207,
        "辅助功能",
        "反转颜色",
        CONTROL | ALTERNATE | COMMAND,
        0x67,
    ),
];

/// symbolic hotkeys 的修饰键掩码是 CGEventFlags 风格；只留类别位，
/// 设备位/其他杂位一律清掉。
fn hotkey_modifier_mask(raw: i64) -> u64 {
    (raw as u64) & (SHIFT | CONTROL | ALTERNATE | COMMAND | SECONDARY_FN)
}

/// id 在内置表里的（分组，名称）。目前只被表格自检测试用；
/// 读取循环直接遍历表本身。
#[cfg(test)]
fn known_hotkey(id: i64) -> Option<(&'static str, &'static str)> {
    KNOWN_HOTKEYS
        .iter()
        .find(|(known, _, _, _, _)| *known == id)
        .map(|(_, group, title, _, _)| (*group, *title))
}

/// plist 里一个 symbolic hotkey 条目的读取结果。
enum PlistHotkey {
    /// 字典里没有这个 id
    Missing,
    /// 条目存在但 enabled=false（用户明确关掉，不显示任何键位）
    Disabled,
    /// 条目存在且解析出了键位（键，原始修饰键掩码）
    Parsed(ShortcutKey, i64),
    /// 条目存在但读不出键位（值形状不对/参数残缺）
    Unparsed,
}

/// 从 AppleSymbolicHotKeys 字典读一个 id。
///
/// # Safety
///
/// `dict` 必须是有效的 CFDictionary。
unsafe fn plist_hotkey(dict: CFTypeRef, id: i64) -> PlistHotkey {
    // SAFETY: dict 是有效 CFDictionary；取出的值由它持有
    let Some(entry) = (unsafe {
        let key = cf::cfstr(&CString::new(id.to_string()).unwrap());
        cf::dict_get(dict, key.as_cf())
    }) else {
        return PlistHotkey::Missing;
    };
    // SAFETY: entry 必须先校验是字典（不是就跳过，绝不能直接取值）
    if !unsafe { cf::is_dictionary(entry) } {
        return PlistHotkey::Unparsed;
    }
    // SAFETY: entry 是有效 CFDictionary
    let enabled = unsafe {
        cf::dict_get(entry, cf::cfstr(c"enabled").as_cf())
            .filter(|v| cf::is_boolean(*v))
            .map(|v| CFBooleanGetValue(v))
    };
    if enabled == Some(false) {
        return PlistHotkey::Disabled;
    }
    // SAFETY: entry 是有效 CFDictionary
    unsafe {
        cf::dict_get(entry, cf::cfstr(c"value").as_cf())
            .filter(|v| cf::is_dictionary(*v))
            .and_then(|v| cf::dict_get(v, cf::cfstr(c"parameters").as_cf()))
            .filter(|v| cf::is_array(*v))
            .and_then(|parameters| parse_hotkey_parameters(parameters))
            .map_or(PlistHotkey::Unparsed, |(key, raw)| {
                PlistHotkey::Parsed(key, raw)
            })
    }
}

/// 读系统快捷键。内置表认识且未被用户禁用的条目，按分组聚合；
/// plist 没有条目的用出厂默认值兜底。
pub fn read_system_shortcuts() -> Vec<ShortcutGroup> {
    // SAFETY: 只读查询；返回值带一个引用，交给 Cf 释放
    let raw = unsafe {
        Cf::owned(CFPreferencesCopyAppValue(
            cf::cfstr(HOTKEY_KEY).as_cf(),
            cf::cfstr(HOTKEY_APP_ID).as_cf(),
        ) as *mut c_void)
    };
    if raw.is_null() || !unsafe { cf::is_dictionary(raw.as_cf()) } {
        return Vec::new();
    }
    let dict = raw.as_cf();
    let mut groups: Vec<ShortcutGroup> = Vec::new();
    for &(id, group, title, def_mods, def_key) in KNOWN_HOTKEYS {
        // SAFETY: dict 是有效 CFDictionary
        let (key, modifiers) = match (unsafe { plist_hotkey(dict, id) }, def_mods) {
            (PlistHotkey::Disabled, _) => continue,
            (PlistHotkey::Parsed(key, raw_modifiers), _) => {
                (key, hotkey_modifier_mask(raw_modifiers))
            }
            (PlistHotkey::Missing | PlistHotkey::Unparsed, NO_DEFAULT) => continue,
            (PlistHotkey::Missing | PlistHotkey::Unparsed, def_mods) => {
                (ShortcutKey::VirtualKey(def_key), def_mods)
            }
        };
        let group_entry = match groups.iter_mut().find(|g| g.title == group) {
            Some(g) => g,
            None => {
                groups.push(ShortcutGroup {
                    title: group.to_string(),
                    entries: Vec::new(),
                });
                groups.last_mut().unwrap()
            }
        };
        group_entry.entries.push(ShortcutEntry {
            title: title.to_string(),
            key,
            modifiers,
        });
    }
    groups
}

/// 解析 `parameters: [字符, 虚拟键码, 修饰键]`。
///
/// # Safety
///
/// `parameters` 必须是有效的 CFArray。
unsafe fn parse_hotkey_parameters(parameters: CFTypeRef) -> Option<(ShortcutKey, i64)> {
    // SAFETY: parameters 是有效 CFArray，存活到函数结束
    unsafe {
        if CFArrayGetCount(parameters) < 3 {
            return None;
        }
        let char_value = CFArrayGetValueAtIndex(parameters, 0);
        let vkey_value = CFArrayGetValueAtIndex(parameters, 1);
        let flags_value = CFArrayGetValueAtIndex(parameters, 2);
        let vkey = (!vkey_value.is_null() && cf::is_number(vkey_value))
            .then(|| cf::number_to_i64(vkey_value))
            .flatten()
            .unwrap_or(0);
        let raw_modifiers = (!flags_value.is_null() && cf::is_number(flags_value))
            .then(|| cf::number_to_i64(flags_value))
            .flatten()
            .unwrap_or(0);
        let key = if !char_value.is_null() && cf::is_string(char_value) {
            let ch = cf::to_rust(char_value).unwrap_or_default();
            if ch.is_empty() {
                ShortcutKey::VirtualKey(vkey)
            } else {
                ShortcutKey::Char(ch)
            }
        } else {
            ShortcutKey::VirtualKey(vkey)
        };
        Some((key, raw_modifiers))
    }
}

// ---------------------------------------------------------------- AX 小工具

/// 读一个属性，返回带引用计数的值（与 dock.rs 同款，模块私有）。
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
    Some(unsafe { Cf::owned(value as *mut c_void) })
}

/// # Safety
///
/// `element` 必须是有效的 AXUIElement。
unsafe fn ax_string(element: AXUIElementRef, attr: &CStr) -> Option<String> {
    let v = unsafe { ax_attr(element, attr) }?;
    cf::is_string(v.as_cf())
        .then(|| unsafe { cf::to_rust(v.as_cf()) })
        .flatten()
}

/// # Safety
///
/// `element` 必须是有效的 AXUIElement。
unsafe fn ax_number(element: AXUIElementRef, attr: &CStr) -> Option<i64> {
    let v = unsafe { ax_attr(element, attr) }?;
    if !cf::is_number(v.as_cf()) {
        return None;
    }
    unsafe { cf::number_to_i64(v.as_cf()) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hotkey_modifier_mask_keeps_only_category_bits() {
        // ⌘Space 的参数掩码 0x100000：只留类别位
        assert_eq!(hotkey_modifier_mask(0x10_0000), COMMAND);
        // 设备位（如左 ⌘ 0x8）与未知高位一律清掉
        assert_eq!(hotkey_modifier_mask(0x10_0008), COMMAND);
        assert_eq!(
            hotkey_modifier_mask(-1),
            SHIFT | CONTROL | ALTERNATE | COMMAND | SECONDARY_FN
        );
    }

    #[test]
    fn known_hotkey_table_lookups() {
        assert_eq!(known_hotkey(32), Some(("调度中心", "调度中心")));
        assert_eq!(known_hotkey(60), Some(("输入法", "选择上一个输入法")));
        assert_eq!(known_hotkey(9999), None);
    }

    /// 真机端到端：读当前前台应用的菜单 + 系统快捷键并打印条数。
    ///   LATERKEY_TEST_E2E=1 cargo test -p laterkey-platform-mac read_cheatsheet_real -- --ignored --nocapture
    #[test]
    #[ignore = "真机：需要辅助功能权限，需 LATERKEY_TEST_E2E=1"]
    fn read_cheatsheet_real_apps_and_system() {
        if std::env::var("LATERKEY_TEST_E2E").as_deref() != Ok("1") {
            eprintln!("跳过：需 LATERKEY_TEST_E2E=1");
            return;
        }
        assert!(super::super::ax_trusted(), "需要辅助功能权限");
        let pid = focus::frontmost_pid().expect("有前台应用");
        let sheet = read_cheatsheet(pid);
        eprintln!(
            "前台 pid {pid}：{}（{} 组菜单）",
            sheet.app_name,
            sheet.app_groups.len()
        );
        for group in &sheet.app_groups {
            eprintln!("  [{}] {} 条", group.title, group.entries.len());
            for entry in group.entries.iter().take(3) {
                eprintln!(
                    "    {:?} {:?} mods={:#x}",
                    entry.title, entry.key, entry.modifiers
                );
            }
        }
        eprintln!("系统快捷键：{} 组", sheet.system_groups.len());
        for group in &sheet.system_groups {
            eprintln!("  [{}] {} 条", group.title, group.entries.len());
            for entry in &group.entries {
                eprintln!("    {} mods={:#x}", entry.title, entry.modifiers);
            }
        }
        assert!(
            !sheet.system_groups.is_empty(),
            "系统快捷键至少应有聚焦/截屏一组可读"
        );
    }
}
