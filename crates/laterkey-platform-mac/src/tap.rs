//! 主动事件拦截：HID 层 CGEventTap。
//!
//! 骨架沿用 LaterScreen `mac_fnkey_tap.rs`（kCGHIDEventTap + head insert + 主动
//! 拦截），差别是源加在**独立线程**的 RunLoop 上：这里每个按键、每次点击都要过
//! 回调，不能被界面或后台任务拖慢。
//!
//! 回调里只做三件事：读原子量、交给纯函数决策、把决定就地写回事件。耗时的活
//! （锁屏、读窗口、最小化）一律经队列转给后台线程。

use std::ffi::c_void;
use std::ptr::null_mut;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicPtr, AtomicU64, Ordering};
use std::sync::mpsc::{self, SyncSender, TrySendError};
use std::sync::{Arc, RwLock};
use std::thread::JoinHandle;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use laterkey_core::{Action, Chord, ClipboardEntry, Decision, Env, Input, Kind, Remapper, Rules};

use super::cf::Cf;
use super::dock::{self, DockDecision, Tile};
use super::ffi::*;
use super::post::OWN_TAG;
use super::shortcuts::CheatSheet;

const fn mask_bit(ty: u32) -> u64 {
    1u64 << (ty & 63)
}

/// 关注的事件：键盘三类 + 左键按下 + 两种「拦截层被禁用」通知。
/// 通知型常量是 0xFFFFFFFE / 0xFFFFFFFF，C 的 CGEventMaskBit 按硬件移位截断（&63）。
const EVENT_MASK: u64 = mask_bit(EVENT_LEFT_MOUSE_DOWN)
    // up/drag 必须一并订阅：只改写 down 的话，应用看到「⌘ 按下、⌃ 抬起」，
    // 手势不完整——VS Code 的 ⌘+点击跳定义因此不触发（2026-11-23 真机定位）
    | mask_bit(EVENT_LEFT_MOUSE_UP)
    | mask_bit(EVENT_LEFT_MOUSE_DRAGGED)
    | mask_bit(EVENT_KEY_DOWN)
    | mask_bit(EVENT_KEY_UP)
    | mask_bit(EVENT_FLAGS_CHANGED)
    | mask_bit(EVENT_TAP_DISABLED_BY_TIMEOUT)
    | mask_bit(EVENT_TAP_DISABLED_BY_USER_INPUT);

