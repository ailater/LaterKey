//! 快捷键速览浮窗：第二视口（deferred），长按 Ctrl 2 秒或托盘「快捷键速览」呼出。
//!
//! 视觉对齐 macOS 快捷键速览工具（键指如飞/CheatSheet 式）：**一整块致密
//! 的深色浮层**——应用名大标题 + 应用菜单组占第一列，系统快捷键组依次流式
//! 排满后续列；每组一条居中圆角标题条，下挂「键帽符号 + 功能名」行（键右
//! 对齐、名称超宽截断加 …），行距紧凑。整窗透明，浮层底半透明圆角。
//!
//! 布局实现注意：
//! - 横向多列**不能用 `ui.horizontal()`**——egui 0.35 里 horizontal 只分配
//!   `interact_size.y`（18px）高，内部 `available_height()` 恒为 18，面板底
//!   与内容全被压扁（此前的 Frame+set_min_height 版本栽在同一根因）；改用
//!   `allocate_ui_with_layout` 显式分配全高；
//! - 应用区和系统区分别管理滚动与分栏：应用菜单不会挤进系统区；系统组
//!   标题横跨右侧两列，组内条目再左右分配；
//! - 每行的分配矩形必须是整行宽，键与名称都画在矩形内，否则换行错位。
//!
//! 其它要点：
//! - 无边框窗口在 macOS 成不了 key window——对剪贴板面板是缺陷，对速览正好：
//!   它不抢焦点、不打字，原应用保持前台；
//! - 透明窗口必须配 `with_has_shadow(false)`（egui 官方注释：避免鬼影），且
//!   `TrayApp::clear_color` 返回全透明——eframe 每帧用它清屏，不清透明的话
//!   圆角外是一块不透明底；
//! - 键名 Mac 口径用键帽符号（⌘⌃⇧⌥）：SFNS.ttf 实测有全部字形，font.rs 的
//!   system-cjk → system-symbols 兜底链负责渲染；物理键口径用 Ctrl/Win/Alt
//!   文字（见 core `format_chord`）；
//! - **关闭只隐藏不销毁视口**：反复销毁会让 glutin CGL 持有失效 view 而 panic
//!   （v0.1.5 修过的 Win+V 连按崩溃），首开后一直保留，重开时重置状态。
//!
//! 数据不在这里读：开窗时派一个后台线程走 AX（可能上百毫秒），写进 Shared
//! 快照；本视图每帧比对 version，拿到新数据就刷新。

use std::sync::{Arc, Mutex};

use eframe::egui;
use egui::{Align2, Color32, FontId, Key, ViewportBuilder, ViewportId};
use laterkey_core::{CheatsheetConfig, DisplayRules, ShortcutGroup, ShortcutKey, Style};
use laterkey_platform_mac::Shared;

use crate::settings::MUTED;

/// 视口尺寸：按参考图的宽高比例与密度设定。
const SIZE: [f32; 2] = [1024.0, 850.0];
/// 浮层圆角。
const PANEL_RADIUS: u8 = 12;
/// 标题条圆角。
const HEADER_RADIUS: u8 = 6;
/// 浮层底：半透明黑（~88%），保证可读性又看得到底下在动。
const PANEL_BG: Color32 = Color32::from_rgba_unmultiplied_const(0x1a, 0x1b, 0x21, 245);
/// 组标题条底：白的高光叠层。
const HEADER_BG: Color32 = Color32::from_rgba_unmultiplied_const(255, 255, 255, 30);
/// 组合键文字（近白）。
const KEY_TEXT: Color32 = Color32::from_rgba_unmultiplied_const(0xe8, 0xea, 0xf5, 255);
/// 键列强调色：让键帽符号从功能名称中脱出来，但不喧宾夺主。
const KEY_ACCENT: Color32 = Color32::from_rgba_unmultiplied_const(0xb8, 0xc9, 0xff, 255);
/// 功能名文字（略暗一档）。
const NAME_TEXT: Color32 = Color32::from_rgba_unmultiplied_const(0xc9, 0xcd, 0xe0, 255);
/// 标题文字，比普通行更亮一档。
const HEADER_TEXT: Color32 = Color32::from_rgba_unmultiplied_const(0xf1, 0xf3, 0xff, 255);
/// 键列宽：键右对齐到此列，名称列起点整齐。
const KEY_COL: f32 = 136.0;
/// 键与功能名之间的留白。
const KEY_NAME_GAP: f32 = 10.0;
/// 行高（紧凑，对齐参考工具的密度）。
const ROW_H: f32 = 19.0;
/// 组标题条高。
const HEADER_H: f32 = 22.0;
/// 应用名大标题条高。
const APP_TITLE_H: f32 = 26.0;
/// 组与组之间的空隙。
const GROUP_GAP: f32 = 9.0;
/// 系统组内部两列之间的空隙。
const COL_GAP: f32 = 28.0;
/// 应用区与系统总区之间的宽留白（参考图的主分区间隔）。
const SECTION_GAP: f32 = 90.0;
/// 浮层内边距。
const OVERLAY_PAD: f32 = 30.0;
/// 数据还没到时按这个节奏轮询 Shared 快照。
const FETCH_POLL: f32 = 0.1;

