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
//! - 每行的分配矩形必须是整行宽，键与名称都画在矩形内，否则换行错位；
//! - 内容超窗时统一滚动：滚轮在浮窗内容区任何位置都滚整个内容，主标题
//!   条固定不滚；行的绘制与点击区域随滚动上移，可视区外的行不画不响应。
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
use laterkey_core::{CheatsheetConfig, DisplayRules, ShortcutKey, Style};
use laterkey_platform_mac::Shared;

/// 视口尺寸：参考图实测 1017×849（左边缘 6px、上边缘 6px）。
const SIZE: [f32; 2] = [1017.0, 849.0];
/// 浮层圆角（参考图实测约 30px）。
const PANEL_RADIUS: u8 = 30;
/// 标题条圆角。
const HEADER_RADIUS: u8 = 6;
/// 浮层底：参考图实测面板底约 (42,41,42)，alpha 约 230。
const PANEL_BG: Color32 = Color32::from_rgba_unmultiplied_const(42, 41, 42, 230);
/// 组标题条底：参考图实测约 (64,63,64)。
const HEADER_BG: Color32 = Color32::from_rgba_unmultiplied_const(64, 63, 64, 255);
/// 键名文字（近白）。
const KEY_TEXT: Color32 = Color32::from_rgba_unmultiplied_const(232, 234, 245, 255);
/// 功能名文字（参考图实测纯白）。
const NAME_TEXT: Color32 = Color32::from_rgba_unmultiplied_const(255, 255, 255, 255);
/// 标题文字（纯白）。
const HEADER_TEXT: Color32 = Color32::from_rgba_unmultiplied_const(255, 255, 255, 255);
/// 右上角控件：悬停淡底（半透明白，无彩色）。
const CONTROL_BG: Color32 = Color32::from_rgba_unmultiplied_const(255, 255, 255, 16);
/// 控件选中态的常驻淡底。
const CONTROL_BG_ON: Color32 = Color32::from_rgba_unmultiplied_const(255, 255, 255, 26);
/// 控件未选中文字（降调）。
const CONTROL_DIM: Color32 = Color32::from_rgba_unmultiplied_const(232, 234, 245, 150);

// ---- 固定坐标（参考图逐像素量出，x/y 均相对窗口左上角） ----
/// 应用区左边距。
const APP_X: f32 = 36.0;
/// 应用区宽度。
const APP_W: f32 = 231.0;
/// 系统区左边距。
const SYS_X: f32 = 357.0;
/// 系统区总宽。
const SYS_W: f32 = 641.0;
/// 系统左列宽。
const SYS_LEFT_W: f32 = 326.0;
/// 系统右列左边距（相对窗口）。
const SYS_RIGHT_X: f32 = 773.0;
/// 系统右列宽。
const SYS_RIGHT_W: f32 = 225.0;
/// 顶部标题条 y。
const TITLE_Y: f32 = 20.0;
/// 顶部标题条高。
const TITLE_H: f32 = 35.0;
/// 组标题条高。
const GROUP_H: f32 = 22.0;
/// 快捷键行高。
const ROW_H: f32 = 23.0;
/// 主标题条到第一组标题条的间距（参考图实测：主标题 y=20..55，第一组 y=67）。
const TITLE_TO_GROUP: f32 = 12.0;
/// 组与组之间的空隙（参考图实测约 1px）。
const GROUP_GAP: f32 = 1.0;

/// 键名列三条线的最小值（Mac 键帽口径的参考图实测值；物理键口径会被
/// column_metrics 按列内最宽内容右推）：
/// 修饰键右对齐到列内此 x（相对列左边缘）。
const MODIFIER_RIGHT: f32 = 47.0;
/// 主键左对齐到列内此 x（相对列左边缘）。
const MAIN_KEY_LEFT: f32 = 61.0;
/// 功能名左对齐到列内此 x（相对列左边缘）。
const NAME_LEFT: f32 = 122.0;

