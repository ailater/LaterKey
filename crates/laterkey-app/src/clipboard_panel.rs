//! 剪贴板历史面板：第二视口（独立小窗），Win+V 呼出。
//!
//! 弹出方式是 [`egui::Context::show_viewport_deferred`] 而不是 immediate：
//! immediate 必须每帧在父视口的 paint pass 里调用，而托盘驻留时设置窗口是
//! 隐藏的，eframe 那时只跑 `logic()`（文档明确 logic() 里禁止任何 UI/绘制），
//! 面板恰恰要在这种时刻弹出。deferred 只需每帧注册一次回调，子视口有自己
//! 的重绘节奏，根窗口藏不藏都不影响。
//!
//! 窗口保留标题栏：无边框窗口在 macOS 不能成为 key window，面板就打不了字。
//! 关闭途径：Esc、失焦、点标题栏红点、选中条目贴回。
//!
//! 面板自己不碰历史数据：列表读 Shared 快照，贴回/清空经通道发给 worker。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use eframe::egui;
use egui::{Align, Color32, Key, RichText, Stroke, ViewportBuilder, ViewportId};

use laterkey_core::ClipboardEntry;
use laterkey_platform_mac::{PanelCommand, Shared};

use crate::settings::{ACCENT, BORDER, CARD, DESC, MUTED, ORANGE, TEXT};

/// 视口尺寸与 ask 一致。
const SIZE: [f32; 2] = [420.0, 520.0];
/// 预览每行截断宽度（字符数），避免超长条目拖慢布局。
const PREVIEW_CHARS: usize = 64;
/// 自动聚焦的重试窗口：系统拒绝激活（如全屏应用限制）时只重试这么久，
/// 之后放弃——每帧重发空转没有意义，失焦自动关闭也随之降级为手动关。
const FOCUS_RETRY: Duration = Duration::from_secs(2);

fn viewport_id() -> ViewportId {
    ViewportId(egui::Id::new("clipboard_panel"))
}

/// 面板内部状态（主线程读写；Mutex 只为满足 deferred 回调的 Send+Sync）。
#[derive(Default)]
struct PanelState {
    search: String,
    /// 键盘选中条目的 id（0 = 未选）。列表变化时按 id 跟随而不是行号：
    /// 面板开着期间新拷贝会插顶，行号会把高亮和 Enter 贴回带偏。
    selected: u64,
    /// 面板拿到过焦点（失焦自动关闭，只在拿到过之后才算「失」）。
    was_focused: bool,
    /// 自动聚焦重试的截止时刻。
    focus_until: Option<Instant>,
    /// 键盘移动过选中项：这一帧把选中项滚进视野。
    scrolled: bool,
    /// 回调请求关闭（Esc/失焦/红点/贴回），TrayApp 看到就停注册，视口随之销毁。
    close: bool,
    /// 过滤缓存：搜索词或历史快照变了才重算。存过滤后条目在快照里的下标。
    last_query: String,
    last_snapshot: Option<Arc<[ClipboardEntry]>>,
    filtered: Vec<usize>,
    /// 条目小写副本缓存（按 id）：搜索匹配不每帧对全文重复分配。
    lower: HashMap<u64, String>,
    /// 图片条目的缩略图纹理缓存（按条目 id），解码与上传只在首次。
    textures: HashMap<u64, egui::TextureHandle>,
}

/// 面板句柄。TrayApp 持有，开着时每帧调一次 [`Panel::show`]。
#[derive(Clone)]
pub struct Panel {
    inner: Arc<Inner>,
}

struct Inner {
    state: Mutex<PanelState>,
    tx: std::sync::mpsc::Sender<PanelCommand>,
    shared: Arc<Shared>,
}

impl Panel {
    pub fn new(tx: std::sync::mpsc::Sender<PanelCommand>, shared: Arc<Shared>) -> Self {
        Self {
            inner: Arc::new(Inner {
                state: Mutex::new(PanelState {
                    ..Default::default()
                }),
                tx,
                shared,
            }),
        }
    }

