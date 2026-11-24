//! 设置面板（egui）。点菜单栏「设置…」打开；与托盘同属一个进程。
//!
//! 布局：左侧固定导航栏 + 右侧单页连续滚动（VS Code 设置式，同 LaterScreen
//! 配置面板），双向联动——滚动时 scroll-spy 高亮当前分区，点导航瞬时跳转。
//! 界面风格：Tokyo Night 暗色（与 LaterTerm 家族一致），卡片分组 + 动画开关。
//! 面板与托盘改的都是同一份 Toggles：改完存盘 + 经通道通知后台线程重建规则。

use eframe::egui;
use egui::{Align2, Color32, FontId, RichText, Vec2};

use crate::toggles::Toggles;

// Tokyo Night 调色板（剪贴板面板也用，见 clipboard_panel.rs）
pub(crate) const BG: Color32 = Color32::from_rgb(0x16, 0x16, 0x1e);
pub(crate) const CARD: Color32 = Color32::from_rgb(0x1f, 0x23, 0x35);
pub(crate) const BORDER: Color32 = Color32::from_rgb(0x29, 0x2e, 0x42);
pub(crate) const TEXT: Color32 = Color32::from_rgb(0xc0, 0xca, 0xf5);
pub(crate) const MUTED: Color32 = Color32::from_rgb(0x56, 0x5f, 0x89);
pub(crate) const DESC: Color32 = Color32::from_rgb(0x8a, 0x92, 0xb2);
pub(crate) const ACCENT: Color32 = Color32::from_rgb(0x7a, 0xa2, 0xf7);
pub(crate) const GREEN: Color32 = Color32::from_rgb(0x9e, 0xce, 0x6a);
pub(crate) const ORANGE: Color32 = Color32::from_rgb(0xe0, 0xaf, 0x68);
pub(crate) const SWITCH_OFF: Color32 = Color32::from_rgb(0x3b, 0x42, 0x61);

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

// ---------------------------------------------------------------- 左侧导航

/// 导航分区：右侧是连续滚动页里的分组锚点（见 ui() 的 scroll-spy），
/// 不是独立页签——切分区只是滚动位置变化，状态全都在。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum Tab {
    #[default]
    Permissions,
    Keys,
    Apps,
    Mouse,
    About,
}

impl Tab {
    /// 导航顺序即展示顺序。
    const ALL: [Self; 5] = [
        Self::Permissions,
        Self::Keys,
        Self::Apps,
        Self::Mouse,
        Self::About,
    ];

    fn label(self) -> &'static str {
        match self {
            Self::Permissions => "权限",
            Self::Keys => "按键映射",
            Self::Apps => "按应用生效",
            Self::Mouse => "鼠标",
            Self::About => "关于",
        }
    }
}

/// 滚动侦测（scroll-spy）：视口顶部下探 `lead` 像素的判定线落在哪个分区
/// 区间内，就高亮哪个分区；滚到最底时强制选中最后一节——矮分区永远碰
/// 不到判定线，不兜底的话「关于」永远高亮不了。`tops` 为各分区起点在
/// 内容坐标系里的 y（同一空间内单调递增即可）。
fn scroll_spy(tops: &[(Tab, f32)], line: f32, at_bottom: bool) -> Tab {
    let mut current = tops.first().map_or(Tab::default(), |&(tab, _)| tab);
    for &(tab, top) in tops {
        if top <= line {
            current = tab;
        }
    }
    if at_bottom {
        if let Some(&(tab, _)) = tops.last() {
            current = tab;
        }
    }
    current
}

/// 导航项：整行可点。扁平风选中态 = 中性提亮底 + 品牌色前景 + 左侧指示条，
/// 未选中悬停 = 中性灰。暗色主题下用半透明白不用品牌色 tint（压暗的蓝在
/// 暗底上发闷，色相交给文字与指示条表达）。
fn nav_item(ui: &mut egui::Ui, selected: bool, label: &str) -> egui::Response {
    let (rect, resp) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), 32.0), egui::Sense::click());
    if selected {
        ui.painter().rect_filled(
            rect,
            egui::CornerRadius::same(8),
            Color32::from_rgba_unmultiplied(255, 255, 255, 22),
        );
    } else if resp.hovered() {
        ui.painter().rect_filled(
            rect,
            egui::CornerRadius::same(8),
            Color32::from_rgb(0x28, 0x2e, 0x45),
        );
    }
    let fg = if selected { ACCENT } else { TEXT };
    if selected {
        ui.painter().rect_filled(
            egui::Rect::from_center_size(
                egui::pos2(rect.left() + 2.0, rect.center().y),
                Vec2::new(3.0, rect.height() * 0.55),
            ),
            egui::CornerRadius::same(2),
            ACCENT,
        );
    }
    ui.painter().text(
        egui::pos2(rect.left() + 14.0, rect.center().y),
        Align2::LEFT_CENTER,
        label,
        FontId::proportional(13.5),
        fg,
    );
    resp
}

