//! 运行时开关：菜单栏与设置面板改的就是这份状态，后台线程按它重建规则。
//!
//! 存成独立的 `~/.config/laterkey/toggles.toml`，与手写的 `config.toml` 分开：
//! 用户的配置文件里可能有注释和自定义应用列表，界面写回时不能把它们抹掉。

use std::path::PathBuf;

use laterkey_core::{Compiled, Config};

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Toggles {
    pub master: bool,
    pub ctrl_as_cmd: bool,
    pub cmd_as_ctrl: bool,
    pub alt_tab: bool,
    pub ctrl_arrows: bool,
    pub win_key_lock: bool,
    pub win_key_clipboard: bool,
    pub dock_click_minimize: bool,
    pub finder_f2: bool,
    pub browser_f5: bool,
}

impl Default for Toggles {
    fn default() -> Self {
        Self {
            master: true,
            ctrl_as_cmd: true,
            cmd_as_ctrl: true,
            alt_tab: true,
            ctrl_arrows: true,
            win_key_lock: true,
            // 剪贴板历史面板尚未实现，开了会吞掉 ⌘V
            win_key_clipboard: false,
            dock_click_minimize: true,
            finder_f2: true,
            browser_f5: true,
        }
    }
}

pub fn path() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config/laterkey/toggles.toml"))
}

impl Toggles {
    pub fn load() -> Self {
        let Some(path) = path() else {
            return Self::default();
        };
        std::fs::read_to_string(&path)
            .ok()
            .and_then(|text| toml::from_str(&text).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) {
        let Some(path) = path() else {
            return;
        };
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(text) = toml::to_string_pretty(self) {
            let _ = std::fs::write(&path, text);
        }
    }
}

/// 把开关叠到基础配置上。关掉的规则从编译结果里移除。
pub fn apply(base: &Config, toggles: &Toggles) -> Result<Compiled, String> {
    let mut cfg = base.clone();
    cfg.remap.ctrl_as_cmd = toggles.master && toggles.ctrl_as_cmd;
    cfg.remap.cmd_as_ctrl = toggles.master && toggles.cmd_as_ctrl;
    cfg.remap.alt_tab = toggles.master && toggles.alt_tab;
    cfg.remap.ctrl_arrows = toggles.master && toggles.ctrl_arrows;
    cfg.remap.win_key_lock = toggles.master && toggles.win_key_lock;
    cfg.remap.win_key_clipboard = toggles.master && toggles.win_key_clipboard;
    cfg.remap.dock_click_minimize = toggles.master && toggles.dock_click_minimize;
    cfg.substitute
        .retain(|sub| match sub.key.to_ascii_lowercase().as_str() {
            "f2" => toggles.finder_f2,
            "f5" => toggles.browser_f5,
            _ => true,
        });
    laterkey_core::compile(&cfg)
}

#[cfg(test)]
mod tests {
    use super::*;
    use laterkey_core::keys::{F2, F5};

    #[test]
    fn master_off_disables_everything() {
        let toggles = Toggles {
            master: false,
            ..Default::default()
        };
        let compiled = apply(&Config::default(), &toggles).unwrap();
        assert!(!compiled.rules.ctrl_as_cmd);
        assert!(!compiled.rules.cmd_as_ctrl);
        assert!(!compiled.rules.alt_tab);
        assert!(!compiled.rules.win_key_lock);
        assert!(!compiled.dock_click_minimize);
    }

    #[test]
    fn individual_toggles_gate_their_rules() {
        let toggles = Toggles {
            finder_f2: false,
            browser_f5: false,
            ..Default::default()
        };
        let compiled = apply(&Config::default(), &toggles).unwrap();
        assert!(compiled.rules.substitutes.is_empty());

        let toggles = Toggles {
            finder_f2: true,
            browser_f5: false,
            ..Default::default()
        };
        let compiled = apply(&Config::default(), &toggles).unwrap();
        let keys: Vec<i64> = compiled
            .rules
            .substitutes
            .iter()
            .map(|s| s.keycode)
            .collect();
        assert_eq!(keys, [F2]);
        assert!(!keys.contains(&F5));
    }

    #[test]
    fn clipboard_stays_off_by_default() {
        assert!(!Toggles::default().win_key_clipboard);
    }

    #[test]
    fn round_trip() {
        let t = Toggles {
            alt_tab: false,
            ..Default::default()
        };
        let text = toml::to_string_pretty(&t).unwrap();
        let back: Toggles = toml::from_str(&text).unwrap();
        assert_eq!(back, t);
    }
}
