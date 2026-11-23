//! 子命令实现：`run` / `doctor` / `lock`。

use std::path::{Path, PathBuf};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

use clap::{Parser, Subcommand, ValueEnum};
use laterkey_core::{groups, Compiled, Config, Remapper, Rules};
use laterkey_platform_mac as mac;

use crate::{daemon, instance};

#[derive(Parser)]
#[command(
    name = "laterkey",
    version,
    about = "让 Windows 布局的键盘在 macOS 上按 Windows 习惯工作"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// 启动重映射。原型阶段默认 5 分钟后自动退出，--forever 取消
    Run {
        /// 配置文件；缺省读 ~/.config/laterkey/config.toml，没有就用默认规则
        #[arg(short, long)]
        config: Option<PathBuf>,
        /// 多久后自动退出，如 30s / 5m / 1h
        #[arg(long = "for", default_value = "5m")]
        duration: String,
        /// 一直运行到 Ctrl+C
        #[arg(long)]
        forever: bool,
        /// 只启用某一部分；none 是自检——装上拦截层但不改任何事件
        #[arg(long, value_enum, default_value_t = Only::All)]
        only: Only,
        /// 逐条打印改写与吞掉的事件（只含键码和标志位，不含输入内容）
        #[arg(short, long)]
        verbose: bool,
    },
    /// 只读诊断：权限、前台应用、Dock 图标、锁屏通路
    Doctor {
        #[arg(short, long)]
        config: Option<PathBuf>,
        /// 弹出系统的辅助功能授权请求
        #[arg(long)]
        request_access: bool,
    },
    /// 立即锁屏
    Lock,
    /// 安装常驻：复制二进制到固定位置，装 LaunchAgent（登录自启 + 崩溃拉起）
    Install,
    /// 卸载常驻（LaunchAgent 与复制的二进制）
    Uninstall,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
enum Only {
    All,
    Keys,
    Dock,
    /// 自检：验证权限、线程与收尾，不改任何事件
    None,
}

pub fn run() -> Result<(), String> {
    match Cli::parse().command {
        Command::Run {
            config,
            duration,
            forever,
            only,
            verbose,
        } => run_remap(config.as_deref(), &duration, forever, only, verbose),
        Command::Doctor {
            config,
            request_access,
        } => doctor(config.as_deref(), request_access),
        Command::Lock => lock_now(),
        Command::Install => daemon::install().map(|msg| println!("{msg}")),
        Command::Uninstall => daemon::uninstall().map(|msg| println!("{msg}")),
    }
}

// ---------------------------------------------------------------- run

