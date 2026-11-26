//! 裸启动入口：托盘驻留 + 重映射后台线程。菜单与设置面板共用一份开关。
//!
//! 结构：
//! - 主线程：eframe/egui（设置窗口是主视口；托盘菜单经 winit 用户事件进来）
//! - 拦截线程 + 后台线程：与 CLI 的 `run` 完全相同
//! - 菜单/面板改开关 → 存 toggles.toml → 通道通知后台线程 → 拦截线程重建状态机
//!
//! 关窗语义：点设置窗口红点只收起窗口（CancelClose + Visible(false)），托盘驻留；
//! 只有托盘「退出 LaterKey」或后台 stop 才真正退出。轮询与命令都在 `App::logic`
//! 里做——窗口隐藏后 eframe 不再调 `ui`，但 `logic` 每帧照跑。

use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use laterkey_core::{Config, DisplayRules, GroupTable, Rules};
use laterkey_platform_mac::PanelCommand;

use crate::cheatsheet::CheatSheetView;
use crate::clipboard_panel::Panel;
use crate::settings::SettingsApp;
use crate::statusbar::{self, Action, Tray};
use crate::toggles::{self, Toggles};

pub fn run() -> Result<(), String> {
    let _instance = crate::instance::InstanceLock::acquire()?;

    if !laterkey_platform_mac::ax_trusted() {
        // 托盘里给入口去开授权；同时弹一次系统请求
        laterkey_platform_mac::request_ax_access();
    }

    let base_config = crate::app::load_base_config()?;
    let toggles = Arc::new(RwLock::new(Toggles::load()));
    let compiled = toggles::apply(
        &base_config,
        &toggles.read().unwrap_or_else(|p| p.into_inner()),
    )?;

    let (tx, rx) = mpsc::sync_channel(64);
    let shared = Arc::new(laterkey_platform_mac::Shared::new(tx, false));
    *shared.rules.write().unwrap_or_else(|p| p.into_inner()) = compiled.rules.clone();
    let (update_tx, update_rx) = mpsc::channel();
    // 面板 → worker：贴回 / 清空
    let (panel_tx, panel_rx) = mpsc::channel();
    let _worker = laterkey_platform_mac::Worker::start(
        shared.clone(),
        rx,
        compiled.groups.clone(),
        // 启动时的生效规则：updates 通道只在用户碰开关后才有货
        compiled.rules.clone(),
        update_rx,
        panel_rx,
    )?;
    let _tap = laterkey_platform_mac::EventTap::start(
        shared.clone(),
        laterkey_core::Remapper::new(compiled.rules.clone()),
        laterkey_platform_mac::TapOptions {
            keys: true,
            dock: compiled.dock_click_minimize,
            verbose: false,
            swallow_tagged: false,
        },
    )?;

    run_event_loop(toggles, update_tx, base_config, shared, panel_tx)
}

fn run_event_loop(
    toggles: Arc<RwLock<Toggles>>,
    update_tx: Sender<(Rules, GroupTable)>,
    base_config: Config,
    shared: Arc<laterkey_platform_mac::Shared>,
    panel_tx: Sender<PanelCommand>,
) -> Result<(), String> {
    let (menu_tx, menu_rx) = mpsc::channel::<tray_icon::menu::MenuEvent>();
    tray_icon::menu::MenuEvent::set_event_handler(Some(move |ev| {
        let _ = menu_tx.send(ev);
    }));

    // eframe 的 run_native 接管主线程；托盘在它的 event loop 里创建
    let viewport = eframe::egui::ViewportBuilder::default()
        .with_inner_size([680.0, 620.0])
        .with_title("LaterKey 设置")
        // GL 上下文的透明度取自根视口这个开关（eframe glow_integration 取
        // native_options.viewport.transparent）——不开它，速览浮窗的
        // with_transparent 只是窗口属性，GL surface 没有 alpha 通道，
        // 圆角外渲染成黑底（2026-11-26 用户反馈「圆角外面是黑的」）。
        // 根窗口内容不透明，开着无副作用。
        .with_transparent(true)
        .with_visible(false); // 默认不显示，点「设置…」才显示

    let options = eframe::NativeOptions {
        viewport,
        event_loop_builder: Some(Box::new(move |builder| {
            // 菜单栏应用：不显示 Dock 图标。方法在扩展 trait 上，要先引入。
            #[cfg(target_os = "macos")]
            {
                use winit::platform::macos::{ActivationPolicy, EventLoopBuilderExtMacOS};
                builder.with_activation_policy(ActivationPolicy::Accessory);
            }
        })),
        ..Default::default()
    };

    let app = TrayApp::new(toggles, update_tx, base_config, shared, menu_rx, panel_tx);
    eframe::run_native(
        "LaterKey",
        options,
        Box::new(move |cc| {
            // 中文字体：默认字体不含 CJK，设置面板会全是方框
            crate::font::setup(&cc.egui_ctx);
            crate::settings::apply_theme(&cc.egui_ctx);
            // 托盘必须在事件循环跑起来之后创建（macOS 全屏应用兼容性）
            cc.egui_ctx.request_repaint();
            Ok(Box::new(app))
        }),
    )
    .map_err(|e| format!("界面退出：{e}"))
}