    /// 每帧注册视口（不注册的那一帧 egui 就把窗口收走）。
    pub fn show(&self, ctx: &egui::Context) {
        let panel = self.clone();
        ctx.show_viewport_deferred(
            viewport_id(),
            ViewportBuilder::default()
                .with_inner_size(SIZE)
                .with_title("剪贴板历史")
                .with_resizable(false)
                .with_always_on_top(),
            move |ui, _class| panel.draw(ui),
        );
    }

    /// 回调请求关闭：TrayApp 检查后停注册。
    pub fn close_requested(&self) -> bool {
        self.inner
            .state
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .close
    }

    fn draw(&self, ui: &mut egui::Ui) {
        let mut st = self.inner.state.lock().unwrap_or_else(|p| p.into_inner());
        let ctx = ui.ctx().clone();

        // 关闭途径：红点、Esc、失焦
        if ctx.input(|i| i.viewport().close_requested()) {
            st.close = true;
        }
        if ctx.input(|i| i.key_pressed(Key::Escape)) {
            st.close = true;
        }
        let focused = ctx.input(|i| i.viewport().focused);
        if focused == Some(true) {
            st.was_focused = true;
        } else if focused == Some(false) && st.was_focused {
            st.close = true;
        }

        // 自动聚焦（带 2s 重试上限）：直接可以打字搜索；系统拒绝激活就放弃重试
        if !st.was_focused {
            let deadline = *st
                .focus_until
                .get_or_insert_with(|| Instant::now() + FOCUS_RETRY);
            if Instant::now() < deadline {
                ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
            }
        }

        // 键盘：↑↓ 移动，Enter 贴回
        let (up, down, enter) = ctx.input(|i| {
            (
                i.key_pressed(Key::ArrowUp),
                i.key_pressed(Key::ArrowDown),
                i.key_pressed(Key::Enter),
            )
        });

        let entries = self.inner.shared.history();
        // 过滤缓存：只在搜索词或历史快照变化的那帧重算（egui 每帧都跑这里，
        // 全量小写会随 1 MiB 级条目放大成百 MB 分配）
        let snapshot_same = st
            .last_snapshot
            .as_ref()
            .is_some_and(|s| Arc::ptr_eq(s, &entries));
        if st.search != st.last_query || !snapshot_same {
            st.last_query = st.search.clone();
            st.last_snapshot = Some(entries.clone());
            let query = st.search.to_lowercase();
            st.filtered = refilter(&entries, &query, &mut st.lower);
            // 图片条目被淘汰或清空后释放纹理
            st.textures
                .retain(|id, _| entries.iter().any(|e| e.id == *id));
        }
        let selected_row = row_of_selected(&st.filtered, &entries, st.selected);

        egui::CentralPanel::default().show(ui, |ui| {
            // 搜索框
            let search = egui::TextEdit::singleline(&mut st.search)
                .hint_text("搜索剪贴板历史")
                .desired_width(ui.available_width())
                .id(egui::Id::new("clip_search"));
            let out = search.show(ui);
            if !st.was_focused && st.focus_until.is_some_and(|t| Instant::now() < t) {
                // 焦点还没落上且还在重试窗口内：每帧再要一次（幂等）
                out.response.request_focus();
            }

            // 选中按 id 跟随：↑↓ 换算成新 id 存回，插顶/淘汰都不会贴错条目
            if (up || down) && !st.filtered.is_empty() {
                let next = if up {
                    selected_row.saturating_sub(1)
                } else {
                    (selected_row + 1).min(st.filtered.len() - 1)
                };
                let new_id = entries[st.filtered[next]].id;
                if new_id != st.selected {
                    st.selected = new_id;
                    st.scrolled = true;
                }
            }
            let selected_row = row_of_selected(&st.filtered, &entries, st.selected);

            ui.add_space(4.0);
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    if st.filtered.is_empty() {
                        ui.add_space(40.0);
                        let msg = if st.search.is_empty() {
                            "暂无剪贴板历史，先拷贝点什么"
                        } else {
                            "没有匹配的内容"
                        };
                        ui.vertical_centered(|ui| {
                            ui.label(RichText::new(msg).size(13.0).color(MUTED));
                        });
                    }
                    let now = now_ms();
                    // 索引循环：循环体里要改 st.close，不能整段借住 st.filtered
                    for row in 0..st.filtered.len() {
                        let i = st.filtered[row];
                        let entry = &entries[i];
                        let selected = row == selected_row;
                        let app = source_name(entry);
                        let inner = egui::Frame::new()
                            .fill(if selected {
                                Color32::from_rgba_unmultiplied(0x7a, 0xa2, 0xf7, 36)
                            } else {
                                CARD
                            })
                            .stroke(if selected {
                                Stroke::new(1.5, ACCENT)
                            } else {
                                Stroke::new(1.0, BORDER)
                            })
                            .corner_radius(egui::CornerRadius::same(8))
                            .inner_margin(egui::Margin::same(10))
                            .show(ui, |ui| {
                                ui.set_min_width(ui.available_width());
                                match &entry.image {
                                    Some(img) => {
                                        // 图片条目：缩略图 + 描述（text 存的是「图片 宽×高」）
                                        ui.horizontal(|ui| {
                                            if let Some(tex) =
                                                texture_for(ui.ctx(), &mut st, entry.id, img)
                                            {
                                                let aspect = tex.size_vec2();
                                                let h = 48.0;
                                                let w =
                                                    (aspect.x / aspect.y * h).clamp(28.0, 160.0);
                                                ui.add(
                                                    egui::Image::new(&tex)
                                                        .fit_to_exact_size(egui::vec2(w, h)),
                                                );
                                            }
                                            ui.vertical(|ui| {
                                                ui.add(
                                                    egui::Label::new(
                                                        RichText::new(&entry.text)
                                                            .size(13.0)
                                                            .color(TEXT),
                                                    )
                                                    .truncate(),
                                                );
                                                ui.label(
                                                    RichText::new(format!(
                                                        "{} · {}",
                                                        relative_time(entry.captured_at, now),
                                                        app
                                                    ))
                                                    .size(11.0)
                                                    .color(MUTED),
                                                );
                                            });
                                        });
                                    }
                                    None => {
                                        let (l1, l2) = preview_lines(&entry.text, PREVIEW_CHARS);
                                        ui.add(
                                            egui::Label::new(
                                                RichText::new(&l1).size(13.0).color(TEXT),
                                            )
                                            .truncate(),
                                        );
                                        if !l2.is_empty() {
                                            ui.add(
                                                egui::Label::new(
                                                    RichText::new(&l2).size(12.0).color(DESC),
                                                )
                                                .truncate(),
                                            );
                                        }
                                        ui.add_space(2.0);
                                        ui.label(
                                            RichText::new(format!(
                                                "{} · {}",
                                                relative_time(entry.captured_at, now),
                                                app
                                            ))
                                            .size(11.0)
                                            .color(MUTED),
                                        );
                                    }
                                }
                            });
                        let resp = ui.interact(
                            inner.response.rect,
                            egui::Id::new(("clip_item", entry.id)),
                            egui::Sense::click(),
                        );
                        if selected && st.scrolled {
                            resp.scroll_to_me(Some(Align::Center));
                        }
                        // tooltip 只在 hover 命中时才取（全文截短也有 2000 字符）
                        let resp = if resp.hovered() {
                            resp.on_hover_text(tooltip_text(&entry.text))
                        } else {
                            resp
                        };
                        if resp.clicked() {
                            let _ = self.inner.tx.send(PanelCommand::Paste(entry.clone()));
                            st.close = true;
                        }
                        ui.add_space(4.0);
                    }
                });
            st.scrolled = false;

            ui.add_space(4.0);
            ui.separator();
            ui.horizontal(|ui| {
                if ui
                    .add(egui::Button::new(
                        RichText::new("清空历史").size(12.0).color(ORANGE),
                    ))
                    .clicked()
                {
                    let _ = self.inner.tx.send(PanelCommand::Clear);
                }
                ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                    ui.label(
                        RichText::new(format!(
                            "{} 条 · ↑↓ 选择 · 回车 贴回 · Esc 关闭",
                            st.filtered.len()
                        ))
                        .size(11.0)
                        .color(MUTED),
                    );
                });
            });

            if enter {
                if let Some(&i) = st.filtered.get(selected_row) {
                    let _ = self.inner.tx.send(PanelCommand::Paste(entries[i].clone()));
                    st.close = true;
                }
            }
        });
    }
}

