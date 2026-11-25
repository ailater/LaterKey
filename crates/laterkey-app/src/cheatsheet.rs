//! 快捷键速览浮窗：第二视口（deferred），长按 Ctrl 3 秒或托盘「快捷键速览」呼出。
//!
//! 视觉对齐 macOS 原生快捷键速览（键指如飞式）：**三栏浮动面板**——左栏当前
//! 应用（大标题 = 应用名），右两栏系统快捷键；每组一条居中标题条，下挂
//! 「组合键 + 功能名」行，键列右对齐、名称列整齐。整窗透明，面板各自半透明
//! 圆角悬浮在桌面/原应用上。
//!
//! 布局实现注意：三栏**不能用 `ui.horizontal()`**——egui 0.35 里 horizontal
//! 只分配 `interact_size.y`（18px）高，内部 `available_height()` 恒为 18，
//! 面板底与滚动区都会被压扁（此前的 Frame+set_min_height 版本也是栽在同一
//! 根因上）；改用 `allocate_ui_with_layout` 显式分配全高，再 `ui.scope`
//! 固定宽高后自绘面板底。每行快捷键的分配矩形必须是整行宽，键与名称都画
//! 在矩形内，否则换行错位。
//!
//! 其它要点：
//! - 无边框窗口在 macOS 成不了 key window——对剪贴板面板是缺陷，对速览正好：
//!   它不抢焦点、不打字，原应用保持前台；
//! - 透明窗口必须配 `with_has_shadow(false)`（egui 官方注释：避免鬼影），且
//!   `TrayApp::clear_color` 返回全透明——eframe 每帧用它清屏，不清透明的话
//!   圆角外是一块不透明底；
//! - 键名用文字（Ctrl+C）而不是 ⌘⌃ 符号：面板字体栈里键帽符号缺字形/形态
//!   与中文混排不齐（font.rs 定案），口径切换仍可看 Mac 原生命名；
//! - **关闭只隐藏不销毁视口**：反复销毁会让 glutin CGL 持有失效 view 而 panic
//!   （v0.1.5 修过的 Win+V 连按崩溃），首开后一直保留，重开时重置状态。
//!
//! 数据不在这里读：开窗时派一个后台线程走 AX（可能上百毫秒），写进 Shared
//! 快照；本视图每帧比对 version，拿到新数据就刷新。

use std::sync::{Arc, Mutex};

use eframe::egui;
use egui::{Align2, Color32, RichText, ViewportBuilder, ViewportId};
use laterkey_core::{DisplayRules, ShortcutGroup, Style};
use laterkey_platform_mac::Shared;

use crate::settings::MUTED;

/// 视口尺寸：三栏面板（应用 + 系统两栏）。
const SIZE: [f32; 2] = [1020.0, 660.0];
/// 面板圆角。
const PANEL_RADIUS: u8 = 12;
/// 标题条圆角。
const HEADER_RADIUS: u8 = 7;
/// 面板底：半透明黑（~88%），保证可读性又看得到底下在动。
const PANEL_BG: Color32 = Color32::from_rgba_unmultiplied_const(0x1a, 0x1b, 0x21, 224);
/// 组标题条底：白的高光叠层。
const HEADER_BG: Color32 = Color32::from_rgba_unmultiplied_const(255, 255, 255, 30);
/// 组合键文字（近白）。
const KEY_TEXT: Color32 = Color32::from_rgba_unmultiplied_const(0xe8, 0xea, 0xf5, 255);
/// 功能名文字（略暗一档）。
const NAME_TEXT: Color32 = Color32::from_rgba_unmultiplied_const(0xc9, 0xcd, 0xe0, 255);
/// 键列宽：键右对齐到此列，名称列起点整齐。
const KEY_COL: f32 = 158.0;
/// 行高。
const ROW_H: f32 = 21.0;
/// 面板内边距。
const PANEL_PAD: f32 = 10.0;
/// 数据还没到时按这个节奏轮询 Shared 快照。
const FETCH_POLL: f32 = 0.1;

fn viewport_id() -> ViewportId {
    ViewportId(egui::Id::new("cheatsheet_peek"))
}

