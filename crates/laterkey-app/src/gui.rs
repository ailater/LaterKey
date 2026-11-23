//! 裸启动入口：托盘驻留 + 重映射后台线程。菜单与设置面板共用一份开关。
//!
//! 结构：
//! - 主线程：eframe/egui（设置窗口是主视口；托盘菜单经 winit 用户事件进来）
//! - 拦截线程 + 后台线程：与 CLI 的 `run` 完全相同
//! - 菜单/面板改开关 → 存 toggles.toml → 通道通知后台线程 → 拦截线程重建状态机

use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use laterkey_core::{Config, GroupTable, Rules};

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
    let _worker = laterkey_platform_mac::Worker::start(
        shared.clone(),
        rx,
        compiled.groups.clone(),
        update_rx,
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

    run_event_loop(toggles, update_tx, base_config, shared)
}

fn run_event_loop(
    toggles: Arc<RwLock<Toggles>>,
    update_tx: Sender<(Rules, GroupTable)>,
    base_config: Config,
    shared: Arc<laterkey_platform_mac::Shared>,
) -> Result<(), String> {
    let (menu_tx, menu_rx) = mpsc::channel::<tray_icon::menu::MenuEvent>();
    tray_icon::menu::MenuEvent::set_event_handler(Some(move |ev| {
        let _ = menu_tx.send(ev);
    }));

    // eframe 的 run_native 接管主线程；托盘在它的 event loop 里创建
    let viewport = eframe::egui::ViewportBuilder::default()
        .with_inner_size([380.0, 520.0])
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

    let app = TrayApp::new(toggles, update_tx, base_config, shared, menu_rx);
    eframe::run_native(
        "LaterKey",
        options,
        Box::new(move |cc| {
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
    tray: Option<Tray>,
    settings_visible: bool,
    inner: Option<SettingsApp>,
}

impl TrayApp {
    fn new(
        toggles: Arc<RwLock<Toggles>>,
        update_tx: Sender<(Rules, GroupTable)>,
        base_config: Config,
        shared: Arc<laterkey_platform_mac::Shared>,
        menu_rx: Receiver<tray_icon::menu::MenuEvent>,
    ) -> Self {
        Self {
            toggles,
            update_tx,
            base_config,
            shared,
            menu_rx,
            tray: None,
            settings_visible: false,
            inner: None,
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
                        self.settings_visible = true;
                        ui.ctx()
                            .send_viewport_cmd(eframe::egui::ViewportCommand::Visible(true));
                        ui.ctx()
                            .send_viewport_cmd(eframe::egui::ViewportCommand::Focus);
                    }
                    Action::OpenAccessibility => statusbar::open_accessibility(),
                    Action::Quit => {
                        self.shared
                            .stop
                            .store(true, std::sync::atomic::Ordering::SeqCst);
                        ui.ctx()
                            .send_viewport_cmd(eframe::egui::ViewportCommand::Close);
                    }
                }
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

        // 停止标志 → 退出
        if self.shared.stop.load(std::sync::atomic::Ordering::SeqCst) {
            ui.ctx()
                .send_viewport_cmd(eframe::egui::ViewportCommand::Close);
        }

        // 菜单事件要能被及时看到：低频轮询
        ui.ctx().request_repaint_after(Duration::from_millis(200));
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
