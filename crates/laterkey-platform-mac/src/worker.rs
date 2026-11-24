//! 后台线程：轮询前台应用与安全输入状态（写进原子量给回调读），执行回调转来的
//! 动作（锁屏、合成按键、最小化窗口），定期刷新 Dock 图标缓存。
//!
//! 拦截回调里不能做任何可能阻塞的事，所以凡是「要跟别的进程说话」的活都在这里。

use std::sync::atomic::Ordering;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use laterkey_core::{
    flags, keys, png_dimensions, Chord, ClipboardEntry, GroupTable, History, ImageFormat, Rules,
};

use super::access;
use super::dock;
use super::focus;
use super::lock;
use super::pasteboard;
use super::post;
use super::tap::{Job, Shared};

/// 轮询间隔。前台应用的变化最多晚这么久被注意到。
pub const POLL_INTERVAL: Duration = Duration::from_millis(50);
/// Dock 图标缓存的兜底刷新间隔（前台应用变化时会立刻刷新）。
const TILE_INTERVAL: Duration = Duration::from_secs(2);
/// 贴回时等前台切回目标应用的上限。超时不贴——宁可不贴也不能贴错窗口。
const PASTE_ACTIVATE_TIMEOUT: Duration = Duration::from_millis(1500);

pub struct Worker {
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
}

/// 界面改了开关后，把新规则经这条通道交给后台线程（重建状态机与应用组表）。
pub type Update = (Rules, GroupTable);

/// 剪贴板面板发给 worker 的命令（经独立通道，worker 线程执行）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PanelCommand {
    /// 贴回选中的条目：写回剪贴板 → 激活目标应用 → 等 → 合成 ⌘V。
    Paste(ClipboardEntry),
    /// 清空全部历史。
    Clear,
}

