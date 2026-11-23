//! 设置面板（egui）。点菜单栏「设置…」打开；与托盘同属一个进程。
//!
//! 面板与托盘改的都是同一份 Toggles：改完存盘 + 经通道通知后台线程重建规则。

use std::sync::mpsc::Sender;
use std::sync::{Arc, RwLock};

use crate::toggles::Toggles;
use eframe::egui;

pub struct SettingsApp {
    toggles: Arc<RwLock<Toggles>>,
    updates: Sender<(laterkey_core::Rules, laterkey_core::GroupTable)>,
    base_config: laterkey_core::Config,
}

impl SettingsApp {
    pub fn new(
        toggles: Arc<RwLock<Toggles>>,
        updates: Sender<(laterkey_core::Rules, laterkey_core::GroupTable)>,
        base_config: laterkey_core::Config,
    ) -> Self {
        Self {
            toggles,
            updates,
            base_config,
        }
    }

    fn apply(&self) {
        let t = self
            .toggles
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .clone();
        t.save();
        match crate::toggles::apply(&self.base_config, &t) {
            Ok(compiled) => {
                let _ = self.updates.send((compiled.rules, compiled.groups));
            }
            Err(e) => eprintln!("[laterkey] 规则更新失败：{e}"),
        }
    }
}

impl eframe::App for SettingsApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let mut changed = false;
        let mut t = self
            .toggles
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .clone();

        ui.heading("LaterKey 设置");
        ui.add_space(8.0);

        changed |= ui
            .checkbox(&mut t.master, "启用 LaterKey")
            .on_hover_text("总开关：关掉后所有按键与 Dock 点击恢复原样")
            .changed();

        ui.add_enabled_ui(t.master, |ui| {
            ui.group(|ui| {
                ui.label("按键映射");
                changed |= ui
                    .checkbox(&mut t.ctrl_as_cmd, "Ctrl 当作 ⌘（复制粘贴）")
                    .changed();
                changed |= ui
                    .checkbox(&mut t.cmd_as_ctrl, "Win 当作 ⌃（终端中断）")
                    .changed();
                changed |= ui.checkbox(&mut t.alt_tab, "Alt+Tab 切换应用").changed();
                changed |= ui
                    .checkbox(&mut t.ctrl_arrows, "Ctrl+←/→/⌫ 按词跳转/删除")
                    .changed();
                changed |= ui.checkbox(&mut t.win_key_lock, "Win+L 锁屏").changed();
                changed |= ui
                    .checkbox(&mut t.win_key_clipboard, "Win+V 剪贴板历史（未实现，勿开）")
                    .changed();
            });
            ui.group(|ui| {
                ui.label("按应用生效");
                changed |= ui.checkbox(&mut t.finder_f2, "访达里 F2 重命名").changed();
                changed |= ui.checkbox(&mut t.browser_f5, "浏览器里 F5 刷新").changed();
            });
            ui.group(|ui| {
                ui.label("鼠标");
                changed |= ui
                    .checkbox(
                        &mut t.dock_click_minimize,
                        "再点前台应用的 Dock 图标 → 最小化",
                    )
                    .changed();
            });
        });

        ui.add_space(8.0);
        ui.horizontal(|ui| {
            if ui.button("打开辅助功能授权").clicked() {
                crate::statusbar::open_accessibility();
            }
            if ui.button("查看配置目录").clicked() {
                if let Some(path) = crate::toggles::path() {
                    if let Some(parent) = path.parent() {
                        // 目录还不存在（从未保存过配置）时先建，否则 open 报错
                        let _ = std::fs::create_dir_all(parent);
                        let _ = std::process::Command::new("open").arg(parent).spawn();
                    }
                }
            }
        });

        if changed {
            *self.toggles.write().unwrap_or_else(|p| p.into_inner()) = t;
            self.apply();
        }

        ui.add_space(6.0);
        ui.separator();
        ui.label(format!(
            "配置：{}",
            crate::toggles::path()
                .map(|p| p.display().to_string())
                .unwrap_or_default()
        ));
    }
}