fn viewport_id() -> ViewportId {
    ViewportId(egui::Id::new("cheatsheet_peek"))
}

#[derive(Default)]
struct CheatState {
    /// 视觉字号配置（来自 config.toml 的 [cheatsheet]）。
    visual: CheatsheetConfig,
    /// 浮窗曾经获得过焦点；用于识别点击外部后的失焦关闭。
    was_focused: bool,
    /// 点击快捷键后关闭并执行
    target_pid: i32,
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
    pub fn new(shared: Arc<Shared>, visual: CheatsheetConfig) -> Self {
        let version = shared
            .cheatsheet_version
            .load(std::sync::atomic::Ordering::Relaxed);
        Self {
            inner: Arc::new(Inner {
                state: Mutex::new(CheatState {
                    visual,
                    target_pid: 0,
                    data: shared.cheatsheet(),
                    data_seen: version,
                    ..Default::default()
                }),
                shared,
            }),
        }
    }

    /// 每次打开都是新会话：口径回 Mac 原生（键帽符号，对齐参考工具）、清除
    /// 关闭标记；`gesture` 标记本次是长按手势开的（松开自动关），托盘菜单开
    /// 的不随 Ctrl 松开关窗。
    pub fn prepare_open(&self, target_pid: i32, release_now: u64) {
        let mut st = self.lock();
        let version = self
            .inner
            .shared
            .cheatsheet_version
            .load(std::sync::atomic::Ordering::Relaxed);
        st.style = Style::Mac;
        st.target_pid = target_pid;
        st.close = false;
        st.auto_close = false;
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
                // 需要接收点击、Esc 和失焦事件：弹窗不再依赖 Ctrl 松开关闭。
                .with_active(true)
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

        if ui.input(|i| i.viewport().close_requested()) || ui.input(|i| i.key_pressed(Key::Escape))
        {
            st.close = true;
        }
        let focused = ui.input(|i| i.viewport().focused);
        if focused == Some(true) {
            st.was_focused = true;
        } else if focused == Some(false) && st.was_focused {
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
                // 浮层底先画（铺满整窗），内容全部叠在其上
                let outer = ui.available_rect_before_wrap();
                ui.painter().rect_filled(outer, PANEL_RADIUS, PANEL_BG);

                // 右上角迷你控件（口径切换 + ×），悬浮在浮层右上角
                let mut controls = ui.new_child(
                    egui::UiBuilder::new()
                        .max_rect(outer)
                        .layout(egui::Layout::right_to_left(egui::Align::Min)),
                );
                controls.add_space(OVERLAY_PAD);
                close_button(&mut controls, &mut st);
                controls.add_space(6.0);
                style_switch(&mut controls, &mut st);

                // 内容区：浮层内边距 + 顶部给控件让位
                egui::Frame::new()
                    .inner_margin(egui::Margin {
                        left: OVERLAY_PAD as i8,
                        right: OVERLAY_PAD as i8,
                        top: 16,
                        bottom: OVERLAY_PAD as i8,
                    })
                    .show(ui, |ui| {
                        draw_columns(ui, &mut st, display, loading);
                    });
            });
    }
}