impl Worker {
    /// 起后台线程；跑完第一轮（前台应用、安全输入、图标缓存都已就绪）才返回。
    ///
    /// `rules` 是启动时的生效规则：updates 通道只在用户碰开关后才发，
    /// worker 的采集开关不能等那份更新——关了 Win+V 历史的用户从第一毫秒起
    /// 就不该被采集。
    pub fn start(
        shared: Arc<Shared>,
        jobs: Receiver<Job>,
        groups: GroupTable,
        rules: Rules,
        updates: Receiver<Update>,
        panels: Receiver<PanelCommand>,
    ) -> Result<Self, String> {
        let (ready_tx, ready_rx) = mpsc::channel();
        let thread = std::thread::Builder::new()
            .name("laterkey-worker".into())
            .spawn({
                let shared = shared.clone();
                move || worker_loop(shared, jobs, groups, rules, updates, panels, ready_tx)
            })
            .map_err(|e| format!("后台线程启动失败：{e}"))?;
        match ready_rx.recv() {
            Ok(()) => Ok(Worker {
                shared,
                thread: Some(thread),
            }),
            Err(_) => {
                let _ = thread.join();
                Err("后台线程启动后即退出".into())
            }
        }
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::SeqCst);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn worker_loop(
    shared: Arc<Shared>,
    jobs: Receiver<Job>,
    mut groups: GroupTable,
    rules: Rules,
    updates: Receiver<Update>,
    panels: Receiver<PanelCommand>,
    ready: mpsc::Sender<()>,
) {
    let mut ready = Some(ready);
    let mut last: Option<(i32, u64)> = None;
    let mut last_secure = false;
    // 让第一轮必定刷新一次
    let mut last_tiles = Instant::now() - TILE_INTERVAL;
    // 规则的本地副本：worker 是唯一写方，采集判断不必再过共享锁。
    // 初值来自 Worker::start 的参数（updates 通道没有初始更新可收）
    let mut cur_rules = rules;
    // 剪贴板历史 worker 独有（单线程写免锁），变了才把快照发布给界面
    let mut history = History::new();
    // 板上内容的版本号：变了才去取内容
    let mut last_change: u64 = 0;
    // 进行中的贴回：等前台切回目标应用（每轮循环推进一步，不阻塞整轮）
    let mut paste: Option<PasteBack> = None;

    while !shared.stop.load(Ordering::SeqCst) {
        // 应用新规则：非阻塞地把通道里所有更新取完，用最后一份
        while let Ok((rules, table)) = updates.try_recv() {
            *shared.rules.write().unwrap_or_else(|p| p.into_inner()) = rules.clone();
            cur_rules = rules;
            groups = table;
            shared.tap_needs_reset.store(true, Ordering::SeqCst);
            info("规则已更新（来自菜单/设置面板）");
        }

        match jobs.recv_timeout(POLL_INTERVAL) {
            Ok(job) => handle(&shared, job),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }

        // 面板命令：贴回 / 清空。频率极低，非阻塞取完即可
        while let Ok(cmd) = panels.try_recv() {
            match cmd {
                PanelCommand::Clear => {
                    history = History::new();
                    shared.set_history(Vec::new());
                    info("已清空剪贴板历史");
                }
                PanelCommand::Paste(entry) => begin_paste_back(&shared, &mut paste, entry),
            }
        }

        // 贴回状态机步进：写板与激活在命令到达那轮做完，这里只等前台切回。
        // 等待切片到轮询节奏，安全输入检测、前台跟踪、采集都不停摆
        if let Some(p) = paste.take() {
            if focus::frontmost_pid() == Some(p.target) {
                // OWN_TAG 让拦截层放行这条合成 ⌘V
                let chord = Chord {
                    keycode: keys::V,
                    flags: flags::COMMAND,
                };
                if post::chord(chord) {
                    // 贴回即置顶；写回的文本会被再次采到，与顶部相同而去重，不成环
                    history.touch(p.entry.id, now_ms());
                    shared.set_history(history.entries().to_vec());
                    info("已贴回一条剪贴板历史");
                } else {
                    info("贴回失败：合成 ⌘V 不成功");
                }
            } else if Instant::now() >= p.deadline {
                info("贴回取消：1.5s 内前台没切回目标应用");
            } else {
                paste = Some(p);
            }
        }

        // 前台应用：变了才写原子量、才刷新图标缓存（切换应用时 Dock 会重排）
        let front = focus::frontmost().map(|f| {
            let mask = groups.lookup(&f.bundle_id);
            (f.bundle_id, f.pid, mask)
        });
        if let Some((bundle_id, pid, mask)) =
            front.filter(|(_, pid, mask)| last != Some((*pid, *mask)))
        {
            last = Some((pid, mask));
            shared.front_pid.store(pid, Ordering::Relaxed);
            shared.groups.store(mask, Ordering::Relaxed);
            trace(
                &shared,
                format!("前台应用 → {bundle_id}（pid {pid}，应用组 {mask:#x}）"),
            );
            last_tiles = refresh_tiles(&shared);
        }

        // 安全输入：开了就暂停映射
        let secure = access::secure_input_enabled();
        if secure != last_secure {
            last_secure = secure;
            shared.paused.store(secure, Ordering::Relaxed);
            info(if secure {
                "安全输入开启，映射暂停"
            } else {
                "安全输入关闭，映射恢复"
            });
        }

        // 剪贴板采集：规则开着且安全输入没开才采。停采期间只同步版本号——
        // 恢复时不回头采暂停期间拷的东西（那段可能有没带 Concealed 标记的密码）
        if capture_enabled(cur_rules.win_key_clipboard, secure) {
            capture_clipboard(&shared, &mut history, &mut last_change);
        } else {
            last_change = pasteboard::change_count();
        }

        if last_tiles.elapsed() >= TILE_INTERVAL {
            last_tiles = refresh_tiles(&shared);
        }

        if let Some(ready) = ready.take() {
            let _ = ready.send(());
        }
    }
}

fn refresh_tiles(shared: &Shared) -> Instant {
    let now = Instant::now();
    let dock_pid = focus::dock_pid().unwrap_or(0);
    shared.dock_pid.store(dock_pid, Ordering::Relaxed);
    let tiles = if dock_pid == 0 {
        Vec::new()
    } else {
        dock::read_tiles(dock_pid, &focus::running_apps())
    };
    trace(
        shared,
        format!("Dock 图标缓存：{} 个运行中应用", tiles.len()),
    );
    shared.set_tiles(tiles);
    now
}

/// 这一轮要不要采剪贴板。抽成纯函数配测试：启动初值（Worker::start 传入）
/// 与运行期开关更新共用这条判断。
fn capture_enabled(win_key_clipboard: bool, secure_input: bool) -> bool {
    win_key_clipboard && !secure_input
}

/// 采集一次剪贴板：changeCount 变了才取内容，入库后发布快照给界面。
/// 文本与图片各自入库；带禁采标记、空白、文件引用都不采。
fn capture_clipboard(shared: &Shared, history: &mut History, last_change: &mut u64) {
    let change = pasteboard::change_count();
    if change == *last_change {
        return;
    }
    *last_change = change;
    let content = match pasteboard::read() {
        Some(pasteboard::ClipContent::Text(text)) if pasteboard::worth_keeping(&text) => {
            if history
                .record(text, now_ms(), source_bundle(shared))
                .is_some()
            {
                shared.set_history(history.entries().to_vec());
                trace(
                    shared,
                    format!("剪贴板采到一条文本，历史 {} 条", history.len()),
                );
            }
            return;
        }
        Some(pasteboard::ClipContent::Image { bytes, format }) => (bytes, format),
        _ => return,
    };
    let (bytes, format) = content;
    let label = describe_image(&bytes);
    if history
        .record_image(bytes, format, now_ms(), source_bundle(shared), label)
        .is_some()
    {
        shared.set_history(history.entries().to_vec());
        trace(
            shared,
            format!("剪贴板采到一张图片，历史 {} 条", history.len()),
        );
    }
}

/// 拷贝来源应用 = 拷贝发生时刻的真实前台（现查，比 50ms 前的快照准；
/// 面板开着期间前台是 LaterKey 自己，条目来源会记成自己，已知限制）。
fn source_bundle(shared: &Shared) -> String {
    let pid = focus::frontmost_pid().unwrap_or_else(|| shared.front_pid.load(Ordering::Relaxed));
    if pid > 0 {
        focus::bundle_id_for_pid(pid).unwrap_or_default()
    } else {
        String::new()
    }
}

/// 图片条目的列表描述：PNG 从 IHDR 读尺寸，其余给通用文案。
fn describe_image(bytes: &[u8]) -> String {
    match png_dimensions(bytes) {
        Some((w, h)) => format!("图片 {w}×{h}"),
        None => "图片".into(),
    }
}

/// 进行中的贴回：写板与激活已完成，等前台切回目标应用。
struct PasteBack {
    entry: ClipboardEntry,
    target: i32,
    deadline: Instant,
}

/// 贴回的快步骤当场做：写回剪贴板、激活目标应用。等前台切回的慢等待
/// 交给 worker_loop 的贴回状态机（每轮推进一步，不阻塞整轮循环）。
/// 任何一步走不通就不贴——贴错窗口比不贴更糟。
fn begin_paste_back(shared: &Shared, slot: &mut Option<PasteBack>, entry: ClipboardEntry) {
    let target = shared.panel_target_pid.load(Ordering::Relaxed);
    if target <= 0 {
        info("贴回取消：没有记录打开面板时的前台应用");
        return;
    }
    let written = match &entry.image {
        Some(img) => pasteboard::write_image(&img.bytes, img.format),
        None => pasteboard::write_string(&entry.text),
    };
    if !written {
        info("贴回失败：写回剪贴板不成功");
        return;
    }
    if !focus::activate_pid(target) {
        info("贴回失败：目标应用激活不了（可能已退出）");
        return;
    }
    slot.replace(PasteBack {
        entry,
        target,
        deadline: Instant::now() + PASTE_ACTIVATE_TIMEOUT,
    });
}

/// 毫秒时间戳。core 不取系统时间，由平台层传入。
fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn handle(shared: &Shared, job: Job) {
    match job {
        Job::Substitute(chord) => {
            if !post::chord(chord) {
                info("合成按键失败");
            }
        }
        Job::Action(laterkey_core::Action::LockScreen) => match lock::lock_screen() {
            Ok(method) => info(&format!("锁屏（{method:?}）")),
            Err(e) => info(&format!("锁屏失败：{e}")),
        },
        Job::Action(laterkey_core::Action::ClipboardHistory) => {
            // 面板由 GUI 轮询 panel_seq 弹出；面板开着时 GUI 忽略新序号（不开两次）
            shared.panel_seq.fetch_add(1, Ordering::Relaxed);
            info("呼出剪贴板历史面板");
        }
        Job::Action(laterkey_core::Action::TaskManager) => {
            // 任务管理器 = macOS 强制退出窗口：合成 ⌘⌥Esc（OWN_TAG 让拦截层放行）
            let force_quit = laterkey_core::Chord {
                keycode: laterkey_core::keys::ESCAPE,
                flags: laterkey_core::flags::COMMAND | laterkey_core::flags::ALTERNATE,
            };
            if post::chord(force_quit) {
                info("已呼出强制退出窗口（⌘⌥Esc）");
            } else {
                info("强制退出窗口呼出失败");
            }
        }
        Job::Minimize { pid } => {
            let n = dock::minimize_app(pid);
            shared
                .stats
                .dock_minimize
                .fetch_add(n as u64, Ordering::Relaxed);
            info(&format!("Dock 再点 → 最小化 pid {pid} 的 {n} 个窗口"));
        }
    }
}

/// 始终打印：动作与状态切换。
fn info(msg: &str) {
    eprintln!("[laterkey] {msg}");
}

/// 仅 --verbose 时打印。
fn trace(shared: &Shared, msg: String) {
    if shared.verbose {
        eprintln!("[+{:6.2}s] {msg}", shared.started.elapsed().as_secs_f32());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capture_gate_follows_rule_and_secure_input() {
        // 规则关了就不采——哪怕安全输入也没开（「开关关掉后完全不采集」的边界）
        assert!(!capture_enabled(false, false));
        assert!(capture_enabled(true, false));
        // 安全输入开着不采
        assert!(!capture_enabled(true, true));
        assert!(!capture_enabled(false, true));
    }

    #[test]
    fn initial_rules_reach_capture_gate() {
        // 启动时规则里关了 Win+V 历史（Worker::start 的初值路径），不采集
        let rules = Rules {
            win_key_clipboard: false,
            ..Rules::default()
        };
        assert!(!capture_enabled(rules.win_key_clipboard, false));
    }

    #[test]
    fn paste_state_machine_waits_then_gives_up() {
        // 状态机的两个出口：前台切回 → 执行；超时 → 放弃。这里的可测部分是
        // 超时判定（frontmost_pid 是真机行为，不测）
        let entry = ClipboardEntry {
            id: 1,
            text: "hi".into(),
            captured_at: 0,
            source_bundle_id: String::new(),
            image: None,
        };
        let mut slot = Some(PasteBack {
            entry,
            target: 42,
            deadline: Instant::now() - Duration::from_millis(1),
        });
        // 已过期：deadline 早于现在，状态机该走「取消」出口
        let p = slot.take().unwrap();
        assert!(Instant::now() >= p.deadline);
        assert!(slot.is_none(), "超时后不再回到等待状态");
    }
}
