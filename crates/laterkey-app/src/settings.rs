//! 设置面板（egui）。点菜单栏「设置…」打开；与托盘同属一个进程。
//!
//! 界面风格：Tokyo Night 暗色（与 LaterTerm 家族一致），卡片分组 + 动画开关。
//! 面板与托盘改的都是同一份 Toggles：改完存盘 + 经通道通知后台线程重建规则。

use eframe::egui;
use egui::{Color32, RichText, Vec2};

use crate::toggles::Toggles;

// Tokyo Night 调色板
const BG: Color32 = Color32::from_rgb(0x16, 0x16, 0x1e);
const CARD: Color32 = Color32::from_rgb(0x1f, 0x23, 0x35);
const BORDER: Color32 = Color32::from_rgb(0x29, 0x2e, 0x42);
const TEXT: Color32 = Color32::from_rgb(0xc0, 0xca, 0xf5);
const MUTED: Color32 = Color32::from_rgb(0x56, 0x5f, 0x89);
const ACCENT: Color32 = Color32::from_rgb(0x7a, 0xa2, 0xf7);
const GREEN: Color32 = Color32::from_rgb(0x9e, 0xce, 0x6a);
const ORANGE: Color32 = Color32::from_rgb(0xe0, 0xaf, 0x68);
const SWITCH_OFF: Color32 = Color32::from_rgb(0x3b, 0x42, 0x61);

/// 应用整体主题（创建时调用一次）。
pub fn apply_theme(ctx: &egui::Context) {
    ctx.set_theme(egui::ThemePreference::Dark);
    // egui 0.35 的全局样式入口：dark/light 两套一起改（我们只用 dark）
    ctx.all_styles_mut(|style| {
        let v = &mut style.visuals;
        *v = egui::Visuals::dark();
        v.window_fill = BG;
        v.panel_fill = BG;
        v.extreme_bg_color = BG;
        v.faint_bg_color = CARD;
        v.window_corner_radius = egui::CornerRadius::same(12);
        v.window_stroke = egui::Stroke::new(1.0, BORDER);
        v.widgets.noninteractive.bg_fill = CARD;
        v.widgets.noninteractive.fg_stroke = egui::Stroke::new(1.0, TEXT);
        v.widgets.inactive.bg_fill = CARD;
        v.widgets.inactive.fg_stroke = egui::Stroke::new(1.0, TEXT);
        v.widgets.inactive.corner_radius = egui::CornerRadius::same(8);
        v.widgets.hovered.bg_fill = Color32::from_rgb(0x28, 0x2e, 0x45);
        v.widgets.hovered.fg_stroke = egui::Stroke::new(1.0, Color32::WHITE);
        v.widgets.hovered.corner_radius = egui::CornerRadius::same(8);
        v.widgets.active.bg_fill = ACCENT;
        v.widgets.active.fg_stroke = egui::Stroke::new(1.0, BG);
        v.widgets.active.corner_radius = egui::CornerRadius::same(8);
        v.selection.bg_fill = ACCENT;
        v.selection.stroke = egui::Stroke::new(1.0, BG);
        style.spacing.item_spacing = Vec2::new(10.0, 8.0);
        style.spacing.button_padding = Vec2::new(14.0, 7.0);
    });
}

/// iOS 风格开关，带过渡动画。
fn switch(ui: &mut egui::Ui, on: &mut bool) -> egui::Response {
    let size = Vec2::new(40.0, 22.0);
    let (rect, mut response) = ui.allocate_exact_size(size, egui::Sense::click());
    if response.clicked() {
        *on = !*on;
        response.mark_changed();
    }
    response.widget_info(|| {
        egui::WidgetInfo::selected(egui::WidgetType::Checkbox, ui.is_enabled(), *on, "")
    });
    let t = ui.ctx().animate_bool(response.id, *on);
    if ui.is_rect_visible(rect) {
        let radius = rect.height() / 2.0;
        let mut bg = SWITCH_OFF.lerp_to_gamma(ACCENT, t);
        if response.hovered() {
            bg = bg.lerp_to_gamma(Color32::WHITE, 0.08);
        }
        ui.painter().rect_filled(rect, radius, bg);
        let x = rect.left() + radius + (rect.width() - 2.0 * radius) * t;
        ui.painter()
            .circle_filled(egui::pos2(x, rect.center().y), radius - 3.5, Color32::WHITE);
    }
    response
}

/// 卡片分组。
fn card(ui: &mut egui::Ui, title: &str, add: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::new()
        .fill(CARD)
        .stroke(egui::Stroke::new(1.0, BORDER))
        .corner_radius(egui::CornerRadius::same(10))
        .inner_margin(egui::Margin::same(14))
        .show(ui, |ui| {
            ui.label(RichText::new(title).size(12.0).color(MUTED).strong());
            ui.add_space(6.0);
            add(ui);
        });
}