/// 数据还没到时按这个节奏轮询 Shared 快照。
const FETCH_POLL: f32 = 0.1;
/// 开窗后忽略失焦的宽限期：窗口重新成 key 需要一两帧，期间的焦点抖动
/// 不是「点击外部」。
const FOCUS_GRACE: std::time::Duration = std::time::Duration::from_millis(300);
/// 内容可视区顶（第一组标题的 y）：主标题条固定不滚，组内容从这里开始。
const CONTENT_TOP: f32 = TITLE_Y + TITLE_H + TITLE_TO_GROUP;
/// 内容可视区底：窗口底再留一点空隙。
const CONTENT_BOTTOM: f32 = SIZE[1] - 24.0;

fn viewport_id() -> ViewportId {
    ViewportId(egui::Id::new("cheatsheet_peek"))
}

#[derive(Default)]
struct CheatState {
    /// 视觉字号配置（来自 config.toml 的 [cheatsheet]）。
    visual: CheatsheetConfig,
    /// 是否已升级到系统苹方字体（首次打开时加载一次）。
    fonts_upgraded: bool,
    /// 浮窗本会话曾经获得过焦点；用于识别点击外部后的失焦关闭。
    /// 每次开窗都要重置——留着上一轮的 true 会在重开瞬间误判失焦关窗。
    was_focused: bool,
    /// 本会话开窗时刻；开窗后头几百毫秒忽略失焦，兜底窗口重新成 key
    /// 之前的焦点闪烁。
    opened_at: Option<std::time::Instant>,
    /// 内容区统一滚动偏移，0 = 顶；开窗清零。
    scroll: f32,
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
        // 焦点状态必须一并清：视口隐藏期间 focused=Some(false)，若沿用上一轮
        // 的 was_focused=true，重开的第一帧（窗口还没重新成 key）会被当成
        // 「点击外部」直接关掉——表现为闪一下就没了。
        st.was_focused = false;
        st.opened_at = Some(std::time::Instant::now());
        st.scroll = 0.0;
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
        if !st.fonts_upgraded {
            st.fonts_upgraded = true;
            crate::font::upgrade_sheet_fonts(ui.ctx());
        }

        if ui.input(|i| i.viewport().close_requested()) || ui.input(|i| i.key_pressed(Key::Escape))
        {
            st.close = true;
        }
        let focused = ui.input(|i| i.viewport().focused);
        let grace_over = st
            .opened_at
            .map(|t| t.elapsed() >= FOCUS_GRACE)
            .unwrap_or(true);
        if focused == Some(true) {
            st.was_focused = true;
        } else if focused == Some(false)
            && st.was_focused
            && grace_over
            && !ui.input(|i| i.modifiers.ctrl)
        {
            // Ctrl 仍按住时点击浮窗属于速览内交互，不当成点击外部；
            // 否则「长按 Ctrl 打开 → Ctrl+点击」会被误判为失焦关闭。
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

                // 固定坐标直接相对窗口左上角，不套内边距 Frame——之前左右各让
                // 36px 会把整套参考图坐标推右，系统右列被推出窗外。
                draw_columns(ui, &mut st, display, loading);
            });
    }
}

// ---------------------------------------------------------------- 绘制

/// 每行快捷键拆成两段：修饰键（右对齐）+ 主键（左对齐）。
#[derive(Clone, Debug, PartialEq)]
struct ShortcutRow {
    /// 修饰键文本（Mac 口径是键帽符号串，物理键口径是文字串）
    modifier_text: String,
    /// 主键文本（Mac 口径是键帽符号，物理键口径是文字）
    main_key_text: String,
    name: String,
    key: ShortcutKey,
    modifiers: u64,
    /// 来自系统组：点击执行要走 System Events 注入。
    system: bool,
}

#[derive(Clone, Debug, PartialEq)]
struct SheetGroup {
    title: String,
    rows: Vec<ShortcutRow>,
    /// 来自系统组（symbolic hotkeys）：点击执行要走 System Events 注入，
    /// Dock 负责的系统热键不认用户态合成的 CGEvent。
    system: bool,
}