struct TrayApp {
    toggles: Arc<RwLock<Toggles>>,
    update_tx: Sender<(Rules, GroupTable)>,
    base_config: Config,
    shared: Arc<laterkey_platform_mac::Shared>,
    menu_rx: Receiver<tray_icon::menu::MenuEvent>,
    panel_tx: Sender<PanelCommand>,
    tray: Option<Tray>,
    /// 已明确要退出（托盘「退出」或后台 stop），下一次 close_requested 不再拦截。
    quit_requested: bool,
    inner: Option<SettingsApp>,
    /// Win+V 呼出面板：worker 推 panel_seq，这里轮询。
    /// 首次打开后一直保留（包括隐藏时），避免反复销毁 CGL 视口。
    panel: Option<Panel>,
    panel_open: bool,
    panel_seq_seen: u64,
    /// 快捷键速览浮窗：长按 Ctrl 手势 / 托盘菜单呼出。同 panel 的保留策略。
    cheatsheet: Option<CheatSheetView>,
    cheatsheet_open: bool,
    cheatsheet_seq_seen: u64,
    /// 本次开窗是否手势触发（松开 Ctrl 自动关）+ 开窗时刻的松开计数。
    /// 松开判定放根窗口 logic() 每帧轮询：浮窗 with_active(false) 加载完
    /// 后收不到输入不再重绘，draw() 里的检查永远没机会跑（真机实测踩过）。
    /// 已同步到托盘菜单勾选的开关状态：面板/菜单/热加载任何路径改了
    /// toggles，轮询发现不一致就刷新菜单勾选（设置面板改开关不经过
    /// TrayApp::apply，只靠菜单路径同步会漏）。
    synced: Option<Toggles>,
    /// 全局快捷键唤出设置的序号（worker 推，GUI 轮询）
    settings_seq_seen: u64,
    /// 主窗口 NSWindow 已设回不透明（GL 透明度只能从根视口开，见
    /// [`force_root_window_opaque`]；一次性）
    root_opaque_done: bool,
}

impl TrayApp {
    fn new(
        toggles: Arc<RwLock<Toggles>>,
        update_tx: Sender<(Rules, GroupTable)>,
        base_config: Config,
        shared: Arc<laterkey_platform_mac::Shared>,
        menu_rx: Receiver<tray_icon::menu::MenuEvent>,
        panel_tx: Sender<PanelCommand>,
    ) -> Self {
        Self {
            toggles,
            update_tx,
            base_config,
            shared,
            menu_rx,
            panel_tx,
            tray: None,
            quit_requested: false,
            inner: None,
            panel: None,
            panel_open: false,
            panel_seq_seen: 0,
            cheatsheet: None,
            cheatsheet_open: false,
            cheatsheet_seq_seen: 0,
            synced: None,
            settings_seq_seen: 0,
            root_opaque_done: false,
        }
    }

    fn apply(&self) {
        let t = self
            .toggles
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .clone();
        t.save();
        if let Ok(compiled) = toggles::apply(&self.base_config, &t) {
            let _ = self.update_tx.send((compiled.rules, compiled.groups));
        }
        if let Some(tray) = &self.tray {
            tray.sync(&t);
        }
    }
}

impl eframe::App for TrayApp {
    // 速览浮窗是透明无边框窗口：清屏必须全透明，自绘圆角外才露出桌面。
    // 其余窗口都有不透明填充盖满整窗，不受影响
    fn clear_color(&self, _visuals: &eframe::egui::Visuals) -> [f32; 4] {
        [0.0, 0.0, 0.0, 0.0]
    }