// ---------------------------------------------------------------- 排版模型

/// 一个待排版的组：标题条 + 已换算的（键文本, 功能名）行。
#[derive(Clone, Debug, PartialEq)]
struct ShortcutRow {
    display_key: String,
    name: String,
    key: ShortcutKey,
    modifiers: u64,
}

#[derive(Clone, Debug, PartialEq)]
enum Block {
    Group {
        title: String,
        rows: Vec<ShortcutRow>,
    },
}

// ---------------------------------------------------------------- 绘制

fn draw_columns(ui: &mut egui::Ui, st: &mut CheatState, display: DisplayRules, loading: bool) {
    let mut app_blocks = section_blocks(&st.data.app_groups, st.style, &display);
    let system_blocks = section_blocks(&st.data.system_groups, st.style, &display);

    if loading {
        app_blocks.insert(
            0,
            Block::Group {
                title: "正在读取当前应用的菜单…".to_string(),
                rows: Vec::new(),
            },
        );
    } else if app_blocks.is_empty() {
        app_blocks.push(Block::Group {
            title: "读不到当前应用的菜单".to_string(),
            rows: vec![ShortcutRow {
                display_key: String::new(),
                name: "该应用可能不暴露辅助功能树".into(),
                key: ShortcutKey::Char(String::new()),
                modifiers: 0,
            }],
        });
    }

    let app_title = if st.data.app_name.is_empty() {
        "当前应用".to_string()
    } else {
        st.data.app_name.clone()
    };
    let inner_w = ui.available_width();
    let col_h = ui.available_height();
    let app_w = (inner_w * 0.25).min(231.0);
    let system_w = inner_w - app_w - SECTION_GAP;
    // 参考图右侧不是等宽三栏：左系统列更宽，右侧只承载少量快捷键。
    let system_col_w = (system_w * 0.66).min(430.0);

    // 参考图的骨架固定为「应用区 | 系统总区」。应用区内部允许纵向滚动，
    // 绝不因为菜单很多而把应用内容挤到系统区；系统组标题横跨右侧两列，
    // 每组的条目再在组内两列流式排列。
    egui::ScrollArea::horizontal()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.allocate_ui_with_layout(
                egui::vec2(inner_w, col_h),
                egui::Layout::left_to_right(egui::Align::Min),
                |ui| {
                    ui.allocate_ui_with_layout(
                        egui::vec2(app_w, col_h),
                        egui::Layout::top_down(egui::Align::Min),
                        |ui| {
                            ui.set_width(app_w);
                            ui.set_height(col_h);
                            header_bar(ui, &app_title, APP_TITLE_H, st.visual.title_size, app_w);
                            ui.add_space(GROUP_GAP);
                            egui::ScrollArea::vertical()
                                .id_salt("cheatsheet-app-scroll")
                                .auto_shrink([false, false])
                                .show(ui, |ui| {
                                    ui.set_width(app_w);
                                    ui.spacing_mut().item_spacing.y = 0.0;
                                    draw_block_list(ui, &app_blocks, app_w, st);
                                });
                        },
                    );
                    ui.add_space(SECTION_GAP);
                    ui.allocate_ui_with_layout(
                        egui::vec2(system_w, col_h),
                        egui::Layout::top_down(egui::Align::Min),
                        |ui| {
                            ui.set_width(system_w);
                            ui.set_height(col_h);
                            header_bar(ui, "系统", HEADER_H, st.visual.title_size, system_w);
                            ui.add_space(GROUP_GAP);
                            egui::ScrollArea::vertical()
                                .id_salt("cheatsheet-system-scroll")
                                .auto_shrink([false, false])
                                .show(ui, |ui| {
                                    ui.set_width(system_w);
                                    ui.spacing_mut().item_spacing.y = 0.0;
                                    draw_system_groups(
                                        ui,
                                        &system_blocks,
                                        system_w,
                                        system_col_w,
                                        st,
                                    );
                                });
                        },
                    );
                },
            );
        });
}

