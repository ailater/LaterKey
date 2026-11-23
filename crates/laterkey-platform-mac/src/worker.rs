//! 后台线程：轮询前台应用与安全输入状态（写进原子量给回调读），执行回调转来的
//! 动作（锁屏、合成按键、最小化窗口），定期刷新 Dock 图标缓存。
//!
//! 拦截回调里不能做任何可能阻塞的事，所以凡是「要跟别的进程说话」的活都在这里。

use std::sync::atomic::Ordering;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use laterkey_core::{GroupTable, Rules};

use super::access;
use super::dock;
use super::focus;
use super::lock;
use super::post;
use super::tap::{Job, Shared};

/// 轮询间隔。前台应用的变化最多晚这么久被注意到。
pub const POLL_INTERVAL: Duration = Duration::from_millis(50);
/// Dock 图标缓存的兜底刷新间隔（前台应用变化时会立刻刷新）。
const TILE_INTERVAL: Duration = Duration::from_secs(2);

pub struct Worker {
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
}

/// 界面改了开关后，把新规则经这条通道交给后台线程（重建状态机与应用组表）。
pub type Update = (Rules, GroupTable);

impl Worker {
    /// 起后台线程；跑完第一轮（前台应用、安全输入、图标缓存都已就绪）才返回。
    pub fn start(
        shared: Arc<Shared>,
        jobs: Receiver<Job>,
        groups: GroupTable,
        updates: Receiver<Update>,
    ) -> Result<Self, String> {
        let (ready_tx, ready_rx) = mpsc::channel();
        let thread = std::thread::Builder::new()
            .name("laterkey-worker".into())
            .spawn({
                let shared = shared.clone();
                move || worker_loop(shared, jobs, groups, updates, ready_tx)
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
    updates: Receiver<Update>,
    ready: mpsc::Sender<()>,
) {
    let mut ready = Some(ready);
    let mut last: Option<(i32, u64)> = None;
    let mut last_secure = false;
    // 让第一轮必定刷新一次
    let mut last_tiles = Instant::now() - TILE_INTERVAL;

    while !shared.stop.load(Ordering::SeqCst) {
        // 应用新规则：非阻塞地把通道里所有更新取完，用最后一份
        while let Ok((rules, table)) = updates.try_recv() {
            *shared.rules.write().unwrap_or_else(|p| p.into_inner()) = rules;
            groups = table;
            shared.tap_needs_reset.store(true, Ordering::SeqCst);
            info("规则已更新（来自菜单/设置面板）");
        }

        match jobs.recv_timeout(POLL_INTERVAL) {
            Ok(job) => handle(&shared, job),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
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
            info("剪贴板历史面板尚未实现：这次 ⌘V 被吞掉了（可在配置里关掉 win_key_clipboard）");
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