fn draw_columns(ui: &mut egui::Ui, st: &mut CheatState, display: DisplayRules, _loading: bool) {
    let app_groups = section_groups(&st.data.app_groups, st.style, &display, false);
    let system_groups = section_groups(&st.data.system_groups, st.style, &display, true);

    // 读不到菜单或加载中时：不画任何组，只保留主标题，下面完全空着
    // （参考工具在菜单读不到时也是空栏，不画占位标题条）

    let app_title = if st.data.app_name.is_empty() {
        "当前应用".to_string()
    } else {
        st.data.app_name.clone()
    };

    // ---- 固定坐标绘制：不用 egui 自适应布局，每个标题条和行都按量出的 x/y 画 ----
    let origin = ui.max_rect().left_top();

    // 键名列三条线按各列实际内容自适应：Mac 键帽紧凑、物理键口径的
    // 「Ctrl+Shift+Alt+」很宽，固定列位会把它挤出列左缘裁掉
    let app_col = Column {
        x: APP_X,
        w: APP_W,
        m: column_metrics(ui, app_groups.iter(), st.visual.key_size, APP_W),
    };
    let (left_groups, right_groups) = split_system_groups(&system_groups);
    let left_col = Column {
        x: SYS_X,
        w: SYS_LEFT_W,
        m: column_metrics(
            ui,
            left_groups.iter().copied(),
            st.visual.key_size,
            SYS_LEFT_W,
        ),
    };
    let right_col = Column {
        x: SYS_RIGHT_X,
        w: SYS_RIGHT_W,
        m: column_metrics(
            ui,
            right_groups.iter().copied(),
            st.visual.key_size,
            SYS_RIGHT_W,
        ),
    };

    let vis_h = CONTENT_BOTTOM - CONTENT_TOP;

    // 统一滚动：滚轮在浮窗内容区任何位置都滚整个内容，三列同步；
    // 命中区从主标题条开始覆盖到底
    let scroll_dy = ui.input(|i| i.smooth_scroll_delta.y);
    let pointer = ui.input(|i| i.pointer.latest_pos());
    let content_h = column_height(app_groups.iter())
        .max(column_height(left_groups.iter().copied()))
        .max(column_height(right_groups.iter().copied()));
    let max_scroll = (content_h - vis_h).max(0.0);
    let hit = egui::Rect::from_min_size(
        origin + egui::vec2(APP_X, TITLE_Y),
        egui::vec2(SYS_X + SYS_W - APP_X, TITLE_H + vis_h),
    );
    if max_scroll > 0.0 && pointer.is_some_and(|pos| hit.contains(pos)) {
        st.scroll = (st.scroll - scroll_dy).clamp(0.0, max_scroll);
    }
    let view = View {
        origin,
        scroll: st.scroll,
    };

    // 应用区
    draw_section_title(ui, origin, APP_X, TITLE_Y, APP_W, &app_title, st);
    let mut y = CONTENT_TOP;
    for group in &app_groups {
        y = draw_group(ui, view, app_col, y, group, st);
    }

    // 系统区
    draw_section_title(ui, origin, SYS_X, TITLE_Y, SYS_W, "系统", st);
    let mut left_y = CONTENT_TOP;
    for group in &left_groups {
        left_y = draw_group(ui, view, left_col, left_y, group, st);
    }
    let mut right_y = CONTENT_TOP;
    for group in &right_groups {
        right_y = draw_group(ui, view, right_col, right_y, group, st);
    }

    // 滚动指示条：画在窗口右缘，内容超窗且鼠标悬停浮窗时才淡显
    // （平时不显示，参考工具的内容区右缘也是干净的）
    let hovering = pointer.is_some_and(|pos| hit.contains(pos));
    draw_scroll_thumb(ui, origin, content_h, st.scroll, hovering);

    // 右上角控件最后画：叠在「系统」标题条右端之上
    draw_controls(ui, origin, st);
}

/// 一列内容排完的总高度（含组间空隙；滚动上限由它决定）。
fn column_height<'a>(groups: impl IntoIterator<Item = &'a SheetGroup>) -> f32 {
    groups
        .into_iter()
        .map(|g| GROUP_H + g.rows.len() as f32 * ROW_H + GROUP_GAP)
        .sum()
}