fn run_remap(
    config: Option<&Path>,
    duration: &str,
    forever: bool,
    only: Only,
    verbose: bool,
) -> Result<(), String> {
    let (compiled, source) = load_config(config)?;
    let limit = if forever {
        None
    } else {
        Some(
            crate::duration::parse(duration)
                .ok_or_else(|| format!("--for 的值看不懂：`{duration}`，请写成 30s / 5m / 1h"))?,
        )
    };

    // 两个 laterkey 同时跑会互相改写事件，必须先抢锁
    let _instance = instance::InstanceLock::acquire()?;

    let host = mac::host_app();
    let group_table = compiled.groups.clone();

    if !mac::listen_allowed() {
        return Err(
            "没有辅助功能权限：到 系统设置 › 隐私与安全性 › 辅助功能 里勾选运行本命令的应用。\
             也可以运行 `laterkey doctor --request-access` 让系统弹出请求"
                .into(),
        );
    }

    let (tx, rx) = mpsc::sync_channel(64);
    let shared = Arc::new(mac::Shared::new(tx, verbose));
    // CLI 模式下规则不热更新：通道的发送端立刻丢弃，后台线程的 try_recv 永远为空
    let (_update_tx, update_rx) = mpsc::channel();
    let worker = mac::Worker::start(shared.clone(), rx, group_table, update_rx)?;
    let tap = mac::EventTap::start(
        shared.clone(),
        Remapper::new(compiled.rules.clone()),
        mac::TapOptions {
            keys: matches!(only, Only::All | Only::Keys),
            dock: matches!(only, Only::All | Only::Dock) && compiled.dock_click_minimize,
            verbose,
            swallow_tagged: false,
        },
    )?;

    eprintln!(
        "laterkey 已启动（{}）",
        match &source {
            Source::File(path) => format!("配置 {}", path.display()),
            Source::Defaults => "默认规则，没有找到配置文件".to_string(),
        }
    );
    if matches!(only, Only::All | Only::Keys) {
        for (name, on) in key_rules(&compiled.rules) {
            eprintln!("  {} {name}", on_off(on));
        }
        for sub in &compiled.rules.substitutes {
            eprintln!(
                "  开 裸 {} → {}（仅 {}）",
                key_name(sub.keycode),
                chord_name(&sub.to),
                group_name(sub.groups)
            );
        }
    }
    if matches!(only, Only::All | Only::Dock) {
        eprintln!(
            "  {} 再点一次前台应用的 Dock 图标 → 最小化",
            on_off(compiled.dock_click_minimize)
        );
    }
    if only == Only::None {
        eprintln!("  自检模式：拦截层已装上，但不改任何事件");
    }
    match &host {
        Some(host) => eprintln!("  从 {} 里启动：终端中断用 Win+C（=⌃C）", host.bundle_id),
        None => eprintln!("  不在应用内运行：到点自动退出"),
    }
    match limit {
        Some(limit) => eprintln!(
            "{} 后自动退出，Win+C（=⌃C）可提前退出。",
            crate::duration::human(limit)
        ),
        None => eprintln!("Win+C（=⌃C）退出。"),
    }

    let (stop_tx, stop_rx) = mpsc::channel::<()>();
    ctrlc::set_handler(move || {
        let _ = stop_tx.send(());
    })
    .map_err(|e| format!("装不上 Ctrl+C 处理：{e}"))?;

    let deadline = limit.map(|limit| Instant::now() + limit);
    let reason = loop {
        let wait = deadline.map_or(Duration::from_millis(200), |d| {
            d.saturating_duration_since(Instant::now())
                .max(Duration::from_millis(20))
        });
        match stop_rx.recv_timeout(wait) {
            Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => break "收到中断（⌃C）",
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if deadline.is_some_and(|d| Instant::now() >= d) {
                    break "到了设定的时间";
                }
            }
        }
    };

    drop(tap);
    drop(worker);
    eprintln!("\n[laterkey] {reason}，已退出。{}", shared.stats.summary());
    Ok(())
}

// ---------------------------------------------------------------- doctor

fn doctor(config: Option<&Path>, request_access: bool) -> Result<(), String> {
    if request_access {
        // 系统只会弹一次；这里只负责触发，结果看下面的权限项
        mac::request_listen_access();
    }
    let yes_no = |ok: bool| if ok { "有" } else { "无" };
    println!(
        "辅助功能（拦截键盘事件）：{}",
        yes_no(mac::listen_allowed())
    );
    println!("发送合成事件：{}", yes_no(mac::post_allowed()));
    println!(
        "辅助功能 API（读 Dock 图标、最小化窗口）：{}",
        yes_no(mac::ax_trusted())
    );
    println!(
        "安全输入：{}",
        if mac::secure_input_enabled() {
            "开启（此时映射会暂停）"
        } else {
            "关闭"
        }
    );

    match mac::host_app() {
        Some(host) => println!(
            "宿主应用：{}（运行 laterkey run 时按终端处理）",
            host.bundle_id
        ),
        None => println!("宿主应用：无（不是从某个应用里启动的）"),
    }
    match mac::frontmost() {
        Some(front) => println!("前台应用：{}（pid {}）", front.bundle_id, front.pid),
        None => println!("前台应用：读不到"),
    }
    println!(
        "锁屏：{}",
        if mac::immediate_available() {
            "SACLockScreenImmediate 可用"
        } else {
            "私有 API 不可用，会退到 ⌃⌘Q"
        }
    );

    match mac::dock_pid() {
        Some(pid) => {
            let tiles = mac::dock_tiles();
            println!("Dock（pid {pid}）：读到 {} 个运行中应用的图标", tiles.len());
            for tile in &tiles {
                println!(
                    "  {:<24} pid {:>6}   ({}, {} {}×{})",
                    tile.title,
                    tile.pid,
                    tile.rect.origin.x,
                    tile.rect.origin.y,
                    tile.rect.size.width,
                    tile.rect.size.height
                );
            }
        }
        None => println!("Dock：找不到进程"),
    }

    let (compiled, source) = load_config(config)?;
    match &source {
        Source::File(path) => println!("配置文件：{}（已生效）", path.display()),
        Source::Defaults => println!(
            "配置文件：没有，使用默认规则。要自定义就创建 {}",
            default_config_path()
                .map(|p| p.display().to_string())
                .unwrap_or_default()
        ),
    }
    for (name, on) in key_rules(&compiled.rules) {
        println!("  {} {name}", on_off(on));
    }
    println!(
        "  {} 再点一次前台应用的 Dock 图标 → 最小化",
        on_off(compiled.dock_click_minimize)
    );
    for sub in &compiled.rules.substitutes {
        println!(
            "  开 裸 {} → {}（仅 {}）",
            key_name(sub.keycode),
            chord_name(&sub.to),
            group_name(sub.groups)
        );
    }
    Ok(())
}