/// 回调转给后台线程的活儿。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Job {
    Substitute(Chord),
    Action(Action),
    Minimize { pid: i32 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TapOptions {
    pub keys: bool,
    pub dock: bool,
    /// 逐条打印改写与吞掉的事件（只含键码和标志位，不含输入内容）
    pub verbose: bool,
    /// 仅自测用：把带标记的合成事件截住，不让它送达应用
    pub swallow_tagged: bool,
}

#[derive(Debug, Default)]
pub struct Stats {
    pub events: AtomicU64,
    pub rewrites: AtomicU64,
    pub consumed: AtomicU64,
    pub effects: AtomicU64,
    pub dropped_jobs: AtomicU64,
    pub paused_pass: AtomicU64,
    pub tagged: AtomicU64,
    pub dock_minimize: AtomicU64,
    pub tap_disabled: AtomicU64,
    pub max_cb_micros: AtomicU64,
}

impl Stats {
    fn n(&self, v: &AtomicU64) -> u64 {
        v.load(Ordering::Relaxed)
    }

    pub fn summary(&self) -> String {
        format!(
            "统计：事件 {}，改写 {}，吞掉 {}，动作 {}，暂停期间放行 {}，拦截层被禁用 {} 次，回调最长 {} µs，Dock 最小化 {} 个窗口",
            self.n(&self.events),
            self.n(&self.rewrites),
            self.n(&self.consumed),
            self.n(&self.effects),
            self.n(&self.paused_pass),
            self.n(&self.tap_disabled),
            self.n(&self.max_cb_micros),
            self.n(&self.dock_minimize),
        )
    }
}

/// 拦截线程与后台线程共享的只读状态 + 一条单向队列。回调只读原子量，不加锁
/// （图标缓存那把读锁只在左键按下时才碰，写方几秒才动一次）。
pub struct Shared {
    pub groups: AtomicU64,
    pub front_pid: AtomicI32,
    pub dock_pid: AtomicI32,
    pub paused: AtomicBool,
    pub stop: AtomicBool,
    pub verbose: bool,
    pub started: Instant,
    /// 生效中的规则。后台线程写（界面改了开关），拦截线程在事件之间读。
    pub rules: RwLock<Rules>,
    /// 剪贴板历史快照：worker 线程写，界面（面板）读。tiles 同款模式。
    /// 仅内存，退出即清；落盘存储是 ROADMAP P2 的待办项。
    history: RwLock<Arc<[ClipboardEntry]>>,
    /// Win+V 呼出面板的信号：worker 收到动作 +1，GUI 轮询到变化就弹面板。
    pub panel_seq: AtomicU64,
    /// 全局快捷键唤出设置面板的序号（GUI 轮询）
    pub settings_seq: AtomicU64,
    /// 打开面板那一刻的前台应用 pid（GUI 快照），贴回的目标窗口。
    pub panel_target_pid: AtomicI32,
    /// 规则换过之后，拦截线程要把状态机重置一次（避免沿用旧规则的状态）
    pub tap_needs_reset: AtomicBool,
    /// 长按 Ctrl 速览手势：裸 Ctrl 按住的起点（毫秒时间戳，0=没在跟踪）。
    /// 拦截线程写，worker 读并统一裁决触发/作废——回调里不能等 3 秒
    pub peek_since: AtomicU64,
    /// 快捷键速览浮窗的开窗请求序号（worker 手势 / 托盘菜单各 +1，GUI 轮询）
    pub cheatsheet_seq: AtomicU64,
    /// 触发过的长按「按住结束」次数（松开或作废），GUI 据此自动关掉手势开的浮窗
    pub cheatsheet_release: AtomicU64,
    /// 速览数据快照：读取线程写，GUI 读。version 变了才有新数据
    cheatsheet: RwLock<Arc<CheatSheet>>,
    pub cheatsheet_version: AtomicU64,
    tiles: RwLock<Arc<[Tile]>>,
    jobs: SyncSender<Job>,
    port: AtomicPtr<c_void>,
    pub stats: Stats,
}

impl Shared {
    pub fn new(jobs: SyncSender<Job>, verbose: bool) -> Self {
        Self {
            groups: AtomicU64::new(0),
            front_pid: AtomicI32::new(0),
            dock_pid: AtomicI32::new(0),
            paused: AtomicBool::new(false),
            stop: AtomicBool::new(false),
            verbose,
            started: Instant::now(),
            tiles: RwLock::new(Arc::from(Vec::new())),
            rules: RwLock::new(Rules::default()),
            history: RwLock::new(Arc::from(Vec::new())),
            panel_seq: AtomicU64::new(0),
            settings_seq: AtomicU64::new(0),
            panel_target_pid: AtomicI32::new(0),
            tap_needs_reset: AtomicBool::new(false),
            peek_since: AtomicU64::new(0),
            cheatsheet_seq: AtomicU64::new(0),
            cheatsheet_release: AtomicU64::new(0),
            cheatsheet: RwLock::new(Arc::new(CheatSheet::default())),
            cheatsheet_version: AtomicU64::new(0),
            jobs,
            port: AtomicPtr::new(null_mut()),
            stats: Stats::default(),
        }
    }

    pub fn tiles(&self) -> Arc<[Tile]> {
        self.tiles.read().unwrap_or_else(|p| p.into_inner()).clone()
    }

    pub fn set_tiles(&self, tiles: Vec<Tile>) {
        *self.tiles.write().unwrap_or_else(|p| p.into_inner()) = tiles.into();
    }

    /// 剪贴板历史快照（从新到旧）。
    pub fn history(&self) -> Arc<[ClipboardEntry]> {
        self.history
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }

    /// worker 采到新内容（或清空）后发布快照。
    pub fn set_history(&self, entries: Vec<ClipboardEntry>) {
        *self.history.write().unwrap_or_else(|p| p.into_inner()) = entries.into();
    }

    /// 速览数据快照（可能是上一次的旧数据；GUI 以 version 判断是否新）。
    pub fn cheatsheet(&self) -> Arc<CheatSheet> {
        self.cheatsheet
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }

    /// 读取线程拿到新数据后发布，版本号 +1。
    pub fn set_cheatsheet(&self, sheet: Arc<CheatSheet>) {
        *self.cheatsheet.write().unwrap_or_else(|p| p.into_inner()) = sheet;
        self.cheatsheet_version.fetch_add(1, Ordering::Relaxed);
    }

    /// 非阻塞投递：队列满了就丢（宁可少执行一次动作，也不能卡住回调）。
    pub fn submit(&self, job: Job) -> bool {
        match self.jobs.try_send(job) {
            Ok(()) => true,
            Err(TrySendError::Full(_)) | Err(TrySendError::Disconnected(_)) => false,
        }
    }
}

// ---------------------------------------------------------------- 决策

/// 回调里的可观测点：统计计数，调试时逐条打印。
pub trait Trace {
    fn rewrite(&mut self, _before: Input, _keycode: i64, _flags: u64) {}
    fn consume(&mut self, _before: Input) {}
    fn effect(&mut self, _before: Input, _effect: &Effect) {}
    /// 命中了替换规则的键被原样放行（前台应用不在对应组）
    fn passthrough(&mut self, _input: Input, _groups: u64) {}
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Effect {
    Action(Action),
    Substitute(Chord),
}

#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    /// 放行（可能已经就地改写）
    Pass,
    Consume,
    Effect(Effect),
}

/// 处理一个键盘事件：改写就地写回 `event`。
///
/// # Safety
///
/// `event` 必须是类型为 `ty` 的有效键盘事件。
pub unsafe fn decide_key(
    remapper: &mut Remapper,
    groups: u64,
    event: CGEventRef,
    ty: u32,
    trace: &mut impl Trace,
) -> Outcome {
    let input = Input {
        kind: match ty {
            EVENT_KEY_DOWN => Kind::KeyDown,
            EVENT_KEY_UP => Kind::KeyUp,
            _ => Kind::FlagsChanged,
        },
        keycode: CGEventGetIntegerValueField(event, FIELD_KEYCODE),
        flags: CGEventGetFlags(event),
        repeat: CGEventGetIntegerValueField(event, FIELD_AUTOREPEAT) != 0,
    };
    match remapper.on_input(input, &Env { groups }) {
        Decision::Keep => {
            // 调试：裸按下了一个替换规则里的键，但前台应用不在对应组——
            // 「F2/F5 没反应」最常见就是这个原因，逐条记下来好定位
            if input.kind == Kind::KeyDown
                && !input.repeat
                && input.flags & laterkey_core::flags::ANY_MODIFIER == 0
                && remapper
                    .rules()
                    .substitutes
                    .iter()
                    .any(|s| s.keycode == input.keycode)
            {
                trace.passthrough(input, groups);
            }
            Outcome::Pass
        }
        Decision::Consume => {
            trace.consume(input);
            Outcome::Consume
        }
        Decision::Rewrite { keycode, flags } => {
            CGEventSetIntegerValueField(event, FIELD_KEYCODE, keycode);
            CGEventSetFlags(event, flags);
            trace.rewrite(input, keycode, flags);
            Outcome::Pass
        }
        Decision::Action(action) => {
            let effect = Effect::Action(action);
            trace.effect(input, &effect);
            Outcome::Effect(effect)
        }
        Decision::Substitute(chord) => {
            let effect = Effect::Substitute(chord);
            trace.effect(input, &effect);
            Outcome::Effect(effect)
        }
    }
}

struct TapTrace<'a> {
    stats: &'a Stats,
    verbose: bool,
    started: Instant,
}