// ---------------------------------------------------------------- 组件

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
    if ui.is_rect_visible(rect) {
        draw_switch(ui, response.id, rect, *on, response.hovered());
    }
    response
}

/// 在给定矩形上画开关（按下状态动画由调用方的 id 驱动）。行内嵌开关与
/// 独立开关共用这一套绘制，保证圆角/滑块比例在任何尺寸下都一致。
fn draw_switch(ui: &egui::Ui, id: egui::Id, rect: egui::Rect, on: bool, hovered: bool) {
    let t = ui.ctx().animate_bool(id, on);
    let radius = rect.height() / 2.0;
    let mut bg = SWITCH_OFF.lerp_to_gamma(ACCENT, t);
    if hovered {
        bg = bg.lerp_to_gamma(Color32::WHITE, 0.08);
    }
    ui.painter().rect_filled(rect, radius, bg);
    let x = rect.left() + radius + (rect.width() - 2.0 * radius) * t;
    ui.painter()
        .circle_filled(egui::pos2(x, rect.center().y), radius - 3.5, Color32::WHITE);
}

/// 左栏「启用」行：与导航项同高（32px）同字号（13.5），整行可点，右侧小号
/// 开关。做成与导航一致的节奏——原先「启用」12.5pt 挤在 22px 高的开关旁，
/// 与 16pt 标题、32px 导航行放一起高矮不一。
fn master_row(ui: &mut egui::Ui, on: &mut bool) -> egui::Response {
    let (rect, mut resp) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), 32.0), egui::Sense::click());
    if resp.hovered() && !*on {
        ui.painter().rect_filled(
            rect,
            egui::CornerRadius::same(8),
            Color32::from_rgb(0x28, 0x2e, 0x45),
        );
    }
    let fg = if *on { ACCENT } else { TEXT };
    if *on {
        // 开着时同样给品牌色指示条，与选中导航项呼应：「启用」也是一项状态
        ui.painter().rect_filled(
            egui::Rect::from_center_size(
                egui::pos2(rect.left() + 2.0, rect.center().y),
                Vec2::new(3.0, rect.height() * 0.55),
            ),
            egui::CornerRadius::same(2),
            ACCENT,
        );
    }
    ui.painter().text(
        egui::pos2(rect.left() + 14.0, rect.center().y),
        Align2::LEFT_CENTER,
        "启用",
        FontId::proportional(13.5),
        fg,
    );
    let sw = Vec2::new(36.0, 20.0);
    let sw_rect = egui::Rect::from_center_size(
        egui::pos2(rect.right() - 8.0 - sw.x / 2.0, rect.center().y),
        sw,
    );
    draw_switch(ui, resp.id.with("sw"), sw_rect, *on, resp.hovered());
    if resp.clicked() {
        *on = !*on;
        resp.mark_changed();
    }
    resp
}

/// 分区卡片：标题左侧一条强调色小竖条 + 圆角面板。扁平风：无描边，
/// 轮廓全靠卡片底色与窗口底的色调差。
fn card(ui: &mut egui::Ui, title: &str, add: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::new()
        .fill(CARD)
        .corner_radius(egui::CornerRadius::same(12))
        .inner_margin(egui::Margin::same(14))
        .outer_margin(egui::Margin {
            bottom: 10,
            ..Default::default()
        })
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                let (rect, _) = ui.allocate_exact_size(Vec2::new(3.0, 14.0), egui::Sense::hover());
                ui.painter()
                    .rect_filled(rect, egui::CornerRadius::same(2), ACCENT);
                ui.strong(RichText::new(title).color(TEXT).size(14.0));
            });
            ui.add_space(10.0);
            add(ui);
        });
}