/// 窗口右缘的极简滚动指示条：内容明显超窗才出现，滑块位置即当前偏移。
/// 只在鼠标悬停浮窗时淡显，平时隐藏，不占视觉。溢出 ≤4px 视为放得下。
fn draw_scroll_thumb(
    ui: &egui::Ui,
    origin: egui::Pos2,
    content_h: f32,
    scroll: f32,
    hovering: bool,
) {
    let vis_h = CONTENT_BOTTOM - CONTENT_TOP;
    let max_scroll = (content_h - vis_h).max(0.0);
    if max_scroll <= 4.0 || !hovering {
        return;
    }
    let thumb_h = (vis_h * vis_h / content_h).clamp(28.0, vis_h);
    let t = (scroll / max_scroll).clamp(0.0, 1.0);
    let rect = egui::Rect::from_min_size(
        origin + egui::vec2(SIZE[0] - 8.0, CONTENT_TOP + t * (vis_h - thumb_h)),
        egui::vec2(3.0, thumb_h),
    );
    ui.painter().rect_filled(
        rect,
        1.5,
        Color32::from_rgba_unmultiplied_const(255, 255, 255, 45),
    );
}

/// 画一个主标题条（「ZCode」「系统」）：固定坐标、圆角矩形、居中大号文字。
fn draw_section_title(
    ui: &egui::Ui,
    origin: egui::Pos2,
    x: f32,
    y: f32,
    w: f32,
    title: &str,
    st: &CheatState,
) {
    let rect = egui::Rect::from_min_size(origin + egui::vec2(x, y), egui::vec2(w, TITLE_H));
    ui.painter().rect_filled(rect, HEADER_RADIUS, HEADER_BG);
    ui.painter().text(
        rect.center(),
        Align2::CENTER_CENTER,
        title,
        crate::font::sheet_bold_font(st.visual.title_size),
        HEADER_TEXT,
    );
}

/// 右上角控件：「物理键 / Mac 原生」口径切换 + 圆形 ×。无边框窗口没有原生
/// 关闭键，托盘开的手动关闭全靠它；参考图右上角虽是空的，这两个入口功能上
/// 必须保留。样式走 macOS 浮层的 ghost 文字路线：无底块，选中只用文字明暗
/// 区分，悬停才浮出极淡的底，让控件融进「系统」标题条而不抢居中大标题。
fn draw_controls(ui: &egui::Ui, origin: egui::Pos2, st: &mut CheatState) {
    let cy = origin.y + TITLE_Y + TITLE_H / 2.0;
    let right = origin.x + SYS_X + SYS_W;
    let font = crate::font::sheet_font(12.0);

    // 两个口径标签：从右往左排（Mac 原生 靠右，物理键 在其左），
    // 与 ×、标签之间都留足空隙，不挤在标题条端头
    let mut cursor = right - 44.0; // 给 × 留 22 + 14 间距
    for (label, value) in [("Mac 原生", Style::Mac), ("物理键", Style::Physical)] {
        let text_w = text_width(ui, label, font.clone());
        let w = text_w + 16.0;
        let rect =
            egui::Rect::from_center_size(egui::pos2(cursor - w / 2.0, cy), egui::vec2(w, 20.0));
        cursor -= w + 10.0;
        let selected = st.style == value;
        let resp = ui.interact(
            rect,
            egui::Id::new(("cheatsheet_style", label)),
            egui::Sense::click(),
        );
        if resp.clicked() {
            st.style = value;
        }
        if selected {
            ui.painter().rect_filled(rect, 10.0, CONTROL_BG_ON);
        } else if resp.hovered() {
            ui.painter().rect_filled(rect, 10.0, CONTROL_BG);
        }
        ui.painter().text(
            rect.center(),
            Align2::CENTER_CENTER,
            label,
            font.clone(),
            if selected || resp.hovered() {
                KEY_TEXT
            } else {
                CONTROL_DIM
            },
        );
    }

    // 圆形 ×：往里收 8px 不贴边，无底只有字形，悬停浮出淡底
    let close_rect =
        egui::Rect::from_center_size(egui::pos2(right - 19.0, cy), egui::vec2(22.0, 22.0));
    let close = ui.interact(
        close_rect,
        egui::Id::new("cheatsheet_close"),
        egui::Sense::click(),
    );
    if close.clicked() {
        st.close = true;
    }
    if close.hovered() {
        ui.painter().rect_filled(close_rect, 10.0, CONTROL_BG);
    }
    ui.painter().text(
        close_rect.center(),
        Align2::CENTER_CENTER,
        "×",
        crate::font::sheet_font(13.0),
        if close.hovered() {
            KEY_TEXT
        } else {
            CONTROL_DIM
        },
    );
}