impl Trace for TapTrace<'_> {
    fn rewrite(&mut self, before: Input, keycode: i64, flags: u64) {
        self.stats.rewrites.fetch_add(1, Ordering::Relaxed);
        if self.verbose {
            eprintln!(
                "[+{:6.2}s] 改写 {:?} kc={} flags={:#x} → kc={} flags={:#x}",
                self.started.elapsed().as_secs_f32(),
                before.kind,
                before.keycode,
                before.flags,
                keycode,
                flags
            );
        }
    }

    fn consume(&mut self, before: Input) {
        self.stats.consumed.fetch_add(1, Ordering::Relaxed);
        if self.verbose {
            eprintln!(
                "[+{:6.2}s] 吞掉 {:?} kc={} flags={:#x}",
                self.started.elapsed().as_secs_f32(),
                before.kind,
                before.keycode,
                before.flags
            );
        }
    }

    fn passthrough(&mut self, before: Input, groups: u64) {
        if self.verbose {
            eprintln!(
                "[+{:6.2}s] 透传 裸 kc={} flags={:#x}（前台应用组 {groups:#x}，不在替换规则组里）",
                self.started.elapsed().as_secs_f32(),
                before.keycode,
                before.flags
            );
        }
    }

    fn effect(&mut self, before: Input, effect: &Effect) {
        self.stats.effects.fetch_add(1, Ordering::Relaxed);
        if self.verbose {
            eprintln!(
                "[+{:6.2}s] 动作 {effect:?}（来自 {:?} kc={} flags={:#x}）",
                self.started.elapsed().as_secs_f32(),
                before.kind,
                before.keycode,
                before.flags
            );
        }
    }
}

// ---------------------------------------------------------------- 回调

struct TapCtx {
    shared: Arc<Shared>,
    /// 只在拦截线程上访问
    remapper: Remapper,
    opts: TapOptions,
    was_paused: bool,
}

unsafe extern "C" fn tap_callback(
    _proxy: CGEventTapProxy,
    ty: u32,
    event: CGEventRef,
    info: *mut c_void,
) -> CGEventRef {
    // SAFETY: info 是 Box::into_raw 出来的 TapCtx。回调只在本线程的 RunLoop 上触发，
    // 而 RunLoop 停止之后才回收它，所以这里有效。
    let ctx = unsafe { &mut *info.cast::<TapCtx>() };
    let started = Instant::now();
    // SAFETY: 见 dispatch 的说明
    let out = unsafe { dispatch(ctx, ty, event) };
    let micros = started.elapsed().as_micros() as u64;
    ctx.shared
        .stats
        .max_cb_micros
        .fetch_max(micros, Ordering::Relaxed);
    out
}