/// 一行「标签 + 说明 + 右侧开关」，返回是否变动。
/// 行高固定（标题 14 + 说明 11.5 + 间距），避免右侧开关把行高压扁导致文字重叠。
fn row(ui: &mut egui::Ui, label: &str, desc: &str, value: &mut bool) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.set_min_height(42.0);
        ui.vertical(|ui| {
            ui.add_space(2.0);
            ui.label(RichText::new(label).size(14.0).color(TEXT));
            if !desc.is_empty() {
                ui.add_space(2.0);
                ui.label(RichText::new(desc).size(11.5).color(DESC));
            }
        });
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            changed = switch(ui, value).changed();
        });
    });
    ui.add_space(6.0);
    changed
}

/// 权限行：状态点 + 名称与说明（左），状态文字与「打开」按钮（右）。
/// 状态文字不放名称旁边：不同字号在同一水平行按垂直居中排，基线会一高一低。
fn perm_row(ui: &mut egui::Ui, name: &str, desc: &str, granted: bool, accessibility: bool) {
    ui.horizontal(|ui| {
        let (dot, state) = if granted {
            (GREEN, "已授权")
        } else {
            (ORANGE, "未授权")
        };
        let (rect, _) = ui.allocate_exact_size(Vec2::new(8.0, 8.0), egui::Sense::hover());
        ui.painter().circle_filled(rect.center(), 4.0, dot);
        ui.vertical(|ui| {
            ui.add_space(2.0);
            ui.label(RichText::new(name).size(13.0).color(TEXT));
            ui.add_space(2.0);
            ui.label(RichText::new(desc).size(11.0).color(DESC));
        });
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if outline_button(ui, "打开").clicked() {
                if accessibility {
                    crate::statusbar::open_accessibility();
                } else {
                    crate::statusbar::open_listen_event();
                }
            }
            ui.label(RichText::new(state).size(11.5).color(dot));
        });
    });
    ui.add_space(6.0);
}

fn outline_button(ui: &mut egui::Ui, label: &str) -> egui::Response {
    ui.add(egui::Button::new(
        RichText::new(label).size(13.0).color(TEXT),
    ))
}

// ---------------------------------------------------------------- App

pub struct SettingsApp {
    toggles: std::sync::Arc<std::sync::RwLock<Toggles>>,
    updates: std::sync::mpsc::Sender<(laterkey_core::Rules, laterkey_core::GroupTable)>,
    base_config: laterkey_core::Config,
    /// 当前导航高亮（scroll-spy 与点击共同维护）
    active_tab: Tab,
    /// 本帧点击导航要跳转的分区（消费后置空）
    jump_target: Option<Tab>,
    /// 「重新检测」按钮的最近一次结果（显示在按钮旁）
    perm_note: Option<String>,
    /// 开机自启开关的最近一次操作结果
    auto_start_note: Option<String>,
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
            active_tab: Tab::default(),
            jump_target: None,
            perm_note: None,
            auto_start_note: None,
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
        // 闭包外取状态，两个 Panel 的闭包各自顺序借用，不重叠
        let mut jump = self.jump_target.take();
        let mut active_tab = self.active_tab;
        let mut toggles = self
            .toggles
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .clone();
        let mut changed = false;
        let mut perm_note = self.perm_note.clone();
        let mut auto_start_note = self.auto_start_note.clone();