/// 一行「标签 + 说明 + 右侧开关」，返回是否变动。
fn row(ui: &mut egui::Ui, label: &str, desc: &str, value: &mut bool) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.vertical(|ui| {
            ui.label(RichText::new(label).size(14.0).color(TEXT));
            if !desc.is_empty() {
                ui.label(RichText::new(desc).size(11.5).color(MUTED));
            }
        });
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            changed = switch(ui, value).changed();
        });
    });
    changed
}

fn outline_button(ui: &mut egui::Ui, label: &str) -> egui::Response {
    ui.add(egui::Button::new(
        RichText::new(label).size(13.0).color(TEXT),
    ))
}

pub struct SettingsApp {
    toggles: std::sync::Arc<std::sync::RwLock<Toggles>>,
    updates: std::sync::mpsc::Sender<(laterkey_core::Rules, laterkey_core::GroupTable)>,
    base_config: laterkey_core::Config,
}

impl SettingsApp {
    pub fn new(
        toggles: std::sync::Arc<std::sync::RwLock<Toggles>>,
        updates: std::sync::mpsc::Sender<(laterkey_core::Rules, laterkey_core::GroupTable)>,
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

        // 顶部：名称 + 总开关
        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                ui.label(RichText::new("LaterKey").size(22.0).color(TEXT).strong());
                ui.label(
                    RichText::new("让 Windows 键盘按 Windows 习惯工作")
                        .size(11.5)
                        .color(MUTED),
                );
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let mut master = t.master;
                if switch(ui, &mut master).changed() {
                    t.master = master;
                    changed = true;
                }
            });
        });
        ui.add_space(4.0);

        // 权限状态
        ui.horizontal(|ui| {
            let granted = laterkey_platform_mac::ax_trusted();
            let (dot, text) = if granted {
                (GREEN, "辅助功能已授权")
            } else {
                (ORANGE, "需要辅助功能授权")
            };
            let (rect, _) = ui.allocate_exact_size(Vec2::new(8.0, 8.0), egui::Sense::hover());
            ui.painter().circle_filled(rect.center(), 4.0, dot);
            ui.label(RichText::new(text).size(12.0).color(dot));
            if !granted {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .link(RichText::new("去授权").size(12.0).color(ACCENT))
                        .clicked()
                    {
                        crate::statusbar::open_accessibility();
                    }
                });
            }
        });
        ui.add_space(8.0);

        ui.add_enabled_ui(t.master, |ui| {
            card(ui, "按键映射", |ui| {
                changed |= row(
                    ui,
                    "Ctrl 当作 ⌘",
                    "Ctrl+C/V 复制粘贴；Ctrl+点击跳定义",
                    &mut t.ctrl_as_cmd,
                );
                changed |= row(
                    ui,
                    "Win 当作 ⌃",
                    "Win+C 即终端里的 Ctrl+C（中断）",
                    &mut t.cmd_as_ctrl,
                );
                changed |= row(
                    ui,
                    "Alt+Tab 切换应用",
                    "左 Alt+Tab，对应系统的 ⌘Tab",
                    &mut t.alt_tab,
                );
                changed |= row(
                    ui,
                    "按词跳转与删除",
                    "Ctrl+←/→ 按词移动，Ctrl+⌫ 按词删除",
                    &mut t.ctrl_arrows,
                );
                changed |= row(ui, "Win+L 锁屏", "立即锁定屏幕", &mut t.win_key_lock);
                changed |= row(
                    ui,
                    "Win+V 剪贴板历史",
                    "尚未实现，先别打开",
                    &mut t.win_key_clipboard,
                );
            });
            ui.add_space(10.0);
            card(ui, "按应用生效", |ui| {
                changed |= row(
                    ui,
                    "访达里 F2 重命名",
                    "F2 当作回车，仅访达前台时",
                    &mut t.finder_f2,
                );
                changed |= row(
                    ui,
                    "浏览器里 F5 刷新",
                    "F5 当作 ⌘R，仅浏览器前台时",
                    &mut t.browser_f5,
                );
            });
            ui.add_space(10.0);
            card(ui, "鼠标", |ui| {
                changed |= row(
                    ui,
                    "Dock 再点最小化",
                    "再点一次前台应用的 Dock 图标",
                    &mut t.dock_click_minimize,
                );
            });
        });

        ui.add_space(14.0);
        ui.horizontal(|ui| {
            if outline_button(ui, "辅助功能授权").clicked() {
                crate::statusbar::open_accessibility();
            }
            if outline_button(ui, "配置目录").clicked() {
                if let Some(path) = crate::toggles::path() {
                    if let Some(parent) = path.parent() {
                        let _ = std::fs::create_dir_all(parent);
                        let _ = std::process::Command::new("open").arg(parent).spawn();
                    }
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(
                    RichText::new(format!("v{}", env!("CARGO_PKG_VERSION")))
                        .size(11.5)
                        .color(MUTED),
                );
            });
        });

        if changed {
            *self.toggles.write().unwrap_or_else(|p| p.into_inner()) = t;
            self.apply();
        }
    }
}