    // 菜单轮询、关闭拦截、退出命令全放 logic()：窗口被隐藏（macOS orderOut → Occluded）
    // 后 eframe glow 不再调用 ui()（run_ui 由 info.occluded 决定），但 logic() 无条件执行，
    // 托盘才能在窗口收起时继续响应、退出路径也保持可用。
    fn logic(&mut self, ctx: &eframe::egui::Context, frame: &mut eframe::Frame) {
        // 首帧把主窗口 NSWindow 设回不透明（一次性）
        if !self.root_opaque_done {
            self.root_opaque_done = true;
            if let Some(win) = frame.winit_window() {
                force_root_window_opaque(win);
            }
        }

        // 收菜单事件
        while let Ok(ev) = self.menu_rx.try_recv() {
            if let Some(action) = statusbar::translate(&ev, &self.toggles) {
                match action {
                    Action::Toggle(field, value) => {
                        {
                            let mut t = self.toggles.write().unwrap_or_else(|p| p.into_inner());
                            set_field(&mut t, &field, value);
                        }
                        self.apply();
                    }
                    Action::OpenSettings => {
                        ctx.send_viewport_cmd(eframe::egui::ViewportCommand::Visible(true));
                        ctx.send_viewport_cmd(eframe::egui::ViewportCommand::Focus);
                    }
                    Action::ShortcutsPeek => {
                        self.shared
                            .cheatsheet_seq
                            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    }
                    Action::OpenAccessibility => statusbar::open_accessibility(),
                    Action::Quit => {
                        self.shared
                            .stop
                            .store(true, std::sync::atomic::Ordering::SeqCst);
                        // 先标记退出意图，否则下一帧 close_requested 会被下面的拦截当成点红点吞掉
                        self.quit_requested = true;
                        ctx.send_viewport_cmd(eframe::egui::ViewportCommand::Close);
                    }
                }
            }
        }

        // 点窗口红点 → 只收起窗口，不退出（托盘驻留）。
        // CancelClose 必须与 close_requested 同帧进入 viewport_output 才生效；
        // logic 每帧执行、命令进本帧输出，恰好满足时效。close_requested 是一次性事件
        // （eframe 每帧末清空视口事件），拦截不会卡住。
        if ctx.input(|i| i.viewport().close_requested()) && !self.quit_requested {
            ctx.send_viewport_cmd(eframe::egui::ViewportCommand::CancelClose);
            ctx.send_viewport_cmd(eframe::egui::ViewportCommand::Visible(false));
        }

        // 停止标志（后台 worker 退出）→ 真退出
        if self.shared.stop.load(std::sync::atomic::Ordering::SeqCst) {
            self.quit_requested = true;
            ctx.send_viewport_cmd(eframe::egui::ViewportCommand::Close);
        }

        // 托盘勾选跟随 toggles：设置面板的改动不经过 TrayApp::apply，
        // 在这里轮询兜底，保证任何路径改动后菜单勾选都一致
        if let Some(tray) = &self.tray {
            let current = self
                .toggles
                .read()
                .unwrap_or_else(|p| p.into_inner())
                .clone();
            if self.synced.as_ref() != Some(&current) {
                tray.sync(&current);
                tray.set_visible(!current.hide_tray_icon);
                self.synced = Some(current);
            }
        }

        // 全局快捷键（Ctrl+Alt+K）唤出设置：与托盘「设置…」同一处理
        let settings_seq = self
            .shared
            .settings_seq
            .load(std::sync::atomic::Ordering::Relaxed);
        if settings_seq != self.settings_seq_seen {
            self.settings_seq_seen = settings_seq;
            ctx.send_viewport_cmd(eframe::egui::ViewportCommand::Visible(true));
            ctx.send_viewport_cmd(eframe::egui::ViewportCommand::Focus);
        }

        // Win+V 是开关（toggle）：面板关着 → 呼出；开着 → 再按一次关闭。
        // 关闭走 Panel::request_close，与 Esc/失焦同一路径。
        let seq = self
            .shared
            .panel_seq
            .load(std::sync::atomic::Ordering::Relaxed);
        if seq != self.panel_seq_seen {
            self.panel_seq_seen = seq;
            if self.panel_open {
                if let Some(panel) = &self.panel {
                    panel.request_close();
                }
            } else {
                // 先快照前台 pid：面板一激活，前台就变成 LaterKey 自己了。
                // worker 贴回时要的就是这个快照
                let pid = self
                    .shared
                    .front_pid
                    .load(std::sync::atomic::Ordering::Relaxed);
                self.shared
                    .panel_target_pid
                    .store(pid, std::sync::atomic::Ordering::Relaxed);
                let panel = self
                    .panel
                    .get_or_insert_with(|| Panel::new(self.panel_tx.clone(), self.shared.clone()));
                panel.prepare_open();
                self.panel_open = true;
                ctx.request_repaint();
            }
        }
        if let Some(panel) = &self.panel {
            if self.panel_open && panel.close_requested() {
                self.panel_open = false;
                // 焦点还给打开面板前的应用：面板激活过 LaterKey，若设置窗口
                // 之前开过会被一起带到前台；Esc/再按 Win+V 关闭后必须把原应用
                // 拉回来，否则设置窗口就残留在最前面（贴回路径 worker 也激活
                // 同一目标，幂等）
                let pid = self
                    .shared
                    .panel_target_pid
                    .load(std::sync::atomic::Ordering::Relaxed);
                if pid > 0 {
                    laterkey_platform_mac::activate_pid(pid);
                }
            }
            // 隐藏时也持续注册同一视口：销毁其 NSView 后再复用 GL context，
            // glutin CGL 会在 is_view_current 的 expect 处 panic（Win+V 连按崩溃）。
            panel.show(ctx, self.panel_open);
        }

        // 快捷键速览：长按 Ctrl 手势 / 托盘菜单推进 seq；tap 在 Ctrl 抬起
        // 时推进 release（浮窗据此自动关）。视口同 panel 的保留策略：关闭只隐藏
        let peek_seq = self
            .shared
            .cheatsheet_seq
            .load(std::sync::atomic::Ordering::Relaxed);
        if peek_seq != self.cheatsheet_seq_seen {
            self.cheatsheet_seq_seen = peek_seq;
            // peek_since 非 0 = 手势按住中开的窗（松开自动关）；菜单开的不受影响
            let release = self
                .shared
                .cheatsheet_release
                .load(std::sync::atomic::Ordering::Relaxed);
            let pid = self
                .shared
                .cheatsheet_target_pid
                .load(std::sync::atomic::Ordering::Relaxed);
            let visual = self.base_config.cheatsheet.clone();
            let view = self
                .cheatsheet
                .get_or_insert_with(|| CheatSheetView::new(self.shared.clone(), visual));
            view.prepare_open(pid, release);
            self.cheatsheet_open = true;
            // AX 读菜单可能上百毫秒：后台线程读，浮窗先显示旧数据/读取中
            crate::cheatsheet::fetch_in_background(self.shared.clone(), pid);
            ctx.request_repaint();
        }
        if let Some(view) = &self.cheatsheet {
            if self.cheatsheet_open && view.close_requested() {
                self.cheatsheet_open = false;
            }
            let t = self.toggles.read().unwrap_or_else(|p| p.into_inner());
            let display = DisplayRules {
                ctrl_as_cmd: t.ctrl_as_cmd,
                cmd_as_ctrl: t.cmd_as_ctrl,
            };
            view.show(ctx, self.cheatsheet_open, display);
        }

        // 菜单事件要能被及时看到：低频轮询（窗口隐藏时也保持）
        ctx.request_repaint_after(Duration::from_millis(200));
    }