/// 条目缩略图纹理：按条目 id 缓存，解码与上传只在首次。
/// 最大边 1024 缩样后再上传——Retina 截图原尺寸一张就是几十 MB 显存。
fn texture_for(
    ctx: &egui::Context,
    st: &mut PanelState,
    id: u64,
    img: &laterkey_core::ImageData,
) -> Option<egui::TextureHandle> {
    if let Some(tex) = st.textures.get(&id) {
        return Some(tex.clone());
    }
    let decoded = image::load_from_memory(&img.bytes).ok()?;
    let decoded = if decoded.width().max(decoded.height()) > 1024 {
        decoded.thumbnail(1024, 1024)
    } else {
        decoded
    };
    let size = [decoded.width() as usize, decoded.height() as usize];
    let rgba = decoded.into_rgba8().into_raw();
    let tex = ctx.load_texture(
        format!("clip-{id}"),
        egui::ColorImage::from_rgba_unmultiplied(size, &rgba),
        egui::TextureOptions::LINEAR,
    );
    st.textures.insert(id, tex.clone());
    Some(tex)
}

/// 过滤 + 小写副本缓存。只在搜索词或快照变化时调用一次；
/// `query` 为空（最常见的开面板即看）零匹配成本，全量列出。
fn refilter(
    entries: &[ClipboardEntry],
    query: &str,
    cache: &mut HashMap<u64, String>,
) -> Vec<usize> {
    let mut out = Vec::new();
    for (i, e) in entries.iter().enumerate() {
        if query.is_empty() {
            out.push(i);
            continue;
        }
        let lower = cache.entry(e.id).or_insert_with(|| e.text.to_lowercase());
        if lower.contains(query) {
            out.push(i);
        }
    }
    // 缓存收敛：条目被淘汰或清空后，副本不再留着
    cache.retain(|id, _| entries.iter().any(|e| e.id == *id));
    out
}