/// # Safety
///
/// 回调约定：`event` 在 `ty` 为键盘/鼠标事件时有效，为「禁用」通知时为 NULL。
unsafe fn dispatch(ctx: &mut TapCtx, ty: u32, event: CGEventRef) -> CGEventRef {
    let shared = &ctx.shared;
    shared.stats.events.fetch_add(1, Ordering::Relaxed);

    if ty == EVENT_TAP_DISABLED_BY_TIMEOUT || ty == EVENT_TAP_DISABLED_BY_USER_INPUT {
        // 回调太慢或被用户禁用：就地重启（此时 event 为 NULL）
        shared.stats.tap_disabled.fetch_add(1, Ordering::Relaxed);
        let port = shared.port.load(Ordering::Acquire);
        if !port.is_null() {
            CGEventTapEnable(port, true);
        }
        return null_mut();
    }
    if event.is_null() {
        return event;
    }

    if CGEventGetIntegerValueField(event, FIELD_USER_DATA) == OWN_TAG {
        // 自己注入的合成事件原样放行，否则会再进一次状态机
        shared.stats.tagged.fetch_add(1, Ordering::Relaxed);
        return if ctx.opts.swallow_tagged {
            null_mut()
        } else {
            event
        };
    }

    let paused = shared.paused.load(Ordering::Relaxed);
    if paused != ctx.was_paused {
        // 暂停期间看不到事件流，状态机可能与硬件脱节
        ctx.was_paused = paused;
        ctx.remapper.reset();
    }
    if paused {
        shared.stats.paused_pass.fetch_add(1, Ordering::Relaxed);
        return event;
    }

    match ty {
        EVENT_LEFT_MOUSE_DOWN | EVENT_LEFT_MOUSE_UP | EVENT_LEFT_MOUSE_DRAGGED => {
            ctx.on_mouse(event, ty)
        }
        EVENT_KEY_DOWN | EVENT_KEY_UP | EVENT_FLAGS_CHANGED if ctx.opts.keys => {
            ctx.on_key(event, ty)
        }
        _ => event,
    }
}

impl TapCtx {
    /// 长按 Ctrl 3 秒 → 快捷键速览的「按住起点」记录（含作废）。
    ///
    /// 这里只写一个时间戳，3 秒的等待由 worker 每轮裁决；期间敲任何键、
    /// 按别的修饰键、点鼠标都算手势作废。速览不是组合键的前置。
    ///
    /// # Safety
    ///
    /// `event` 必须是类型为 `ty` 的有效键盘事件。
    unsafe fn track_peek(&mut self, event: CGEventRef, ty: u32) {
        if !self.opts.keys || !self.remapper.rules().shortcuts_peek {
            return;
        }
        let modifiers = CGEventGetFlags(event) & laterkey_core::flags::ANY_MODIFIER;
        let keycode = CGEventGetIntegerValueField(event, FIELD_KEYCODE);
        let is_ctrl = keycode == laterkey_core::keys::CONTROL
            || keycode == laterkey_core::keys::RIGHT_CONTROL;
        match ty {
            EVENT_FLAGS_CHANGED if is_ctrl => {
                if modifiers == laterkey_core::flags::CONTROL {
                    // 裸 Ctrl 按住：开始计时（已是 0 才写入，触发后不重复计）
                    let _ = self.shared.peek_since.compare_exchange(
                        0,
                        unix_ms(),
                        Ordering::Relaxed,
                        Ordering::Relaxed,
                    );
                } else {
                    // 松开、或按成了组合（⌃⇧ 等）：作废
                    self.shared.peek_since.store(0, Ordering::Relaxed);
                }
            }
            // 按住期间敲了任何键：作废
            EVENT_KEY_DOWN | EVENT_KEY_UP => {
                self.shared.peek_since.store(0, Ordering::Relaxed);
            }
            _ => {}
        }
    }

    /// # Safety
    ///
    /// `event` 必须是类型为 `ty` 的有效键盘事件。
    unsafe fn on_key(&mut self, event: CGEventRef, ty: u32) -> CGEventRef {
        // 长按 Ctrl 速览的计时起点（含作废判断）；放在最前，避免和后面的
        // 共享借用冲突。规则刚换过时这里用的还是旧规则，差一个事件无碍
        self.track_peek(event, ty);
        let shared = &self.shared;
        if shared.tap_needs_reset.swap(false, Ordering::SeqCst) {
            // 规则换了：状态机带着新规则重来，避免沿用旧规则的修饰键状态
            let rules = shared
                .rules
                .read()
                .unwrap_or_else(|p| p.into_inner())
                .clone();
            self.remapper = Remapper::new(rules);
        }
        let groups = shared.groups.load(Ordering::Relaxed);
        let mut trace = TapTrace {
            stats: &shared.stats,
            verbose: shared.verbose,
            started: shared.started,
        };
        match decide_key(&mut self.remapper, groups, event, ty, &mut trace) {
            Outcome::Pass => event,
            Outcome::Consume => null_mut(),
            Outcome::Effect(effect) => {
                let job = match effect {
                    Effect::Action(action) => Job::Action(action),
                    Effect::Substitute(chord) => Job::Substitute(chord),
                };
                if !shared.submit(job) {
                    shared.stats.dropped_jobs.fetch_add(1, Ordering::Relaxed);
                }
                null_mut()
            }
        }
    }