    fn ui(&mut self, ui: &mut eframe::egui::Ui, frame: &mut eframe::Frame) {
        // 托盘第一次有机会就建（事件循环已在跑）
        if self.tray.is_none() {
            let t = self
                .toggles
                .read()
                .unwrap_or_else(|p| p.into_inner())
                .clone();
            match Tray::new(&t) {
                Ok(tray) => self.tray = Some(tray),
                Err(e) => eprintln!("[laterkey] {e}"),
            }
        }

        // 设置窗口内容（只在可见时才有意义）
        if self.inner.is_none() {
            self.inner = Some(SettingsApp::new(
                self.toggles.clone(),
                self.update_tx.clone(),
                self.base_config.clone(),
            ));
        }
        if let Some(inner) = &mut self.inner {
            eframe::App::ui(inner, ui, frame);
        }
    }
}

/// 把主窗口的 NSWindow 设回不透明。
///
/// 背景矛盾：速览浮窗的圆角外要透出桌面，GL surface 必须带 alpha 通道；
/// 而 eframe 的 GL 上下文透明度只认 `native_options.viewport.transparent`
/// （glow_integration 直接读根视口这个字段）——开了它，主窗口的 NSWindow
/// 也被建成非不透明，设置页标题栏跟着变透明（2026-11-26 用户反馈）。这里
/// 在首帧用 ObjC 把主窗口 NSWindow 的 isOpaque 设回 YES。winit 0.30 不再
/// 直接暴露 ns_window，走 raw-window-handle 拿 NSView 再向 AppKit 要 window。
/// 速览浮窗是独立 NSWindow（自己的 with_transparent），不受影响。
fn force_root_window_opaque(win: &winit::window::Window) {
    use objc2::msg_send;
    use objc2::runtime::{AnyObject, Bool};
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};

    let Ok(handle) = win.window_handle() else {
        return;
    };
    let RawWindowHandle::AppKit(appkit) = handle.as_raw() else {
        return;
    };
    // SAFETY: ns_view 是 winit 创建的有效 NSView 指针；向它要 window 并
    // 调 setOpaque:/setBackgroundColor: 是标准 AppKit 消息。两处都要还原：
    // winit 的 transparent 属性除了 setOpaque(false) 还把背景色设成了
    // clearColor——不还原背景色，标准标题栏仍画在透明底上
    unsafe {
        let ns_view = appkit.ns_view.as_ptr() as *mut AnyObject;
        if ns_view.is_null() {
            return;
        }
        let window: *mut AnyObject = msg_send![ns_view, window];
        if window.is_null() {
            return;
        }
        let _: () = msg_send![window, setOpaque: Bool::new(true)];
        let Some(ns_color) = objc2::runtime::AnyClass::get(c"NSColor") else {
            return;
        };
        let bg: *mut AnyObject = msg_send![ns_color, windowBackgroundColor];
        if !bg.is_null() {
            let _: () = msg_send![window, setBackgroundColor: bg];
        }
    }
}