#[derive(Default)]
struct CheatState {
    /// 显示口径（本会话内切换）
    style: Style,
    /// 回调请求关闭（手势松开 / × 按钮）
    close: bool,
    /// 手势开的窗才随松开自动关；托盘菜单开的不受 Ctrl 松开影响
    auto_close: bool,
    /// 开窗时刻的松开计数；变了就自动关
    release_seen: u64,
    /// 开窗时刻的数据版本：出现更新的版本（本次 fetch 完成）才算加载结束。
    /// 不能拿「app_groups 为空」当加载中——菜单读不到的应用会永远转圈
    open_version: u64,
    /// 已渲染的数据版本（Shared.cheatsheet_version）
    data_seen: u64,
    data: Arc<laterkey_platform_mac::CheatSheet>,
}

/// 浮窗句柄。首次打开后 TrayApp 一直持有，每帧调一次 show；关闭只隐藏。
#[derive(Clone)]
pub struct CheatSheetView {
    inner: Arc<Inner>,
}

struct Inner {
    state: Mutex<CheatState>,
    shared: Arc<Shared>,
}

impl CheatSheetView {
    pub fn new(shared: Arc<Shared>) -> Self {
        let version = shared
            .cheatsheet_version
            .load(std::sync::atomic::Ordering::Relaxed);
        Self {
            inner: Arc::new(Inner {
                state: Mutex::new(CheatState {
                    data: shared.cheatsheet(),
                    data_seen: version,
                    ..Default::default()
                }),
                shared,
            }),
        }
    }

    /// 每次打开都是新会话：口径回物理键、清除关闭标记；`gesture` 标记本次
    /// 是长按手势开的（松开自动关），托盘菜单开的不随 Ctrl 松开关窗。
    pub fn prepare_open(&self, gesture: bool, release_now: u64) {
        let mut st = self.lock();
        let version = self
            .inner
            .shared
            .cheatsheet_version
            .load(std::sync::atomic::Ordering::Relaxed);
        st.style = Style::Physical;
        st.close = false;
        st.auto_close = gesture;
        st.release_seen = release_now;
        st.open_version = version;
        st.data_seen = version;
    }

    /// 每帧注册视口；关闭时继续注册并设 visible=false（复用原生视口）。
    pub fn show(&self, ctx: &egui::Context, visible: bool, display: DisplayRules) {
        let view = self.clone();
        ctx.show_viewport_deferred(
            viewport_id(),
            ViewportBuilder::default()
                .with_inner_size(SIZE)
                .with_resizable(false)
                .with_always_on_top()
                // 不抢前台焦点：原应用继续接收按键，松开 Ctrl 即回原状
                .with_active(false)
                // 无边框 + 透明：面板直接悬浮（键指如飞式）
                .with_decorations(false)
                .with_transparent(true)
                // egui 官方建议：透明浮窗关掉系统投影，避免圆角外鬼影
                .with_has_shadow(false)
                .with_visible(visible),
            move |ui, _class| view.draw(ui, display),
        );
    }

    /// 回调请求关闭：TrayApp 检查后隐藏视口。
    pub fn close_requested(&self) -> bool {
        self.lock().close
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, CheatState> {
        self.inner.state.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn draw(&self, ui: &mut egui::Ui, display: DisplayRules) {
        let shared = &self.inner.shared;
        let mut st = self.lock();

        // 手势开的窗：对应的那次按住结束（松开/作废）就自动关
        if st.auto_close
            && shared
                .cheatsheet_release
                .load(std::sync::atomic::Ordering::Relaxed)
                != st.release_seen
        {
            st.close = true;
        }
        if ui.input(|i| i.viewport().close_requested()) {
            st.close = true;
        }

        // 数据刷新：version 变了才换快照；还没读到「本次开窗之后」的新版本
        // 就按节奏等（loading 只看版本，不看 app_groups 是否为空）
        let version = shared
            .cheatsheet_version
            .load(std::sync::atomic::Ordering::Relaxed);
        if version != st.data_seen {
            st.data = shared.cheatsheet();
            st.data_seen = version;
        }
        let loading = version == st.open_version;
        if loading {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_secs_f32(FETCH_POLL));
        }

        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(Color32::TRANSPARENT))
            .show(ui, |ui| {
                // 右上角迷你控件（口径切换 + ×），悬浮在面板上方
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                    close_button(ui, &mut st);
                    ui.add_space(6.0);
                    style_switch(ui, &mut st);
                });
                ui.add_space(4.0);

                let app_groups = st.data.app_groups.clone();
                let system_groups = st.data.system_groups.clone();
                let (sys_a, sys_b) = split_groups(&system_groups);

                // 三栏浮动面板：应用 | 系统前半 | 系统后半。
                // **不能用 ui.horizontal()**：egui 0.35 里 horizontal 只分配
                // interact_size.y（18px）高，内部 available_height 恒为 18，
                // 面板底和滚动区都会被压扁（无头测试锁过这个回归）；
                // 用 allocate_ui_with_layout 显式分配全高。
                ui.spacing_mut().item_spacing.x = 12.0;
                let col_w = (ui.available_width() - 24.0) / 3.0;
                let col_h = ui.available_height();
                ui.allocate_ui_with_layout(
                    egui::vec2(ui.available_width(), col_h),
                    egui::Layout::left_to_right(egui::Align::Min),
                    |ui| {
                        if app_groups.is_empty() {
                            placeholder_panel(
                                ui,
                                col_w,
                                col_h,
                                if loading {
                                    "正在读取当前应用的菜单…"
                                } else {
                                    "读不到当前应用的菜单\n（该应用可能不暴露辅助功能树）"
                                },
                                loading,
                            );
                        } else {
                            let app_name = if st.data.app_name.is_empty() {
                                "当前应用".to_string()
                            } else {
                                st.data.app_name.clone()
                            };
                            group_panel(
                                ui,
                                col_w,
                                col_h,
                                "app",
                                &app_name,
                                &app_groups,
                                st.style,
                                &display,
                                true,
                            );
                        }
                        group_panel(
                            ui, col_w, col_h, "sys-a", "系统", &sys_a, st.style, &display, true,
                        );
                        // 第二个系统面板不重复大标题（对齐截图右栏的排版）
                        group_panel(
                            ui, col_w, col_h, "sys-b", "", &sys_b, st.style, &display, false,
                        );
                    },
                );
            });
    }
}