        // 左侧导航：只做「当前分区」指示 + 点击跳转（scroll-spy 联动见下方）。
        // 与窗口底同色（融入页框），唯一的色块是右侧内容卡片。
        egui::Panel::left(egui::Id::new("settings-nav"))
            .exact_size(150.0)
            .resizable(false)
            .frame(
                egui::Frame::new()
                    .fill(BG)
                    .inner_margin(egui::Margin::same(10)),
            )
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 4.0;
                ui.add_space(4.0);
                ui.label(RichText::new("LaterKey").size(15.0).color(TEXT).strong());
                ui.label(RichText::new("Windows 键盘习惯").size(11.0).color(MUTED));
                ui.add_space(10.0);
                let mut master = toggles.master;
                if master_row(ui, &mut master).changed() {
                    toggles.master = master;
                    changed = true;
                }
                for tab in Tab::ALL {
                    if nav_item(ui, active_tab == tab, tab.label()).clicked() {
                        jump = Some(tab);
                    }
                }
            });

        // 右侧内容：全部分区连续排布在同一滚动页里，左栏与滚动位置双向联动：
        // 滚动 → scroll-spy 高亮；点击 → 瞬时跳转到分区起点
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(BG).inner_margin(egui::Margin {
                left: 4,
                right: 14,
                top: 10,
                bottom: 4,
            }))
            .show(ui, |ui| {
                // 本帧点击导航 → 立即跳转并跳过本帧滚动侦测：pass 内的
                // cursor 位置还是旧 offset，侦测会把高亮拉回旧分区
                let jumping = jump.take();
                let mut tops: Vec<(Tab, f32)> = Vec::with_capacity(Tab::ALL.len());
                let scroll_out = egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        // pass 内 cursor 随 offset 平移，减去首分区起点归一化成
                        // 与 state.offset 同一空间的内容坐标，scroll-spy 才可比
                        let base = ui.cursor().top();
                        for (index, tab) in Tab::ALL.iter().enumerate() {
                            if index > 0 {
                                ui.add_space(6.0);
                            }
                            if jumping == Some(*tab) {
                                // 瞬时跳转（不用平滑动画）：动画期间侦测会沿途
                                // 经高亮中间分区，滚轮输入也会与动画目标拉扯
                                ui.scroll_to_cursor_animation(
                                    Some(egui::Align::Min),
                                    egui::style::ScrollAnimation::none(),
                                );
                            }
                            tops.push((*tab, ui.cursor().top() - base));
                            match tab {
                                Tab::Permissions => {
                                    permissions_section(ui, &mut toggles, &mut perm_note);
                                }
                                Tab::Keys => keys_section(ui, &mut toggles, &mut changed),
                                Tab::Apps => apps_section(ui, &mut toggles, &mut changed),
                                Tab::Mouse => mouse_section(ui, &mut toggles, &mut changed),
                                Tab::About => about_section(ui, &mut auto_start_note),
                            }
                        }
                    });
                if jumping.is_none()
                    && scroll_out.content_size.y > scroll_out.inner_rect.height() + 2.0
                {
                    // 判定线：视口顶部下探 80px；滚到底兜底选最后一节（矮分区
                    // 碰不到判定线）。内容不超高时跳过侦测：at_bottom 恒真会
                    // 把高亮错拉到末节
                    let line = scroll_out.state.offset.y + 80.0;
                    let at_bottom = scroll_out.state.offset.y
                        >= scroll_out.content_size.y - scroll_out.inner_rect.height() - 2.0;
                    active_tab = scroll_spy(&tops, line, at_bottom);
                }
            });

        // 闭包后写回状态
        self.jump_target = jump;
        self.active_tab = active_tab;
        self.perm_note = perm_note;
        self.auto_start_note = auto_start_note;
        if changed {
            *self.toggles.write().unwrap_or_else(|p| p.into_inner()) = toggles;
            self.apply();
        }
    }
}

// ---------------------------------------------------------------- 分区

fn permissions_section(ui: &mut egui::Ui, _t: &mut Toggles, perm_note: &mut Option<String>) {
    let ax = laterkey_platform_mac::ax_trusted();
    let listen = laterkey_platform_mac::listen_allowed();
    card(ui, "权限", |ui| {
        perm_row(ui, "辅助功能", "主动拦截、Dock、最小化都靠它", ax, true);
        perm_row(
            ui,
            "监听事件 ListenEvent",
            "仅诊断需要，主动拦截不依赖它",
            listen,
            false,
        );
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            if outline_button(ui, "重新检测").clicked() {
                *perm_note = Some(format!(
                    "辅助功能 {}　监听事件 {}",
                    if laterkey_platform_mac::ax_trusted() {
                        "√ 已授权"
                    } else {
                        "× 未授权"
                    },
                    if laterkey_platform_mac::listen_allowed() {
                        "√ 已授权"
                    } else {
                        "× 未授权"
                    },
                ));
            }
        });
        if let Some(note) = perm_note {
            ui.add_space(2.0);
            ui.label(RichText::new(note.as_str()).size(11.5).color(DESC));
        }
    });
}