fn draw_block_list(ui: &mut egui::Ui, blocks: &[Block], col_w: f32, st: &mut CheatState) {
    for (index, block) in blocks.iter().enumerate() {
        if index > 0 {
            ui.add_space(GROUP_GAP);
        }
        draw_block(ui, block, col_w, st);
    }
}

/// 系统区：标题条横跨两列，组内快捷键才分到左右两列，保持参考图的层级。
fn draw_system_groups(
    ui: &mut egui::Ui,
    groups: &[Block],
    system_w: f32,
    left_w: f32,
    st: &mut CheatState,
) {
    let right_w = (system_w - left_w - COL_GAP).max(120.0);
    for (index, block) in groups.iter().enumerate() {
        if index > 0 {
            ui.add_space(GROUP_GAP);
        }
        let Block::Group { title, rows } = block;
        header_bar(ui, title, HEADER_H, st.visual.group_size, system_w);
        let split = rows.len().div_ceil(2);
        let left = &rows[..split];
        let right = &rows[split..];
        let row_count = left.len().max(right.len());
        ui.allocate_ui_with_layout(
            egui::vec2(system_w, row_count as f32 * ROW_H),
            egui::Layout::left_to_right(egui::Align::Min),
            |ui| {
                draw_row_column(ui, left, left_w, st);
                ui.add_space(COL_GAP);
                draw_row_column(ui, right, right_w, st);
            },
        );
    }
}

fn draw_row_column(ui: &mut egui::Ui, rows: &[ShortcutRow], col_w: f32, st: &mut CheatState) {
    ui.allocate_ui_with_layout(
        egui::vec2(col_w, rows.len() as f32 * ROW_H),
        egui::Layout::top_down(egui::Align::Min),
        |ui| {
            ui.set_width(col_w);
            ui.spacing_mut().item_spacing.y = 0.0;
            for row in rows {
                shortcut_row(ui, row, st);
            }
        },
    );
}

fn section_blocks(groups: &[ShortcutGroup], style: Style, display: &DisplayRules) -> Vec<Block> {
    groups
        .iter()
        .map(|group| Block::Group {
            title: group.title.clone(),
            rows: group
                .entries
                .iter()
                .filter_map(|entry| {
                    laterkey_core::format_chord(entry.modifiers, &entry.key, style, display).map(
                        |key| ShortcutRow {
                            display_key: key,
                            name: entry.title.clone(),
                            key: entry.key.clone(),
                            modifiers: entry.modifiers,
                        },
                    )
                })
                .collect(),
        })
        .collect()
}

fn draw_block(ui: &mut egui::Ui, block: &Block, col_w: f32, st: &mut CheatState) {
    match block {
        Block::Group { title, rows } => {
            header_bar(ui, title, HEADER_H, st.visual.group_size, col_w);
            for row in rows {
                shortcut_row(ui, row, st);
            }
        }
    }
}

/// 居中标题条：整宽圆角浅色块 + 白字，超宽截断（截图里的「ZCode / 调度中心」样式）。
fn header_bar(ui: &mut egui::Ui, title: &str, height: f32, font: f32, col_w: f32) {
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), height),
        egui::Sense::hover(),
    );
    ui.painter().rect_filled(rect, HEADER_RADIUS, HEADER_BG);
    let text = truncate_to_width(ui, title, FontId::proportional(font), col_w - 20.0);
    ui.painter().text(
        rect.center(),
        Align2::CENTER_CENTER,
        text,
        FontId::proportional(font),
        HEADER_TEXT,
    );
}