    /// 鼠标事件：Dock 再点最小化（仅左键按下）+ Ctrl+点击的修饰键换算。
    ///
    /// # Safety
    ///
    /// `event` 必须是有效的左键事件（down/up/drag）。
    unsafe fn on_mouse(&mut self, event: CGEventRef, ty: u32) -> CGEventRef {
        let shared = &self.shared;

        if ty == EVENT_LEFT_MOUSE_DOWN {
            // Ctrl+点击是 ⌘+点击（跳定义/新标签），不是速览长按
            self.shared.peek_since.store(0, Ordering::Relaxed);
        }

        if ty == EVENT_LEFT_MOUSE_DOWN && self.opts.dock {
            let point = CGEventGetLocation(event);
            let front_pid = shared.front_pid.load(Ordering::Relaxed);

            // 先做纯内存命中：不在运行中应用的图标上，或者点的不是前台应用的图标，
            // 就立刻放行，不产生任何系统调用
            let tiles = shared.tiles();
            if dock::tile_at(&tiles, point).map(|t| t.pid) == Some(front_pid) {
                let dock_pid = shared.dock_pid.load(Ordering::Relaxed);
                if dock::decide(point, front_pid, dock_pid, &dock::on_screen_windows())
                    != DockDecision::Pass
                {
                    if !shared.submit(Job::Minimize { pid: front_pid }) {
                        shared.stats.dropped_jobs.fetch_add(1, Ordering::Relaxed);
                    }
                    return null_mut();
                }
            }
        }

        // 诊断：verbose 时把左键 down/up/drag 的原始 flags 都打出来（不改写也打），
        // 用来对照原生 ⌘+点击 与改写后的 ⌘+点击 的完整事件序列
        if shared.verbose {
            eprintln!(
                "[+{:6.2}s] 鼠标 type={} 原始 flags={:#x} ctrl_as_cmd={}",
                shared.started.elapsed().as_secs_f32(),
                ty,
                CGEventGetFlags(event),
                self.remapper.ctrl_as_cmd()
            );
        }

        // Ctrl+点击 → ⌘+点击：Mac 的 ⌘+点击是编辑器跳定义（VS Code/Zed/TRAE）、
        // 浏览器新标签、访达多选，对应 Windows 的 Ctrl+点击。键盘事件的 Ctrl 已被
        // 改写成 ⌘，点击事件不改的话应用收到的是 ⌃+点击（右键菜单）。
        if self.opts.keys && self.remapper.ctrl_as_cmd() {
            let raw = CGEventGetFlags(event);
            let swapped = laterkey_core::flags::swap(raw, &laterkey_core::flags::CTL_TO_CMD);
            if swapped != raw {
                CGEventSetFlags(event, swapped);
                shared.stats.rewrites.fetch_add(1, Ordering::Relaxed);
                if shared.verbose {
                    eprintln!(
                        "[+{:6.2}s] 改写 鼠标 type={} flags={:#x} → flags={:#x}",
                        shared.started.elapsed().as_secs_f32(),
                        ty,
                        raw,
                        swapped
                    );
                }
            }
        }
        event
    }
}

// ---------------------------------------------------------------- 生命周期

/// 毫秒时间戳（peek 手势的按住起点）。
fn unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// 已安装的拦截层。Drop 时停掉线程上的 RunLoop 并等它收尾。
pub struct EventTap {
    /// 拦截线程的 CFRunLoopRef（已 retain），用来从别的线程叫停
    runloop: Arc<AtomicPtr<c_void>>,
    thread: Option<JoinHandle<()>>,
}

impl EventTap {
    /// 起拦截线程并装上拦截层；装好了（或失败）才返回。
    pub fn start(
        shared: Arc<Shared>,
        remapper: Remapper,
        opts: TapOptions,
    ) -> Result<Self, String> {
        let runloop = Arc::new(AtomicPtr::new(null_mut()));
        let (ready_tx, ready_rx) = mpsc::channel();
        let thread = std::thread::Builder::new()
            .name("laterkey-tap".into())
            .spawn({
                let runloop = runloop.clone();
                move || tap_main(shared, remapper, opts, runloop, ready_tx)
            })
            .map_err(|e| format!("拦截线程启动失败：{e}"))?;
        match ready_rx.recv() {
            Ok(Ok(())) => Ok(EventTap {
                runloop,
                thread: Some(thread),
            }),
            Ok(Err(e)) => {
                let _ = thread.join();
                Err(e)
            }
            Err(_) => {
                let _ = thread.join();
                Err("拦截线程启动后即退出".into())
            }
        }
    }
}