// ---------------------------------------------------------------- 面板

/// 把系统分组按条目数对半切开，两栏高度大致均衡；至少保证前半非空。
fn split_groups(groups: &[ShortcutGroup]) -> (Vec<ShortcutGroup>, Vec<ShortcutGroup>) {
    let total: usize = groups.iter().map(|g| g.entries.len()).sum();
    let half = total.div_ceil(2);
    let mut acc = 0usize;
    let mut first = Vec::new();
    let mut second = Vec::new();
    for group in groups {
        if acc < half || first.is_empty() {
            acc += group.entries.len();
            first.push(group.clone());
        } else {
            second.push(group.clone());
        }
    }
    (first, second)
}

/// 一栏面板：`ui.scope` 固定宽高 + 自绘半透明圆角底 + 大标题（可选）+ 分组滚动区。
#[expect(clippy::too_many_arguments)]
fn group_panel(
    ui: &mut egui::Ui,
    width: f32,
    height: f32,
    panel_key: &str,
    title: &str,
    groups: &[ShortcutGroup],
    style: Style,
    display: &DisplayRules,
    with_title: bool,
) {
    // scope/Frame 默认继承外层三栏的 horizontal 布局；面板内部必须显式竖排
    ui.scope_builder(
        egui::UiBuilder::new().layout(egui::Layout::top_down(egui::Align::Min)),
        |ui| {
            ui.set_width(width);
            ui.set_height(height);
            let rect = ui.max_rect();
            ui.painter().rect_filled(rect, PANEL_RADIUS, PANEL_BG);
            egui::Frame::new()
                .inner_margin(egui::Margin::same(PANEL_PAD as i8))
                .show(ui, |ui| {
                    if with_title && !title.is_empty() {
                        header_bar(ui, title, 26.0, 13.5);
                        ui.add_space(8.0);
                    }
                    egui::ScrollArea::vertical()
                        .id_salt(egui::Id::new(("cheat-scroll", panel_key)))
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            ui.spacing_mut().item_spacing.y = 10.0;
                            for group in groups {
                                header_bar(ui, &group.title, 24.0, 12.5);
                                ui.add_space(4.0);
                                for entry in &group.entries {
                                    let Some(key) = laterkey_core::format_chord(
                                        entry.modifiers,
                                        &entry.key,
                                        style,
                                        display,
                                    ) else {
                                        continue;
                                    };
                                    shortcut_row(ui, &key, &entry.title);
                                }
                                ui.add_space(4.0);
                            }
                        });
                });
        },
    );
}