/// 选中 id → 过滤后列表的行号。条目被清空/滤掉时回落顶条：贴回顶条
/// 总比贴错条目强。
fn row_of_selected(filtered: &[usize], entries: &[ClipboardEntry], selected: u64) -> usize {
    filtered
        .iter()
        .position(|&i| entries[i].id == selected)
        .unwrap_or(0)
}

/// 来源列：显示名优先，退回 bundle id，都没有就「未知」。
fn source_name(entry: &ClipboardEntry) -> String {
    if entry.source_bundle_id.is_empty() {
        return "未知来源".into();
    }
    laterkey_platform_mac::app_name_for_bundle(&entry.source_bundle_id)
        .unwrap_or_else(|| entry.source_bundle_id.clone())
}

/// 全文 tooltip 截短：1 MiB 的条目不能整个塞进 tooltip。
fn tooltip_text(text: &str) -> String {
    const MAX: usize = 2000;
    if text.chars().count() <= MAX {
        text.to_string()
    } else {
        text.chars().take(MAX).collect::<String>() + "…"
    }
}

/// 预览：取前两个非空行，各自截断（按字符截，别在多字节中间下刀）。
pub fn preview_lines(text: &str, max_chars: usize) -> (String, String) {
    let mut lines = text.lines().filter(|l| !l.trim().is_empty());
    let first = lines.next().unwrap_or("");
    let second = lines.next().unwrap_or("");
    (truncate(first, max_chars), truncate(second, max_chars))
}

fn truncate(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(max_chars).collect();
        out.push('…');
        out
    }
}