fn keys_section(ui: &mut egui::Ui, t: &mut Toggles, changed: &mut bool) {
    ui.add_enabled_ui(t.master, |ui| {
        card(ui, "按键映射", |ui| {
            *changed |= row(
                ui,
                "Ctrl 当作 Command",
                "Ctrl+C/V 复制粘贴；Ctrl+点击跳定义",
                &mut t.ctrl_as_cmd,
            );
            *changed |= row(
                ui,
                "Win 当作 Ctrl",
                "Win+C 即终端里的 Ctrl+C（中断）",
                &mut t.cmd_as_ctrl,
            );
            *changed |= row(
                ui,
                "Alt+Tab 切换应用",
                "左 Alt+Tab，对应系统的 Command+Tab",
                &mut t.alt_tab,
            );
            *changed |= row(
                ui,
                "按词跳转与删除",
                "Ctrl+←/→ 按词移动，Ctrl+退格 按词删除",
                &mut t.ctrl_arrows,
            );
            *changed |= row(ui, "Win+L 锁屏", "立即锁定屏幕", &mut t.win_key_lock);
            *changed |= row(
                ui,
                "Win+V 剪贴板历史",
                "Win+V 弹出历史面板，文本图片都收，选中即贴回（Command+V 粘贴不受影响）",
                &mut t.win_key_clipboard,
            );
            *changed |= row(
                ui,
                "任务管理器",
                "Ctrl+Alt+Del 或 Ctrl+Esc 呼出强制退出",
                &mut t.task_manager,
            );
        });
    });
}

fn apps_section(ui: &mut egui::Ui, t: &mut Toggles, changed: &mut bool) {
    ui.add_enabled_ui(t.master, |ui| {
        card(ui, "按应用生效", |ui| {
            *changed |= row(
                ui,
                "访达里 F2 重命名",
                "F2 当作回车，仅访达前台时",
                &mut t.finder_f2,
            );
            *changed |= row(
                ui,
                "浏览器里 F5 刷新",
                "F5 当作 Command+R，仅浏览器前台时",
                &mut t.browser_f5,
            );
        });
    });
}

fn mouse_section(ui: &mut egui::Ui, t: &mut Toggles, changed: &mut bool) {
    ui.add_enabled_ui(t.master, |ui| {
        card(ui, "鼠标", |ui| {
            *changed |= row(
                ui,
                "Dock 再点最小化",
                "再点一次前台应用的 Dock 图标",
                &mut t.dock_click_minimize,
            );
        });
    });
}

fn about_section(ui: &mut egui::Ui, auto_start_note: &mut Option<String>) {
    // 开机自启不是按键规则，不进 Toggles——状态直接读 LaunchAgent plist。
    // 只写 plist 不立即 bootstrap：当前 GUI 持有实例锁，拉起第二个实例会
    // 抢锁失败并被 KeepAlive 反复重启；下次登录自然生效。
    card(ui, "开机自启", |ui| {
        let mut on = crate::daemon::auto_start_enabled();
        if row(
            ui,
            "登录时自动启动",
            "开机后自动驻留菜单栏。关闭时若当前实例由 LaunchAgent 托管，它会一同退出",
            &mut on,
        ) {
            let result = if on {
                crate::daemon::enable_auto_start()
            } else {
                crate::daemon::disable_auto_start()
            };
            *auto_start_note = Some(match result {
                Ok(msg) => msg,
                Err(err) => format!("设置失败：{err}"),
            });
        }
        if let Some(note) = auto_start_note {
            ui.add_space(2.0);
            ui.label(RichText::new(note.as_str()).size(11.0).color(DESC));
        }
    });

    card(ui, "关于", |ui| {
        ui.vertical(|ui| {
            ui.label(RichText::new("LaterKey").size(15.0).color(TEXT).strong());
            ui.label(
                RichText::new("让 Windows 布局键盘按 Windows 习惯工作")
                    .size(11.5)
                    .color(DESC),
            );
            ui.add_space(4.0);
            ui.label(
                RichText::new(format!("版本 v{}", env!("CARGO_PKG_VERSION")))
                    .size(11.5)
                    .color(MUTED),
            );
            ui.add_space(6.0);
            if outline_button(ui, "打开配置目录").clicked() {
                if let Some(path) = crate::toggles::path() {
                    if let Some(parent) = path.parent() {
                        let _ = std::fs::create_dir_all(parent);
                        let _ = std::process::Command::new("open").arg(parent).spawn();
                    }
                }
            }
        });
    });
}
