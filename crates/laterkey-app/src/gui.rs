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

use laterkey_core::{Config, GroupTable, Rules};
use laterkey_platform_mac::PanelCommand;

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
    /// Win+V 呼出面板：worker 推 panel_seq，这里轮询。None = 面板关着。
    panel: Option<Panel>,
    panel_seq_seen: u64,
    /// 已同步到托盘菜单勾选的开关状态：面板/菜单/热加载任何路径改了
    /// toggles，轮询发现不一致就刷新菜单勾选（设置面板改开关不经过
    /// TrayApp::apply，只靠菜单路径同步会漏）。
    synced: Option<Toggles>,
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
            panel_seq_seen: 0,
            synced: None,
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
    // 菜单轮询、关闭拦截、退出命令全放 logic()：窗口被隐藏（macOS orderOut → Occluded）
    // 后 eframe glow 不再调用 ui()（run_ui 由 info.occluded 决定），但 logic() 无条件执行，
    // 托盘才能在窗口收起时继续响应、退出路径也保持可用。
    fn logic(&mut self, ctx: &eframe::egui::Context, _frame: &mut eframe::Frame) {
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
                self.synced = Some(current);
            }
        }

        // Win+V 是开关（toggle）：面板关着 → 呼出；开着 → 再按一次关闭。
        // 关闭走 Panel::request_close，与 Esc/失焦同一路径。
        let seq = self
            .shared
            .panel_seq
            .load(std::sync::atomic::Ordering::Relaxed);
        if seq != self.panel_seq_seen {
            self.panel_seq_seen = seq;
            match &self.panel {
                Some(panel) => panel.request_close(),
                None => {
                    // 先快照前台 pid：面板一激活，前台就变成 LaterKey 自己了。
                    // worker 贴回时要的就是这个快照
                    let pid = self
                        .shared
                        .front_pid
                        .load(std::sync::atomic::Ordering::Relaxed);
                    self.shared
                        .panel_target_pid
                        .store(pid, std::sync::atomic::Ordering::Relaxed);
                    self.panel = Some(Panel::new(self.panel_tx.clone(), self.shared.clone()));
                    ctx.request_repaint();
                }
            }
        }
        // 开着就每帧注册一次视口；回调请求关闭后停注册，egui 随即收走窗口
        if let Some(panel) = &self.panel {
            panel.show(ctx);
            if panel.close_requested() {
                self.panel = None;
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

fn set_field(t: &mut Toggles, field: &str, value: bool) {
    match field {
        "master" => t.master = value,
        "ctrl_as_cmd" => t.ctrl_as_cmd = value,
        "cmd_as_ctrl" => t.cmd_as_ctrl = value,
        "alt_tab" => t.alt_tab = value,
        "ctrl_arrows" => t.ctrl_arrows = value,
        "win_key_lock" => t.win_key_lock = value,
        "win_key_clipboard" => t.win_key_clipboard = value,
        "dock_click_minimize" => t.dock_click_minimize = value,
        "finder_f2" => t.finder_f2 = value,
        "browser_f5" => t.browser_f5 = value,
        _ => {}
    }
}