/// 一列：横向位置、宽度、自适应键位三条线。应用列与系统左右列各一份。
#[derive(Clone, Copy)]
struct Column {
    x: f32,
    w: f32,
    m: ColumnMetrics,
}

/// 一列的自适应键位坐标：修饰键右对齐线、主键左对齐线、功能名左对齐线。
/// 三个固定常量只是 Mac 键帽口径的最小值；物理键口径的长串会依次把三条线
/// 右推。功能名最多让到 w-60，再放不下就省略号 + 悬停提示兜底。
#[derive(Clone, Copy)]
struct ColumnMetrics {
    /// 修饰键右对齐到列内此 x。
    mod_right: f32,
    /// 主键左对齐到列内此 x。
    main_left: f32,
    /// 功能名左对齐到列内此 x。
    name_left: f32,
}

/// 量出一列里最宽的修饰键串和主键串，推出三条对齐线。
fn column_metrics<'a>(
    ui: &egui::Ui,
    groups: impl IntoIterator<Item = &'a SheetGroup>,
    key_size: f32,
    w: f32,
) -> ColumnMetrics {
    let font = crate::font::sheet_font(key_size);
    let mut max_mod = 0.0_f32;
    let mut max_main = 0.0_f32;
    for group in groups {
        for row in &group.rows {
            max_mod = max_mod.max(text_width(ui, &row.modifier_text, font.clone()));
            max_main = max_main.max(text_width(ui, &row.main_key_text, font.clone()));
        }
    }
    let mod_right = MODIFIER_RIGHT.max(max_mod + 4.0);
    let main_left = MAIN_KEY_LEFT.max(mod_right + 14.0);
    let name_left = NAME_LEFT
        .max(main_left + max_main + 14.0)
        .min((w - 60.0).max(NAME_LEFT));
    ColumnMetrics {
        mod_right,
        main_left,
        name_left,
    }
}

/// 用 egui 的排版量字符串宽度（颜色无谓，只要 rect）。
fn text_width(ui: &egui::Ui, text: &str, font: egui::FontId) -> f32 {
    ui.painter()
        .layout_no_wrap(text.to_string(), font, Color32::WHITE)
        .rect
        .width()
}

/// 可视区状态：窗口原点 + 统一滚动偏移。裁剪矩形由列与常量推出。
#[derive(Clone, Copy)]
struct View {
    origin: egui::Pos2,
    scroll: f32,
}

/// 列的可视裁剪区：x 取列范围，y 固定为内容区（主标题条不参与滚动）。
fn view_clip(view: View, col: Column) -> egui::Rect {
    egui::Rect::from_min_size(
        view.origin + egui::vec2(col.x, CONTENT_TOP),
        egui::vec2(col.w, CONTENT_BOTTOM - CONTENT_TOP),
    )
}