/// 一行快捷键：分配矩形是**整行宽**，键右对齐到键列，名称紧随其后——
/// 名称也画在矩形内（超宽截断加 …），下一行才不会和它叠在一起。
fn shortcut_row(ui: &mut egui::Ui, row: &ShortcutRow, st: &mut CheatState) {
    let key = &row.display_key;
    let name = &row.name;
    let (rect, resp) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), ROW_H),
        egui::Sense::hover(),
    );
    if resp.hovered() {
        ui.painter().rect_filled(
            rect,
            4.0,
            Color32::from_rgba_unmultiplied_const(122, 162, 247, 14),
        );
    }
    let key_col = KEY_COL.min((rect.width() * 0.52).max(72.0));
    let key_x = rect.left() + key_col;
    draw_key_text(
        ui,
        egui::pos2(key_x, rect.center().y),
        Align2::RIGHT_CENTER,
        key,
        st.visual.key_size,
    );
    let name_x = key_x + KEY_NAME_GAP;
    let max_w = (rect.right() - name_x).max(20.0);
    let name = truncate_to_width(ui, name, FontId::proportional(st.visual.name_size), max_w);
    ui.painter().text(
        egui::pos2(name_x, rect.center().y),
        Align2::LEFT_CENTER,
        name,
        FontId::proportional(st.visual.name_size),
        NAME_TEXT,
    );
    if resp.clicked() {
        let chord = laterkey_core::Chord {
            keycode: match &row.key {
                ShortcutKey::VirtualKey(code) => *code,
                ShortcutKey::Char(ch) => laterkey_core::keys::named(ch).unwrap_or(-1),
            },
            flags: row.modifiers,
        };
        if chord.keycode >= 0 {
            let _ = laterkey_platform_mac::trigger_chord(st.target_pid, chord);
            st.close = true;
        }
    }
}
/// 一段连续同脚本的文本（CJK 或非 CJK）。
fn is_cjk(c: char) -> bool {
    let cp = c as u32;
    (0x2E80..=0x9FFF).contains(&cp)      // CJK 部首扩展 → 统一表意（含 CJK 标点）
        || (0xF900..=0xFAFF).contains(&cp) // 兼容表意
        || (0xFF00..=0xFFEF).contains(&cp) // 全角
        || (0x20000..=0x2FA1F).contains(&cp)
}

/// 把文本按脚本拆成 run（CJK / 非 CJK 交替）。
fn split_script_runs(text: &str) -> Vec<(bool, String)> {
    let mut runs: Vec<(bool, String)> = Vec::new();
    for c in text.chars() {
        let cjk = is_cjk(c);
        match runs.last_mut() {
            Some((flag, s)) if *flag == cjk => s.push(c),
            _ => runs.push((cjk, c.to_string())),
        }
    }
    runs
}

/// 键名绘制：非 CJK 段（⌘⌃⇧⌥、字母、数字、箭头）用键帽字体族（SF 独占），
/// CJK 段（空格/回车/退格等键名）用正文字体，**拆成独立的 galley 逐段
/// 画、各自垂直居中**。egui 0.35 同一行混字体时逐字形按各自字体度量取整
/// 定位（text_layout.rs:978-987），SF 与冬青黑的取整方向不同——直接一个
/// 字符串排出来就高低不齐（2026-11-26 用户反馈「老毛病又犯了」）。
/// 返回总宽（右对齐定位用）。
fn draw_key_text(ui: &egui::Ui, anchor: egui::Pos2, align: Align2, text: &str, size: f32) -> f32 {
    let galley_for = |cjk: bool, s: String, color: Color32| {
        let font = if cjk {
            FontId::proportional(size)
        } else {
            crate::font::keycaps_font(size)
        };
        ui.painter().layout_no_wrap(s, font, color)
    };
    let runs = split_script_runs(text);
    let mut galleys: Vec<_> = runs
        .iter()
        .map(|&(cjk, ref s)| galley_for(cjk, s.clone(), KEY_ACCENT))
        .collect();
    // 末尾若是省略号段（…），颜色跟正文一档（理论上键名不截断，防御）
    let total_w: f32 = galleys.iter().map(|g| g.size().x).sum();
    let mut cursor_x = match align {
        Align2::RIGHT_CENTER | Align2::RIGHT_TOP | Align2::RIGHT_BOTTOM => anchor.x - total_w,
        Align2::CENTER_CENTER => anchor.x - total_w / 2.0,
        _ => anchor.x,
    };
    for g in &galleys {
        let pos = egui::pos2(cursor_x, anchor.y - g.size().y / 2.0);
        ui.painter().galley(pos, g.clone(), KEY_TEXT);
        cursor_x += g.size().x;
    }
    debug_assert!(!galleys.is_empty());
    let _ = &mut galleys; // keep clippy quiet about unused mut in release
    total_w
}

