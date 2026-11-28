//! 菜单栏图标：tray-icon（LaterScreen 同款）。菜单里直接放各项功能的开关，
//! 另有「设置…」打开 egui 面板、「退出」。
//!
//! 托盘在 winit 事件循环里创建（resumed 时）。开关变化经通道发给后台线程
//! 重建规则；配置存 toggles.toml，与手写的 config.toml 分开。

use std::sync::{Arc, RwLock};

use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, TrayIconBuilder};

use crate::toggles::Toggles;

pub const QUIT_ID: &str = "quit";
pub const SETTINGS_ID: &str = "settings";
pub const ACCESSIBILITY_ID: &str = "accessibility";
/// 手动呼出快捷键速览浮窗（长按 Ctrl 手势之外的入口）
pub const PEEK_ID: &str = "peek_now";

const ITEMS: &[(&str, &str)] = &[
    ("ctrl_as_cmd", "Ctrl 当作 ⌘（复制粘贴）"),
    ("cmd_as_ctrl", "Win 当作 ⌃（终端中断）"),
    ("alt_tab", "Alt+Tab 切换应用"),
    ("ctrl_arrows", "Ctrl+←/→/退格 按词跳转"),
    ("win_key_lock", "Win+L 锁屏"),
    ("win_key_clipboard", "Win+V 剪贴板历史"),
    ("task_manager", "Ctrl+Alt+Del 任务管理器"),
    ("settings_hotkey", "Ctrl+Alt+K 打开设置"),
    ("shortcuts_peek", "长按 Ctrl 显示快捷键速览"),
    ("dock_click_minimize", "再点 Dock 图标最小化"),
    ("finder_f2", "访达里 F2 重命名"),
    ("browser_f5", "浏览器里 F5 刷新"),
    ("finder_terminal", "访达里 Ctrl+/ 打开终端"),
];

/// 用内嵌 PNG 生成模板图标。失败则退回无图标（菜单照样可用）。
fn make_icon() -> Option<Icon> {
    let bytes = include_bytes!("../assets/menu@2x.png");
    let img = image::load_from_memory_with_format(bytes, image::ImageFormat::Png).ok()?;
    let rgba = img.to_rgba8();
    let (w, h) = rgba.dimensions();
    Icon::from_rgba(rgba.into_raw(), w, h).ok()
}

pub struct Tray {
    _tray: tray_icon::TrayIcon,
    master: CheckMenuItem,
    items: Vec<(String, CheckMenuItem)>,
}

impl Tray {
    pub fn new(toggles: &Toggles) -> Result<Self, String> {
        let menu = Menu::new();

        let master = CheckMenuItem::with_id("master", "启用 LaterKey", true, toggles.master, None);
        let _ = menu.append(&master);
        let _ = menu.append(&PredefinedMenuItem::separator());

        let mut items = Vec::new();
        for (id, label) in ITEMS {
            let checked = field(toggles, id);
            let item = CheckMenuItem::with_id(*id, *label, true, checked, None);
            let _ = menu.append(&item);
            items.push((id.to_string(), item));
        }

        let _ = menu.append(&PredefinedMenuItem::separator());
        let settings = MenuItem::with_id(SETTINGS_ID, "设置…", true, None);
        let _ = menu.append(&settings);
        let peek = MenuItem::with_id(PEEK_ID, "快捷键速览", true, None);
        let _ = menu.append(&peek);
        let ax = MenuItem::with_id(ACCESSIBILITY_ID, "打开辅助功能授权", true, None);
        let _ = menu.append(&ax);
        let _ = menu.append(&PredefinedMenuItem::separator());
        let quit = MenuItem::with_id(QUIT_ID, "退出 LaterKey", true, None);
        let _ = menu.append(&quit);

        let mut builder = TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_tooltip("LaterKey：让键盘按 Windows 习惯工作");
        if let Some(icon) = make_icon() {
            builder = builder.with_icon(icon);
        }
        #[cfg(target_os = "macos")]
        let builder = builder.with_icon_as_template(true);

        let tray = builder
            .build()
            .map_err(|e| format!("菜单栏图标创建失败：{e}"))?;
        Ok(Self {
            _tray: tray,
            master,
            items,
        })
    }

    /// 显示/隐藏菜单栏图标（隐藏后用 Ctrl+Alt+K 唤出设置面板恢复）。
    pub fn set_visible(&self, visible: bool) {
        let _ = self._tray.set_visible(visible);
    }

    /// 界面/后台改动后刷新勾选状态。
    pub fn sync(&self, toggles: &Toggles) {
        self.master.set_checked(toggles.master);
        for (id, item) in &self.items {
            item.set_checked(field(toggles, id));
        }
    }
}

fn field(t: &Toggles, id: &str) -> bool {
    match id {
        "master" => t.master,
        "ctrl_as_cmd" => t.ctrl_as_cmd,
        "cmd_as_ctrl" => t.cmd_as_ctrl,
        "alt_tab" => t.alt_tab,
        "ctrl_arrows" => t.ctrl_arrows,
        "win_key_lock" => t.win_key_lock,
        "win_key_clipboard" => t.win_key_clipboard,
        "task_manager" => t.task_manager,
        "settings_hotkey" => t.settings_hotkey,
        "shortcuts_peek" => t.shortcuts_peek,
        "dock_click_minimize" => t.dock_click_minimize,
        "finder_f2" => t.finder_f2,
        "browser_f5" => t.browser_f5,
        "finder_terminal" => t.finder_terminal,
        _ => false,
    }
}

/// 把菜单事件翻译成一个动作。调用方（winit 事件循环）决定怎么处理。
pub enum Action {
    Toggle(String, bool),
    OpenSettings,
    /// 呼出快捷键速览浮窗
    ShortcutsPeek,
    OpenAccessibility,
    Quit,
}

pub fn translate(ev: &MenuEvent, toggles: &Arc<RwLock<Toggles>>) -> Option<Action> {
    let id = ev.id().0.as_str();
    match id {
        QUIT_ID => Some(Action::Quit),
        SETTINGS_ID => Some(Action::OpenSettings),
        PEEK_ID => Some(Action::ShortcutsPeek),
        ACCESSIBILITY_ID => Some(Action::OpenAccessibility),
        other => {
            let is_known = other == "master" || ITEMS.iter().any(|(k, _)| *k == other);
            if !is_known {
                return None;
            }
            let current = {
                let t = toggles.read().unwrap_or_else(|p| p.into_inner());
                field(&t, other)
            };
            Some(Action::Toggle(other.to_string(), !current))
        }
    }
}

/// 打开系统设置的辅助功能面板。
pub fn open_accessibility() {
    open_privacy_pane("Privacy_Accessibility");
}

/// 打开系统设置的「监听事件」面板（macOS 15+ 的独立权限项）。
pub fn open_listen_event() {
    open_privacy_pane("Privacy_ListenEvent");
}

fn open_privacy_pane(anchor: &str) {
    let _ = std::process::Command::new("open")
        .arg(format!(
            "x-apple.systempreferences:com.apple.preference.security?{anchor}"
        ))
        .spawn();
}