fn set_field(t: &mut Toggles, field: &str, value: bool) {
    match field {
        "master" => t.master = value,
        "ctrl_as_cmd" => t.ctrl_as_cmd = value,
        "cmd_as_ctrl" => t.cmd_as_ctrl = value,
        "alt_tab" => t.alt_tab = value,
        "ctrl_arrows" => t.ctrl_arrows = value,
        "win_key_lock" => t.win_key_lock = value,
        "win_key_clipboard" => t.win_key_clipboard = value,
        "task_manager" => t.task_manager = value,
        "settings_hotkey" => t.settings_hotkey = value,
        "shortcuts_peek" => t.shortcuts_peek = value,
        "dock_click_minimize" => t.dock_click_minimize = value,
        "finder_f2" => t.finder_f2 = value,
        "browser_f5" => t.browser_f5 = value,
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::Ordering;

    fn test_app() -> TrayApp {
        let (jobs_tx, _jobs_rx) = mpsc::sync_channel(1);
        let shared = Arc::new(laterkey_platform_mac::Shared::new(jobs_tx, false));
        let (update_tx, _update_rx) = mpsc::channel();
        let (_menu_tx, menu_rx) = mpsc::channel();
        let (panel_tx, _panel_rx) = mpsc::channel();
        TrayApp::new(
            Arc::new(RwLock::new(Toggles::default())),
            update_tx,
            Config::default(),
            shared,
            menu_rx,
            panel_tx,
        )
    }

    fn press_win_v(app: &mut TrayApp, ctx: &eframe::egui::Context, frame: &mut eframe::Frame) {
        app.shared.panel_seq.fetch_add(1, Ordering::Relaxed);
        ctx.begin_pass(Default::default());
        eframe::App::logic(app, ctx, frame);
        let _ = ctx.end_pass();
    }

    #[test]
    fn repeated_win_v_reuses_the_panel_viewport() {
        let mut app = test_app();
        let ctx = eframe::egui::Context::default();
        let mut frame = eframe::Frame::_new_kittest();

        for _ in 0..20 {
            press_win_v(&mut app, &ctx, &mut frame);
            let panel = app.panel.as_ref().expect("Win+V 应打开面板");
            assert!(!panel.close_requested(), "重开时必须清掉旧的关闭请求");

            press_win_v(&mut app, &ctx, &mut frame);
            assert!(
                app.panel.is_some(),
                "关闭只能隐藏视口；销毁后重建会让 glutin CGL 持有失效的 view"
            );
        }
    }
}