/// 相对时间：刚刚 / N 分钟前 / N 小时前 / N 天前。时钟回拨按「刚刚」算。
pub fn relative_time(captured_at_ms: u64, now_ms: u64) -> String {
    let s = now_ms.saturating_sub(captured_at_ms) / 1000;
    match s {
        0..=59 => "刚刚".into(),
        60..=3599 => format!("{} 分钟前", s / 60),
        3600..=86399 => format!("{} 小时前", s / 3600),
        _ => format!("{} 天前", s / 86400),
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::Ordering;

    #[test]
    fn preview_takes_two_nonempty_lines_and_truncates() {
        assert_eq!(
            preview_lines("hello\nworld", 64),
            ("hello".into(), "world".into())
        );
        assert_eq!(preview_lines("only", 64), ("only".into(), String::new()));
        assert_eq!(
            preview_lines("\n\nthird\nfourth", 64),
            ("third".into(), "fourth".into())
        );
        assert_eq!(preview_lines("", 64), (String::new(), String::new()));
        // 截断加省略号，且不在多字节字符中间断开（4 字符 + …）
        let (a, b) = preview_lines("剪贴板剪贴板剪贴板", 4);
        assert_eq!(a, "剪贴板剪…");
        assert_eq!(b, "");
        assert_eq!(preview_lines("ab\ncd", 1).0, "a…");
    }

    #[test]
    fn relative_time_buckets() {
        let now = 10_000_000_000; // 毫秒基准，够减 3 天不溢出
        assert_eq!(relative_time(now - 30_000, now), "刚刚");
        assert_eq!(relative_time(now - 120_000, now), "2 分钟前");
        assert_eq!(relative_time(now - 7_200_000, now), "2 小时前");
        assert_eq!(relative_time(now - 3 * 86_400_000, now), "3 天前");
        // 时钟回拨（captured_at 在未来）不算负数
        assert_eq!(relative_time(now + 5_000, now), "刚刚");
    }

    #[test]
    fn shared_round_trip_of_open_signal() {
        // 面板开/关由 TrayApp 轮询 panel_seq 驱动；这里验证序号语义可用
        let shared = Shared::new(std::sync::mpsc::sync_channel(1).0, false);
        assert_eq!(shared.panel_seq.load(Ordering::Relaxed), 0);
        shared.panel_seq.fetch_add(1, Ordering::Relaxed);
        assert_eq!(shared.panel_seq.load(Ordering::Relaxed), 1);
        // 目标 pid 快照默认 0（贴回路径会拒贴）
        assert_eq!(shared.panel_target_pid.load(Ordering::Relaxed), 0);
    }

    fn entry(id: u64, text: &str) -> ClipboardEntry {
        ClipboardEntry {
            id,
            text: text.into(),
            captured_at: 0,
            source_bundle_id: String::new(),
            image: None,
        }
    }

    #[test]
    fn refilter_is_case_insensitive_and_cache_converges() {
        let entries = vec![entry(1, "Hello"), entry(2, "世界"), entry(3, "hello again")];
        let mut cache = HashMap::new();
        assert_eq!(refilter(&entries, "hello", &mut cache), vec![0, 2]);
        assert_eq!(refilter(&entries, "", &mut cache).len(), 3, "空查询全量");
        // 缓存收敛：条目没了副本也删掉
        let kept = vec![entry(2, "世界")];
        let _ = refilter(&kept, "x", &mut cache);
        assert!(!cache.contains_key(&1) && !cache.contains_key(&3));
        assert!(cache.contains_key(&2));
    }

    #[test]
    fn selection_follows_entry_id_not_row() {
        let snapshot = vec![entry(1, "a"), entry(2, "b"), entry(3, "c")];
        let filtered: Vec<usize> = (0..snapshot.len()).collect();
        // 选中 id=2（行 1）；面板开着期间新拷贝插顶，列表变成 [9,1,2,3]
        assert_eq!(row_of_selected(&filtered, &snapshot, 2), 1);
        let mut snapshot2 = vec![entry(9, "new")];
        snapshot2.extend(snapshot.clone());
        let filtered2: Vec<usize> = (0..snapshot2.len()).collect();
        // 高亮仍落在 id=2 那条（行 2），不会漂移到别的条目
        assert_eq!(row_of_selected(&filtered2, &snapshot2, 2), 2);
        // 条目被清空/滤掉：回落顶条，Enter 贴顶条总比贴错强
        assert_eq!(row_of_selected(&filtered2, &snapshot2, 99), 0);
        assert_eq!(row_of_selected(&[], &snapshot2, 2), 0);
    }
}