/// 画一个组：标题条（左对齐缩进 21px）+ 行列表。返回组结束后的 y。
/// 内容按滚动偏移上移，只画可视区内的部分（半可见由裁剪画器负责）。
fn draw_group(
    ui: &egui::Ui,
    view: View,
    col: Column,
    y: f32,
    group: &SheetGroup,
    st: &mut CheatState,
) -> f32 {
    let origin = view.origin;
    let clip = view_clip(view, col);
    let rows_h = group.rows.len() as f32 * ROW_H;
    let top = y - view.scroll;
    // 整组在可视区外：跳过绘制（调用方的 y 仍按全高推进）
    if top >= clip.bottom() || top + GROUP_H + rows_h <= clip.top() {
        return y + GROUP_H + rows_h + GROUP_GAP;
    }
    let p = ui.painter().with_clip_rect(clip);

    // 标题条：整宽圆角矩形，文字左对齐缩进 21px
    let header_rect =
        egui::Rect::from_min_size(origin + egui::vec2(col.x, top), egui::vec2(col.w, GROUP_H));
    p.rect_filled(header_rect, HEADER_RADIUS, HEADER_BG);
    let font = crate::font::sheet_font(st.visual.group_size);
    let title = truncate_to_width(ui, &group.title, font.clone(), col.w - 30.0);
    p.text(
        origin + egui::vec2(col.x + 21.0, top + GROUP_H / 2.0),
        Align2::LEFT_CENTER,
        &title,
        crate::font::sheet_font(st.visual.group_size),
        HEADER_TEXT,
    );

    // 快捷键行
    let mut ry = y + GROUP_H;
    for row in &group.rows {
        draw_shortcut_row(ui, &p, view, col, ry, row, st);
        ry += ROW_H;
    }

    ry + GROUP_GAP
}

/// 画一行快捷键：修饰键右对齐、主键左对齐、功能名左对齐到列的三条线上；
/// 键串或功能名被截断时，悬停整行显示完整内容。可视区外不画不响应。
fn draw_shortcut_row(
    ui: &egui::Ui,
    p: &egui::Painter,
    view: View,
    col: Column,
    y: f32,
    row: &ShortcutRow,
    st: &mut CheatState,
) {
    let origin = view.origin;
    let top = origin.y + y - view.scroll;
    let full_rect =
        egui::Rect::from_min_size(egui::pos2(origin.x + col.x, top), egui::vec2(col.w, ROW_H));
    let hit = full_rect.intersect(view_clip(view, col));
    if hit.is_negative() {
        return;
    }
    let center_y = top + ROW_H / 2.0;
    let key_font = crate::font::sheet_font(st.visual.key_size);

    // 修饰键：右对齐到修饰键线
    if !row.modifier_text.is_empty() {
        draw_key_text(
            p,
            egui::pos2(origin.x + col.x + col.m.mod_right, center_y),
            Align2::RIGHT_CENTER,
            &row.modifier_text,
            st.visual.key_size,
        );
    }

    // 主键：左对齐到主键线
    if !row.main_key_text.is_empty() {
        draw_key_text(
            p,
            egui::pos2(origin.x + col.x + col.m.main_left, center_y),
            Align2::LEFT_CENTER,
            &row.main_key_text,
            st.visual.key_size,
        );
    }

    // 功能名：左对齐到名称线，超宽截断加 …
    let name_x = origin.x + col.x + col.m.name_left;
    let max_w = (origin.x + col.x + col.w - name_x).max(20.0);
    let name = truncate_to_width(
        ui,
        &row.name,
        crate::font::sheet_font(st.visual.name_size),
        max_w,
    );
    p.text(
        egui::pos2(name_x, center_y),
        Align2::LEFT_CENTER,
        &name,
        crate::font::sheet_font(st.visual.name_size),
        NAME_TEXT,
    );

    // 点击区域：可见部分
    let mut resp = ui.interact(hit, ui.id().with(("row", &row.name)), egui::Sense::click());
    if resp.hovered() {
        p.rect_filled(
            full_rect,
            4.0,
            Color32::from_rgba_unmultiplied_const(122, 162, 247, 14),
        );
        // 键串挤进名称区（物理键长串 + 窄列时）或名称被截断 → 悬停给全文
        let key_overflow =
            col.m.main_left + text_width(ui, &row.main_key_text, key_font) > col.m.name_left + 1.0;
        if key_overflow || name != row.name {
            resp = resp.on_hover_text(format!(
                "{}{}　{}",
                row.modifier_text, row.main_key_text, row.name
            ));
        }
    }
    if resp.clicked() {
        let chord = laterkey_core::Chord {
            keycode: match &row.key {
                ShortcutKey::VirtualKey(code) => *code,
                ShortcutKey::Char(ch) => laterkey_core::keys::named(ch).unwrap_or(-1),
            },
            flags: row.modifiers,
        };
        if chord.keycode >= 0 {
            if row.system {
                // 系统组：Dock 一类的热键不认合成 CGEvent，走 System Events 注入
                let _ = laterkey_platform_mac::trigger_chord_system(chord);
            } else {
                // 应用菜单：激活原应用后按合成组合键
                let _ = laterkey_platform_mac::trigger_chord(st.target_pid, chord);
            }
            st.close = true;
        }
    }
}