// ---------------------------------------------------------------- lock

fn lock_now() -> Result<(), String> {
    match mac::lock_screen()? {
        mac::LockMethod::Immediate => Ok(()),
        mac::LockMethod::Hotkey => {
            eprintln!("[laterkey] 私有 API 不可用，已发送 ⌃⌘Q");
            Ok(())
        }
    }
}

// ---------------------------------------------------------------- 配置与展示

#[derive(Debug)]
enum Source {
    File(PathBuf),
    Defaults,
}

/// 供 GUI 用的基础配置（不含开关叠加）。
pub fn load_base_config() -> Result<Config, String> {
    let path = default_config_path().filter(|p| p.exists());
    match path {
        Some(path) => {
            let text = std::fs::read_to_string(&path)
                .map_err(|e| format!("读不了配置文件 {}：{e}", path.display()))?;
            laterkey_core::parse(&text)
        }
        None => Ok(Config::default()),
    }
}

fn load_config(path: Option<&Path>) -> Result<(Compiled, Source), String> {
    load_from(resolve_config_path(path))
}

/// 指定了就用指定的；没指定就用默认路径（文件不存在则返回 None，即默认规则）。
fn resolve_config_path(path: Option<&Path>) -> Option<PathBuf> {
    match path {
        Some(path) => Some(path.to_path_buf()),
        None => default_config_path().filter(|path| path.exists()),
    }
}

fn load_from(path: Option<PathBuf>) -> Result<(Compiled, Source), String> {
    let Some(path) = path else {
        return Ok((
            laterkey_core::compile(&Config::default())?,
            Source::Defaults,
        ));
    };
    let text = std::fs::read_to_string(&path)
        .map_err(|e| format!("读不了配置文件 {}：{e}", path.display()))?;
    let config = laterkey_core::parse(&text).map_err(|e| format!("{}：{e}", path.display()))?;
    Ok((laterkey_core::compile(&config)?, Source::File(path)))
}

fn default_config_path() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("XDG_CONFIG_HOME").filter(|v| !v.is_empty()) {
        Some(PathBuf::from(dir).join("laterkey").join("config.toml"))
    } else {
        std::env::var_os("HOME")
            .map(|home| PathBuf::from(home).join(".config/laterkey/config.toml"))
    }
}

fn key_rules(rules: &Rules) -> Vec<(&'static str, bool)> {
    vec![
        (
            "物理 Ctrl 当作 ⌘（terminals 组里保持 ⌃）",
            rules.ctrl_as_cmd,
        ),
        (
            "物理 Win(⌘) 当作 ⌃：终端里的中断键（Win+C = Ctrl+C）",
            rules.cmd_as_ctrl,
        ),
        ("左 Alt+Tab 切换应用", rules.alt_tab),
        ("Ctrl+←/→/⌫ 按词跳转/删除（⌥）", rules.ctrl_arrows),
        ("Win(⌘)+L 锁屏", rules.win_key_lock),
        ("Win(⌘)+V 剪贴板历史", rules.win_key_clipboard),
    ]
}