fn placeholder_panel(ui: &mut egui::Ui, width: f32, height: f32, text: &str, loading: bool) {
    // scope/Frame 默认继承外层三栏的 horizontal 布局；面板内部必须显式竖排
    ui.scope_builder(
        egui::UiBuilder::new().layout(egui::Layout::top_down(egui::Align::Min)),
        |ui| {
            ui.set_width(width);
            ui.set_height(height);
            let rect = ui.max_rect();
            ui.painter().rect_filled(rect, PANEL_RADIUS, PANEL_BG);
            egui::Frame::new()
                .inner_margin(egui::Margin::same(PANEL_PAD as i8))
                .show(ui, |ui| {
                    ui.set_width(width - PANEL_PAD * 2.0);
                    ui.vertical_centered(|ui| {
                        ui.add_space(height * 0.35);
                        if loading {
                            ui.add(egui::Spinner::new().size(18.0));
                            ui.add_space(8.0);
                        }
                        ui.label(RichText::new(text).size(12.0).color(MUTED));
                    });
                });
        },
    );
}

/// 居中标题条：整宽圆角浅色块 + 加粗白字（截图里的「ZCode / 调度中心」样式）。
fn header_bar(ui: &mut egui::Ui, title: &str, height: f32, font: f32) {
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), height),
        egui::Sense::hover(),
    );
    ui.painter().rect_filled(rect, HEADER_RADIUS, HEADER_BG);
    ui.painter().text(
        rect.center(),
        Align2::CENTER_CENTER,
        title,
        egui::FontId::proportional(font),
        KEY_TEXT,
    );
}

/// 一行快捷键：分配矩形是**整行宽**，键右对齐到键列，名称紧随其后——
/// 名称也画在矩形内，下一行才不会和它叠在一起。
fn shortcut_row(ui: &mut egui::Ui, key: &str, name: &str) {
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), ROW_H),
        egui::Sense::hover(),
    );
    let key_x = rect.left() + KEY_COL;
    ui.painter().text(
        egui::pos2(key_x, rect.center().y),
        Align2::RIGHT_CENTER,
        key,
        egui::FontId::proportional(12.0),
        KEY_TEXT,
    );
    ui.painter().text(
        egui::pos2(key_x + 10.0, rect.center().y),
        Align2::LEFT_CENTER,
        name,
        egui::FontId::proportional(12.0),
        NAME_TEXT,
    );
}

// ---------------------------------------------------------------- 迷你控件

/// 右上角圆形 ×（无边框窗口没有原生红点，托盘开的手动关全靠它）。
fn close_button(ui: &mut egui::Ui, st: &mut CheatState) {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(22.0, 22.0), egui::Sense::click());
    if resp.clicked() {
        st.close = true;
    }
    if ui.is_rect_visible(rect) {
        let fill = if resp.hovered() {
            Color32::from_rgba_unmultiplied_const(255, 255, 255, 26)
        } else {
            Color32::TRANSPARENT
        };
        ui.painter().rect_filled(rect, 11, fill);
        ui.painter().text(
            rect.center(),
            Align2::CENTER_CENTER,
            "×",
            egui::FontId::proportional(14.0),
            if resp.hovered() { KEY_TEXT } else { MUTED },
        );
    }
}

/// 「物理键 / Mac 原生」分段切换：药丸容器里两枚按钮，选中亮品牌色。
fn style_switch(ui: &mut egui::Ui, st: &mut CheatState) {
    egui::Frame::new()
        .fill(Color32::from_rgba_unmultiplied_const(255, 255, 255, 18))
        .corner_radius(egui::CornerRadius::same(9))
        .inner_margin(egui::Margin::symmetric(3, 3))
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            for (label, value) in [("物理键", Style::Physical), ("Mac 原生", Style::Mac)] {
                let selected = st.style == value;
                let (rect, resp) =
                    ui.allocate_exact_size(egui::vec2(54.0, 18.0), egui::Sense::click());
                if resp.clicked() {
                    st.style = value;
                }
                if ui.is_rect_visible(rect) {
                    let fill = if selected {
                        Color32::from_rgba_unmultiplied_const(122, 162, 247, 80)
                    } else if resp.hovered() {
                        Color32::from_rgba_unmultiplied_const(255, 255, 255, 18)
                    } else {
                        Color32::TRANSPARENT
                    };
                    ui.painter().rect_filled(rect, 6, fill);
                    ui.painter().text(
                        rect.center(),
                        Align2::CENTER_CENTER,
                        label,
                        egui::FontId::proportional(11.0),
                        if selected { KEY_TEXT } else { MUTED },
                    );
                }
            }
        });
}