/// 把系统组按参考图的两列流式排布：保持顺序，优先填充左列。
fn split_system_groups(groups: &[SheetGroup]) -> (Vec<&SheetGroup>, Vec<&SheetGroup>) {
    let mut left = Vec::new();
    let mut right = Vec::new();
    let mut left_h = 0.0;
    let limit = SIZE[1] - TITLE_Y - TITLE_H - TITLE_TO_GROUP - 24.0;
    for group in groups {
        let height = GROUP_H + group.rows.len() as f32 * ROW_H + GROUP_GAP;
        if left_h + height <= limit {
            left.push(group);
            left_h += height;
        } else {
            right.push(group);
        }
    }
    (left, right)
}

fn section_groups(
    groups: &[laterkey_core::ShortcutGroup],
    style: Style,
    display: &DisplayRules,
    system: bool,
) -> Vec<SheetGroup> {
    groups
        .iter()
        .map(|group| SheetGroup {
            title: group.title.clone(),
            system,
            rows: group
                .entries
                .iter()
                .filter_map(|entry| {
                    laterkey_core::format_chord_parts(entry.modifiers, &entry.key, style, display)
                        .map(|(modifier_text, main_key_text)| ShortcutRow {
                            modifier_text,
                            main_key_text,
                            name: entry.title.clone(),
                            key: entry.key.clone(),
                            modifiers: entry.modifiers,
                            system,
                        })
                })
                .collect(),
        })
        .collect()
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
fn draw_key_text(
    p: &egui::Painter,
    anchor: egui::Pos2,
    align: Align2,
    text: &str,
    size: f32,
) -> f32 {
    let galley_for = |cjk: bool, s: String, color: Color32| {
        let font = if cjk {
            FontId::proportional(size)
        } else {
            crate::font::keycaps_font(size)
        };
        p.layout_no_wrap(s, font, color)
    };
    let runs = split_script_runs(text);
    if runs.is_empty() {
        return 0.0;
    }
    let galleys: Vec<_> = runs
        .iter()
        .map(|&(cjk, ref s)| galley_for(cjk, s.clone(), KEY_TEXT))
        .collect();
    let total_w: f32 = galleys.iter().map(|g| g.size().x).sum();
    let mut cursor_x = match align {
        Align2::RIGHT_CENTER | Align2::RIGHT_TOP | Align2::RIGHT_BOTTOM => anchor.x - total_w,
        Align2::CENTER_CENTER => anchor.x - total_w / 2.0,
        _ => anchor.x,
    };
    for g in &galleys {
        let pos = egui::pos2(cursor_x, anchor.y - g.size().y / 2.0);
        p.galley(pos, g.clone(), KEY_TEXT);
        cursor_x += g.size().x;
    }
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
        laterkey_platform_mac::CheatSheet {
            app_name: "ZCode".into(),
            app_groups: vec![
                laterkey_core::ShortcutGroup {
                    title: "Apple".into(),
                    entries: vec![entry("强制退出"), entry("退出登录"), entry("锁屏")],
                },
                laterkey_core::ShortcutGroup {
                    title: "文件".into(),
                    entries: (0..30).map(|i| entry(&format!("条目{i}"))).collect(),
                },
            ],
            system_groups: vec![
                laterkey_core::ShortcutGroup {
                    title: "调度中心".into(),
                    entries: (0..8).map(|i| entry(&format!("功能{i}"))).collect(),
                },
                laterkey_core::ShortcutGroup {
                    title: "聚焦".into(),
                    entries: (0..4).map(|i| entry(&format!("功能{i}"))).collect(),
                },
                laterkey_core::ShortcutGroup {
                    title: "窗口".into(),
                    entries: (0..6).map(|i| entry(&format!("功能{i}"))).collect(),
                },
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