fn on_off(on: bool) -> &'static str {
    if on {
        "开"
    } else {
        "关"
    }
}

fn group_name(mask: u64) -> String {
    let mut names = Vec::new();
    if mask & groups::TERMINAL != 0 {
        names.push("终端");
    }
    if mask & groups::BROWSER != 0 {
        names.push("浏览器");
    }
    if mask & groups::FINDER != 0 {
        names.push("访达");
    }
    if mask & groups::PASSTHROUGH != 0 {
        names.push("透传应用");
    }
    names.join("/")
}

fn key_name(keycode: i64) -> String {
    laterkey_core::keys::NAMED
        .iter()
        .find(|(_, code)| *code == keycode)
        .map(|(name, _)| name.to_uppercase())
        .unwrap_or_else(|| format!("键码 {keycode}"))
}

fn chord_name(chord: &laterkey_core::Chord) -> String {
    let mut out = String::new();
    if chord.flags & laterkey_core::flags::CONTROL != 0 {
        out.push('⌃');
    }
    if chord.flags & laterkey_core::flags::ALTERNATE != 0 {
        out.push('⌥');
    }
    if chord.flags & laterkey_core::flags::SHIFT != 0 {
        out.push('⇧');
    }
    if chord.flags & laterkey_core::flags::COMMAND != 0 {
        out.push('⌘');
    }
    out.push_str(&key_name(chord.keycode));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use laterkey_core::Chord;

    #[test]
    fn names_are_readable() {
        assert_eq!(key_name(0x08), "C");
        assert_eq!(key_name(0x78), "F2");
        assert_eq!(key_name(0x24), "RETURN");
        assert_eq!(key_name(9999), "键码 9999");
        assert_eq!(
            chord_name(&Chord {
                keycode: 0x0F,
                flags: laterkey_core::flags::COMMAND
            }),
            "⌘R"
        );
        assert_eq!(
            chord_name(&Chord {
                keycode: 0x24,
                flags: 0
            }),
            "RETURN"
        );
        assert_eq!(
            chord_name(&Chord {
                keycode: 0x08,
                flags: laterkey_core::flags::CONTROL
                    | laterkey_core::flags::SHIFT
                    | laterkey_core::flags::ALTERNATE
                    | laterkey_core::flags::COMMAND
            }),
            "⌃⌥⇧⌘C"
        );
    }

    #[test]
    fn group_names_join_every_match() {
        assert_eq!(group_name(groups::TERMINAL), "终端");
        assert_eq!(group_name(groups::BROWSER | groups::FINDER), "浏览器/访达");
        assert_eq!(group_name(0), "");
    }

    #[test]
    fn no_config_file_means_default_rules() {
        let (compiled, source) = load_from(None).unwrap();
        assert!(matches!(source, Source::Defaults));
        assert_eq!(
            compiled,
            laterkey_core::compile(&Config::default()).unwrap()
        );
    }

    #[test]
    fn config_file_is_loaded_and_bad_config_is_an_error() {
        let dir = std::env::temp_dir().join(format!("laterkey-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");

        std::fs::write(&path, laterkey_core::config::EXAMPLE).unwrap();
        let (compiled, source) = load_from(Some(path.clone())).unwrap();
        assert_eq!(compiled.rules.substitutes.len(), 2);
        assert!(matches!(source, Source::File(p) if p == path));

        // 写错的配置不能带着跑：必须报错，并指出是哪个文件的哪个字段
        std::fs::write(&path, "[remap]\nctrl_to_comand = true\n").unwrap();
        let err = load_from(Some(path.clone())).unwrap_err();
        assert!(err.contains("ctrl_to_comand"), "{err}");
        assert!(err.contains("config.toml"), "{err}");

        // 指定了却读不到：同样报错，不悄悄退回默认规则
        let missing = dir.join("nope.toml");
        let err = load_from(Some(missing.clone())).unwrap_err();
        assert!(err.contains("nope.toml"), "{err}");

        std::fs::remove_dir_all(&dir).unwrap();
    }
}