/// 开窗时在后台线程读一次数据（AX 消息可能上百毫秒，不能卡 GUI）。
pub fn fetch_in_background(shared: Arc<Shared>, pid: i32) {
    let _ = std::thread::Builder::new()
        .name("cheatsheet-fetch".into())
        .spawn(move || {
            let sheet = laterkey_platform_mac::read_cheatsheet(pid);
            shared.set_cheatsheet(Arc::new(sheet));
        });
}

// ---------------------------------------------------------------- 测试

/// 无头布局测试：不渲染像素，直接对 tessellate 出的图元做几何断言。
/// 锁屏/无 GPU 环境也能跑，替代截图自检。
#[cfg(test)]
mod headless_layout_tests {
    use super::*;
    use laterkey_core::{ShortcutEntry, ShortcutGroup, ShortcutKey};

    fn entry(title: &str) -> ShortcutEntry {
        ShortcutEntry {
            title: title.into(),
            key: ShortcutKey::Char("C".into()),
            modifiers: laterkey_core::flags::COMMAND,
        }
    }

    fn fake_sheet() -> laterkey_platform_mac::CheatSheet {
        let sys_group = |name: &str, n: usize| ShortcutGroup {
            title: name.into(),
            entries: (0..n).map(|i| entry(&format!("功能{i}"))).collect(),
        };
        laterkey_platform_mac::CheatSheet {
            app_name: "ZCode".into(),
            app_groups: vec![ShortcutGroup {
                title: "Apple".into(),
                entries: vec![entry("强制退出")],
            }],
            system_groups: vec![
                sys_group("调度中心", 8),
                sys_group("聚焦", 4),
                sys_group("窗口", 6),
            ],
        }
    }

    /// 面板底（PANEL_BG）的图元竖向必须覆盖控件行下方到窗口底部，
    /// 即三栏都要撑满全高（回归：Frame+set_min_height 撑不起高度的历史缺陷）。
    #[test]
    fn panels_fill_full_height() {
        let (tx, _rx) = std::sync::mpsc::sync_channel(1);
        let shared = Arc::new(laterkey_platform_mac::Shared::new(tx, false));
        shared.set_cheatsheet(Arc::new(fake_sheet()));
        let view = CheatSheetView::new(shared.clone());
        view.prepare_open(false, 0);

        let ctx = egui::Context::default();
        let raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(SIZE[0], SIZE[1]),
            )),
            ..Default::default()
        };
        ctx.begin_pass(raw);
        let mut root = egui::Ui::new(
            ctx.clone(),
            egui::Id::new("cheatsheet-headless-test"),
            egui::UiBuilder::new().max_rect(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(SIZE[0], SIZE[1]),
            )),
        );
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(egui::Color32::TRANSPARENT))
            .show(&mut root, |ui| {
                view.draw(ui, laterkey_core::DisplayRules::default())
            });
        let out = ctx.end_pass();
        let prims = ctx.tessellate(out.shapes, 1.0);

        // 面板底是半透明色，tessellate 后顶点色可能按预乘 alpha 落盘，两者都认
        let unmultiplied = PANEL_BG;
        let premul = |c: u8| (c as u32 * PANEL_BG.a() as u32 / 255) as u8;
        let premultiplied = egui::Color32::from_rgba_premultiplied(
            premul(PANEL_BG.r()),
            premul(PANEL_BG.g()),
            premul(PANEL_BG.b()),
            PANEL_BG.a(),
        );
        let is_panel_bg = |c: egui::Color32| c == unmultiplied || c == premultiplied;

        let mut min_y = f32::MAX;
        let mut max_y = f32::MIN;
        for p in &prims {
            if let egui::epaint::Primitive::Mesh(mesh) = &p.primitive {
                for v in &mesh.vertices {
                    if is_panel_bg(v.color) {
                        min_y = min_y.min(v.pos.y);
                        max_y = max_y.max(v.pos.y);
                    }
                }
            }
        }
        let height = max_y - min_y;
        assert!(
            max_y > 640.0,
            "面板底没到窗口底部: 图元 y 范围 {min_y:.0}..{max_y:.0}"
        );
        // 顶部控件行（口径切换 + ×）约占 30px，面板底从其下方一直铺到窗口底
        assert!(
            height > 600.0,
            "面板底竖向只覆盖 {height:.0}px（期望 ≈{}-顶部控件行）",
            { SIZE[1] }
        );
    }
}