impl Drop for EventTap {
    fn drop(&mut self) {
        let rl = self.runloop.load(Ordering::Acquire);
        if !rl.is_null() {
            // SAFETY: rl 是拦截线程 retain 过的 CFRunLoop；叫停后等线程自己收尾
            unsafe { CFRunLoopStop(rl) };
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn tap_main(
    shared: Arc<Shared>,
    remapper: Remapper,
    opts: TapOptions,
    runloop: Arc<AtomicPtr<c_void>>,
    ready: mpsc::Sender<Result<(), String>>,
) {
    if !super::ax_trusted() {
        let _ = ready.send(Err(
            "没有辅助功能权限：系统设置 › 隐私与安全性 › 辅助功能".into()
        ));
        return;
    }

    let ctx = Box::new(TapCtx {
        shared: shared.clone(),
        remapper,
        opts,
        was_paused: false,
    });
    let ctx_ptr = Box::into_raw(ctx);

    // SAFETY: kCGHIDEventTap / kCGHeadInsertEventTap / kCGEventTapOptionDefault（主动拦截）；
    // ctx_ptr 归本线程所有，RunLoop 停止后回收
    let port = unsafe {
        Cf::owned(CGEventTapCreate(
            TAP_HID,
            TAP_HEAD_INSERT,
            TAP_OPTION_DEFAULT,
            EVENT_MASK,
            tap_callback,
            ctx_ptr.cast(),
        ))
    };
    if port.is_null() {
        // Create 失败基本等于权限未生效（TCC 授权有延迟）
        drop(unsafe { Box::from_raw(ctx_ptr) });
        let _ = ready.send(Err(
            "拦截层创建失败：辅助功能权限没有生效，重新勾选后再试".into()
        ));
        return;
    }
    shared.port.store(port.0, Ordering::Release);

    // SAFETY: port 有效；返回的 source 归本线程所有
    let source = unsafe { Cf::owned(CFMachPortCreateRunLoopSource(std::ptr::null(), port.0, 0)) };
    if source.is_null() {
        unsafe {
            CFMachPortInvalidate(port.0);
        }
        shared.port.store(null_mut(), Ordering::Release);
        drop(source);
        drop(port);
        drop(unsafe { Box::from_raw(ctx_ptr) });
        let _ = ready.send(Err("拦截层的 RunLoop 源创建失败".into()));
        return;
    }

    // SAFETY: 本线程的 RunLoop；CFRetain 出来的引用交给 Cf 释放
    let rl = unsafe { Cf::owned(CFRetain(CFRunLoopGetCurrent()) as CFRunLoopRef) };
    unsafe {
        CFRunLoopAddSource(rl.0, source.0, kCFRunLoopCommonModes);
        CGEventTapEnable(port.0, true);
    }
    runloop.store(rl.0, Ordering::Release);
    let _ = ready.send(Ok(()));

    // SAFETY: 在本线程上跑本线程的 RunLoop
    unsafe { CFRunLoopRun() };

    // 收尾：先摘源、失效端口（此后不再有回调），再回收 userdata。
    // 回调与本段同在本线程，顺序有保证。
    unsafe {
        CFRunLoopRemoveSource(rl.0, source.0, kCFRunLoopCommonModes);
        CGEventTapEnable(port.0, false);
        CFMachPortInvalidate(port.0);
    }
    runloop.store(null_mut(), Ordering::Release);
    shared.port.store(null_mut(), Ordering::Release);
    drop(source);
    drop(port);
    drop(rl);
    // SAFETY: ctx_ptr 是上面 into_raw 出来的，且此后不再有回调
    let mut ctx = unsafe { Box::from_raw(ctx_ptr) };
    // 退出前复位修饰键：按住的键若被改写过，系统还认为「改写后的身份」按着，
    // 补一条抬起（带 OWN_TAG 直接投递，不再进状态机）。tap 已失效，顺序安全。
    for chord in ctx.remapper.release_all() {
        super::post::flags_changed(chord);
    }
    drop(ctx);
}

#[cfg(test)]
mod tests {
    use super::*;
    use laterkey_core::{flags, groups, keys, Rules};
    use std::time::Duration;

    /// 造一个事件，字段布局与真实硬件事件相同（类型、键码、标志位）。不投递。
    fn key(ty: u32, keycode: i64, flag_values: u64) -> Cf {
        // SAFETY: 只在本进程内读写，不投递
        unsafe {
            let ev = CGEventCreateKeyboardEvent(
                std::ptr::null_mut(),
                keycode as u16,
                ty != EVENT_KEY_UP,
            );
            CGEventSetType(ev, ty);
            CGEventSetFlags(ev, flag_values);
            Cf::owned(ev)
        }
    }

    fn state(ev: &Cf) -> (i64, u64) {
        // SAFETY: ev 是本测试创建的有效事件
        unsafe {
            (
                CGEventGetIntegerValueField(ev.0, FIELD_KEYCODE),
                CGEventGetFlags(ev.0),
            )
        }
    }

    /// 把回调收到的 trace 记下来，用来断言「改写前后各是什么」
    #[derive(Default)]
    struct Log(Vec<String>);

    impl Trace for Log {
        fn rewrite(&mut self, before: Input, keycode: i64, flags: u64) {
            self.0.push(format!(
                "rewrite {:?} kc={} {:#x} -> kc={} {:#x}",
                before.kind, before.keycode, before.flags, keycode, flags
            ));
        }
        fn consume(&mut self, before: Input) {
            self.0.push(format!(
                "consume {:?} kc={} {:#x}",
                before.kind, before.keycode, before.flags
            ));
        }
        fn effect(&mut self, before: Input, effect: &Effect) {
            self.0.push(format!(
                "effect {effect:?} from kc={} {:#x}",
                before.keycode, before.flags
            ));
        }
    }

    const CTL_DOWN: u64 = flags::CONTROL | flags::L_CTL;
    const CMD_L: u64 = flags::COMMAND | flags::L_CMD;

    #[test]
    fn ctrl_c_is_rewritten_on_the_real_event_object() {
        let mut r = Remapper::new(Rules::default());
        let mut log = Log::default();

        let ctl = key(EVENT_FLAGS_CHANGED, keys::CONTROL, CTL_DOWN);
        assert_eq!(
            unsafe { decide_key(&mut r, 0, ctl.0, EVENT_FLAGS_CHANGED, &mut log) },
            Outcome::Pass
        );
        assert_eq!(state(&ctl), (keys::COMMAND, CMD_L));

        let c_down = key(EVENT_KEY_DOWN, keys::C, CTL_DOWN);
        assert_eq!(
            unsafe { decide_key(&mut r, 0, c_down.0, EVENT_KEY_DOWN, &mut log) },
            Outcome::Pass
        );
        assert_eq!(state(&c_down), (keys::C, CMD_L));

        let c_up = key(EVENT_KEY_UP, keys::C, CTL_DOWN);
        assert_eq!(
            unsafe { decide_key(&mut r, 0, c_up.0, EVENT_KEY_UP, &mut log) },
            Outcome::Pass
        );
        assert_eq!(state(&c_up), (keys::C, CMD_L));

        let ctl_up = key(EVENT_FLAGS_CHANGED, keys::CONTROL, 0);
        assert_eq!(
            unsafe { decide_key(&mut r, 0, ctl_up.0, EVENT_FLAGS_CHANGED, &mut log) },
            Outcome::Pass
        );
        assert_eq!(state(&ctl_up), (keys::COMMAND, 0));

        assert_eq!(log.0.len(), 4);
    }

    #[test]
    fn unmodified_event_is_returned_as_is() {
        let mut r = Remapper::new(Rules::default());
        let mut log = Log::default();
        let ev = key(EVENT_KEY_DOWN, keys::C, 0);
        assert_eq!(
            unsafe { decide_key(&mut r, 0, ev.0, EVENT_KEY_DOWN, &mut log) },
            Outcome::Pass
        );
        assert_eq!(state(&ev), (keys::C, 0));
        assert!(log.0.is_empty());
    }

    #[test]
    fn win_l_is_consumed_and_reported_as_an_effect() {
        let mut r = Remapper::new(Rules::default());
        let mut log = Log::default();
        let ev = key(EVENT_KEY_DOWN, keys::L, CMD_L);
        assert_eq!(
            unsafe { decide_key(&mut r, 0, ev.0, EVENT_KEY_DOWN, &mut log) },
            Outcome::Effect(Effect::Action(Action::LockScreen))
        );
        let up = key(EVENT_KEY_UP, keys::L, CMD_L);
        assert_eq!(
            unsafe { decide_key(&mut r, 0, up.0, EVENT_KEY_UP, &mut log) },
            Outcome::Consume
        );
    }

    #[test]
    fn f5_in_a_browser_is_substituted() {
        let mut rules = Rules::default();
        rules.substitutes.push(laterkey_core::Substitute {
            keycode: keys::F5,
            groups: groups::BROWSER,
            to: Chord {
                keycode: keys::R,
                flags: flags::COMMAND,
            },
        });
        let mut r = Remapper::new(rules);
        let mut log = Log::default();
        let ev = key(EVENT_KEY_DOWN, keys::F5, 0);
        assert_eq!(
            unsafe { decide_key(&mut r, groups::BROWSER, ev.0, EVENT_KEY_DOWN, &mut log) },
            Outcome::Effect(Effect::Substitute(Chord {
                keycode: keys::R,
                flags: flags::COMMAND
            }))
        );
        // 同一个 F5，前台不是浏览器：原样放行
        let ev = key(EVENT_KEY_DOWN, keys::F5, 0);
        assert_eq!(
            unsafe { decide_key(&mut r, 0, ev.0, EVENT_KEY_DOWN, &mut log) },
            Outcome::Pass
        );
    }

    #[test]
    fn alt_tab_release_masquerades_as_command_release() {
        let mut r = Remapper::new(Rules::default());
        let mut log = Log::default();
        let alt = flags::ALTERNATE | flags::L_ALT;
        assert_eq!(
            unsafe {
                decide_key(
                    &mut r,
                    0,
                    key(EVENT_FLAGS_CHANGED, keys::OPTION, alt).0,
                    EVENT_FLAGS_CHANGED,
                    &mut log,
                )
            },
            Outcome::Pass
        );
        let tab = key(EVENT_KEY_DOWN, keys::TAB, alt);
        assert_eq!(
            unsafe { decide_key(&mut r, 0, tab.0, EVENT_KEY_DOWN, &mut log) },
            Outcome::Pass
        );
        assert_eq!(state(&tab), (keys::TAB, CMD_L));
        let release = key(EVENT_FLAGS_CHANGED, keys::OPTION, 0);
        assert_eq!(
            unsafe { decide_key(&mut r, 0, release.0, EVENT_FLAGS_CHANGED, &mut log) },
            Outcome::Pass
        );
        assert_eq!(state(&release), (keys::COMMAND, 0));
    }

    #[test]
    fn ctrl_click_is_rewritten_to_cmd_click() {
        let (tx, _rx) = mpsc::sync_channel(16);
        let shared = Arc::new(Shared::new(tx, false));
        let mut ctx = TapCtx {
            shared: shared.clone(),
            remapper: Remapper::new(Rules::default()),
            opts: TapOptions {
                keys: true,
                dock: false,
                verbose: false,
                swallow_tagged: false,
            },
            was_paused: false,
        };
        // 按住物理 Ctrl：键盘事件已被改写成 ⌘
        let ctl = key(EVENT_FLAGS_CHANGED, keys::CONTROL, CTL_DOWN);
        let mut log = Log::default();
        unsafe { decide_key(&mut ctx.remapper, 0, ctl.0, EVENT_FLAGS_CHANGED, &mut log) };
        assert!(ctx.remapper.ctrl_as_cmd());

        // 合成带 ⌃ 标志的左键按下：应被换成 ⌘ 标志（VS Code 跳定义、浏览器新标签）
        let ev = unsafe {
            let e = CGEventCreateMouseEvent(
                std::ptr::null_mut(),
                EVENT_LEFT_MOUSE_DOWN,
                crate::geometry::CGPoint { x: 100.0, y: 100.0 },
                0,
            );
            CGEventSetFlags(e, CTL_DOWN);
            Cf::owned(e)
        };
        let out = unsafe { ctx.on_mouse(ev.0, EVENT_LEFT_MOUSE_DOWN) };
        assert!(!out.is_null(), "普通点击应放行");
        let f = unsafe { CGEventGetFlags(ev.0) };
        assert_eq!(
            f & (flags::COMMAND | flags::L_CMD),
            flags::COMMAND | flags::L_CMD
        );
        assert_eq!(f & (flags::CONTROL | flags::L_CTL), 0, "⌃ 位应被清掉");
    }

    #[test]
    fn click_without_ctrl_is_untouched() {
        let (tx, _rx) = mpsc::sync_channel(16);
        let shared = Arc::new(Shared::new(tx, false));
        let mut ctx = TapCtx {
            shared: shared.clone(),
            remapper: Remapper::new(Rules::default()),
            opts: TapOptions {
                keys: true,
                dock: false,
                verbose: false,
                swallow_tagged: false,
            },
            was_paused: false,
        };
        let ev = unsafe {
            let e = CGEventCreateMouseEvent(
                std::ptr::null_mut(),
                EVENT_LEFT_MOUSE_DOWN,
                crate::geometry::CGPoint { x: 100.0, y: 100.0 },
                0,
            );
            Cf::owned(e)
        };
        // 合成鼠标事件的 flags 会带上系统当前的修饰键状态（如 CapsLock），
        // 不能断言等于 0；该测试关心的是「没按 Ctrl 时不被改写」
        let before = unsafe { CGEventGetFlags(ev.0) };
        let out = unsafe { ctx.on_mouse(ev.0, EVENT_LEFT_MOUSE_DOWN) };
        assert!(!out.is_null());
        let after = unsafe { CGEventGetFlags(ev.0) };
        assert_eq!(after, before, "没按 Ctrl 时点击原样");
    }

    fn wait_until(mut cond: impl FnMut() -> bool) -> bool {
        let deadline = Instant::now() + Duration::from_secs(3);
        while Instant::now() < deadline {
            if cond() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        cond()
    }

    /// 真机端到端：装上拦截层，投递一组带标记的合成按键，确认
    /// (1) 投递的事件确实经过拦截回调；(2) 自测模式下被截住，不会送达应用；
    /// (3) 投递前后 HID 层的修饰键状态一致（合成事件不留残余标志位）。
    ///   LATERKEY_TEST_E2E=1 cargo test -p laterkey-platform-mac -- --ignored --nocapture
    #[test]
    #[ignore = "真机：需要辅助功能权限，需 LATERKEY_TEST_E2E=1"]
    fn tagged_events_pass_through_the_tap_and_leave_no_flags_behind() {
        if std::env::var("LATERKEY_TEST_E2E").as_deref() != Ok("1") {
            eprintln!("跳过：需 LATERKEY_TEST_E2E=1");
            return;
        }
        assert!(super::super::ax_trusted(), "需要辅助功能权限");

        let (tx, rx) = mpsc::sync_channel(16);
        let shared = Arc::new(Shared::new(tx, false));
        let tap = EventTap::start(
            shared.clone(),
            Remapper::new(Rules::default()),
            // keys/dock 都关：未标记的真实按键原样通过，测试期间不影响正在用电脑的人
            TapOptions {
                keys: false,
                dock: false,
                verbose: false,
                swallow_tagged: true,
            },
        )
        .unwrap();

        let before = super::super::post::hid_flags_state();
        assert!(super::super::post::chord(Chord {
            keycode: keys::F2,
            flags: 0
        }));
        let seen = wait_until(|| shared.stats.tagged.load(Ordering::Relaxed) >= 2);
        let after = super::super::post::hid_flags_state();
        drop(tap);

        eprintln!(
            "HID flags：投递前 {before:#x}，投递后 {after:#x}，回调看到 {} 个带标记事件",
            shared.stats.tagged.load(Ordering::Relaxed)
        );
        assert!(seen, "合成事件没有经过拦截回调");
        // 队列里不该有任何动作：带标记的事件被原样识别，不进状态机
        assert!(rx.try_iter().next().is_none());
        let mods = flags::ANY_MODIFIER | flags::SECONDARY_FN;
        assert_eq!(
            before & mods,
            after & mods,
            "合成事件改变了 HID 层的修饰键状态"
        );
    }
}