/// 文本超宽时逐字截断加 …（行短、频率高，简单实现就够）。
fn truncate_to_width(ui: &egui::Ui, text: &str, font: FontId, max_w: f32) -> String {
    let measure = |s: &str| {
        ui.painter()
            .layout_no_wrap(s.to_owned(), font.clone(), NAME_TEXT)
            .size()
            .x
    };
    if measure(text) <= max_w {
        return text.to_string();
    }
    let mut chars: Vec<char> = text.chars().collect();
    while chars.len() > 1 {
        chars.pop();
        let mut candidate = chars.clone();
        candidate.push('…');
        let s: String = candidate.into_iter().collect();
        if measure(&s) <= max_w {
            return s;
        }
    }
    "…".to_string()
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
            FontId::proportional(14.0),
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
                        FontId::proportional(11.0),
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
    use laterkey_core::{ShortcutEntry, ShortcutKey};

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
            app_groups: vec![
                ShortcutGroup {
                    title: "Apple".into(),
                    entries: vec![entry("强制退出"), entry("退出登录"), entry("锁屏")],
                },
                ShortcutGroup {
                    title: "文件".into(),
                    entries: (0..30).map(|i| entry(&format!("条目{i}"))).collect(),
                },
            ],
            system_groups: vec![
                sys_group("调度中心", 8),
                sys_group("聚焦", 4),
                sys_group("窗口", 6),
            ],
        }
    }

    fn tessellate(view: &CheatSheetView) -> Vec<egui::epaint::ClippedPrimitive> {
        let ctx = egui::Context::default();
        // 键帽字体族由 font::setup 注册（真实应用启动即装）；不装的话
        // draw_key_text 会对未注册的族 panic
        crate::font::setup(&ctx);
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
        ctx.tessellate(out.shapes, 1.0)
    }

    /// 面板底（PANEL_BG）图元必须铺满整窗（回归：ui.horizontal 18px 陷阱、
    /// Frame+set_min_height 旧缺陷——浮层只画到内容高度）。
    #[test]
    fn overlay_fills_full_window() {
        let (tx, _rx) = std::sync::mpsc::sync_channel(1);
        let shared = Arc::new(laterkey_platform_mac::Shared::new(tx, false));
        shared.set_cheatsheet(Arc::new(fake_sheet()));
        let view = CheatSheetView::new(shared.clone(), CheatsheetConfig::default());
        view.prepare_open(0, 0);
        let prims = tessellate(&view);

        // 面板底是半透明色，tessellate 后顶点色可能按预乘 alpha 落盘，两者都认
        let premul = |c: u8| (c as u32 * PANEL_BG.a() as u32 / 255) as u8;
        let premultiplied = egui::Color32::from_rgba_premultiplied(
            premul(PANEL_BG.r()),
            premul(PANEL_BG.g()),
            premul(PANEL_BG.b()),
            PANEL_BG.a(),
        );
        let is_panel_bg = |c: egui::Color32| c == PANEL_BG || c == premultiplied;

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
            min_y < 20.0 && max_y > SIZE[1] - 20.0,
            "浮层底没铺满整窗: 图元 y 范围 {min_y:.0}..{max_y:.0}（期望 ≈0..{}）",
            SIZE[1]
        );
        assert!(
            height > SIZE[1] - 40.0,
            "浮层底竖向只覆盖 {height:.0}px（期望 ≈{}）",
            SIZE[1]
        );
    }
}
