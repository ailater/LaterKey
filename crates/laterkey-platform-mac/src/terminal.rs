//! 访达「在此处打开终端」：问 Finder 要当前文件夹 → 按配置拉起终端。
//!
//! osascript 与 `open` 都是上百毫秒级的子进程，Finder 卡住时还会更久，
//! 全程放在独立线程，绝不占 worker 主循环（那里停一拍，剪贴板采集、前台
//! 跟踪、Dock 缓存全都停摆）。拉起失败回退 Terminal.app——「至少开了个
//! 终端」比「配错了什么都不发生」可用；回退也失败就只留日志。
//!
//! 问 Finder 走 AppleScript（`target of front window`），需要系统「自动化」
//! 授权（控制 Finder），拒绝时拿不到路径、只记日志。
//! `auto` 预设在拉起时解析：装了 LaterTerm 用 LaterTerm，否则系统自带终端。

use std::process::Command;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use laterkey_core::TerminalOpen;
use objc2::rc::autoreleasepool;
use objc2_app_kit::NSWorkspace;
use objc2_foundation::NSString;

/// LaterTerm 的 bundle id（auto 解析与候选列表共用）。
const LATERTERM_BUNDLE: &str = "io.github.crazykun.LaterTerm";

/// 候选终端：(bundle id, 预设名, 显示名)。顺序即设置面板下拉的展示顺序。
/// 只列打开方式已验证过的；其他终端可用 [terminal] command 自定义命令接。
const KNOWN_TERMINALS: &[(&str, &str, &str)] = &[
    (LATERTERM_BUNDLE, "laterterm", "LaterTerm"),
    ("com.apple.Terminal", "terminal", "Terminal（系统自带）"),
    ("com.googlecode.iterm2", "iterm", "iTerm2"),
    ("com.mitchellh.ghostty", "ghostty", "Ghostty"),
    ("com.github.wez.wezterm", "wezterm", "WezTerm"),
    ("org.alacritty", "alacritty", "Alacritty"),
    ("net.kovidgoyal.kitty", "kitty", "kitty"),
];

/// 访达 Ctrl+/ 入口：worker 线程调用，内部转独立线程执行。
pub fn open_terminal_here(open: TerminalOpen) {
    std::thread::spawn(move || {
        let Some(dir) = finder_folder() else {
            info("打开终端失败：拿不到访达当前文件夹（自动化授权被拒或 Finder 无响应）");
            return;
        };
        match launch(&dir, &open) {
            Ok(what) => info(&format!("已在访达当前文件夹打开终端（{what}）：{dir}")),
            Err(e) => {
                info(&format!("打开终端失败：{e}，回退 Terminal.app"));
                let ok = run(Command::new("open").arg("-a").arg("Terminal").arg(&dir));
                if ok.is_ok() {
                    info(&format!("已回退到 Terminal.app：{dir}"));
                } else {
                    info("回退 Terminal.app 也失败了");
                }
            }
        }
    });
}

/// 访达当前文件夹：前台 Finder 窗口的 target；没有窗口（或 target 转不成
/// alias，如「最近使用」这类智能视图）回退主目录。
fn finder_folder() -> Option<String> {
    let script = r#"
tell application "Finder"
	if (count of windows) > 0 then
		try
			return POSIX path of (target of front window as alias)
		end try
	end if
	return POSIX path of (path to home folder)
end tell
"#;
    let out = Command::new("osascript")
        .arg("-e")
        .arg(script)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let path = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if path.is_empty() {
        return None;
    }
    Some(path)
}

/// 按配置拉起终端，返回描述文字用于日志。
fn launch(dir: &str, open: &TerminalOpen) -> Result<String, String> {
    // 自定义命令优先于预设：{path} 换成引好号的目录，交给 sh 执行
    let template = open.command.trim();
    if !template.is_empty() {
        run(Command::new("sh").arg("-c").arg(custom(template, dir)))?;
        return Ok("自定义命令".into());
    }
    let preset = resolve_preset(open.app.trim(), app_installed(LATERTERM_BUNDLE));
    let (args, what) = open_args(preset, dir);
    run(Command::new("open").args(&args))?;
    Ok(what)
}

/// `auto` = 装了 LaterTerm 用 LaterTerm，否则系统自带终端；其余预设原样。
fn resolve_preset(app: &str, laterterm_installed: bool) -> &str {
    if app.eq_ignore_ascii_case("auto") {
        if laterterm_installed {
            "laterterm"
        } else {
            "terminal"
        }
    } else {
        app
    }
}

/// 已装终端列表的 5 秒缓存：上次查询时刻 + 结果。
type TerminalCache = Mutex<Option<(Instant, Vec<(&'static str, &'static str)>)>>;

/// 设置面板下拉用的已装终端列表（预设名, 显示名），按固定顺序。
/// Launch Services 查询不便宜且面板每帧重绘，缓存 5 秒。
pub fn detected_terminals() -> Vec<(&'static str, &'static str)> {
    static CACHE: TerminalCache = Mutex::new(None);
    let mut cache = CACHE.lock().unwrap_or_else(|p| p.into_inner());
    if let Some((at, list)) = cache.as_ref() {
        if at.elapsed() < Duration::from_secs(5) {
            return list.clone();
        }
    }
    let list: Vec<_> = KNOWN_TERMINALS
        .iter()
        .filter(|(bundle, _, _)| app_installed(bundle))
        .map(|(_, preset, name)| (*preset, *name))
        .collect();
    *cache = Some((Instant::now(), list.clone()));
    list
}

/// 系统「已安装应用」登记表里查 bundle id（Launch Services 静态登记，
/// 不要求应用在跑）。
fn app_installed(bundle_id: &str) -> bool {
    autoreleasepool(|_pool| {
        NSWorkspace::sharedWorkspace()
            .URLForApplicationWithBundleIdentifier(&NSString::from_str(bundle_id))
            .is_some()
    })
}

/// 预设名/bundle id/应用名 → `open` 的参数（不含 "open" 本身）与日志描述。
/// 空名字按 LaterTerm 处理（默认值）。带点号的名字当作 bundle id 走 -b，
/// 其余走 -a。各预设的打开方式：
/// - Terminal/iTerm 认 odoc（把文件夹当文档打开），无需新实例；
/// - Ghostty/Alacritty/kitty/WezTerm/LaterTerm 走自己的命令行参数，
///   必须 `-n` 新实例才能收到 `--args`（已运行的实例收不到）。
pub(crate) fn open_args(app: &str, dir: &str) -> (Vec<String>, String) {
    let app = app.trim();
    let item = |s: &str| s.to_string();
    let what;
    let args = match app.to_ascii_lowercase().as_str() {
        // auto 正常已在 resolve_preset 解析；这里兜底防漏
        "" | "auto" | "laterterm" => {
            what = "LaterTerm".into();
            vec![item("-na"), item("LaterTerm"), item("--args"), item(dir)]
        }
        "terminal" => {
            what = "Terminal.app".into();
            vec![item("-a"), item("Terminal"), item(dir)]
        }
        "iterm" | "iterm2" => {
            what = "iTerm2".into();
            vec![item("-a"), item("iTerm"), item(dir)]
        }
        "ghostty" => {
            what = "Ghostty".into();
            vec![
                item("-na"),
                item("Ghostty"),
                item("--args"),
                item(&format!("--working-directory={dir}")),
            ]
        }
        "wezterm" => {
            what = "WezTerm".into();
            vec![
                item("-na"),
                item("WezTerm"),
                item("--args"),
                item("start"),
                item("--cwd"),
                item(dir),
            ]
        }
        "alacritty" => {
            what = "Alacritty".into();
            vec![
                item("-na"),
                item("Alacritty"),
                item("--args"),
                item("--working-directory"),
                item(dir),
            ]
        }
        "kitty" => {
            what = "kitty".into();
            vec![
                item("-na"),
                item("kitty"),
                item("--args"),
                item(&format!("--directory={dir}")),
            ]
        }
        other if other.contains('.') => {
            what = format!("bundle {app}");
            vec![item("-b"), item(app), item(dir)]
        }
        // 其余裸名字当应用名，保持原样大小写交给 open -a
        _ => {
            what = app.to_string();
            vec![item("-a"), item(app), item(dir)]
        }
    };
    (args, what)
}

/// 自定义命令模板：{path} 替换成引好号的目录，经 sh -c 执行。模板来自用户
/// 自己的配置文件，经 sh 执行是特性不是漏洞；目录可能带空格和引号，必须转义。
fn custom(template: &str, dir: &str) -> String {
    template.replace("{path}", &sh_quote(dir))
}

/// POSIX 单引号转义：`It's` → `'It'\''s'`。
fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

fn run(cmd: &mut Command) -> Result<(), String> {
    let program = cmd.get_program().to_string_lossy().into_owned();
    let status = cmd
        .status()
        .map_err(|e| format!("`{program}` 启动失败：{e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("`{program}` 退出码 {:?}", status.code()))
    }
}

fn info(msg: &str) {
    eprintln!("[laterkey] {msg}");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_args_presets() {
        // 空名字 = 默认 LaterTerm：新实例 + --args 收目录
        let (args, what) = open_args("", "/tmp/a b");
        assert_eq!(args, ["-na", "LaterTerm", "--args", "/tmp/a b"]);
        assert_eq!(what, "LaterTerm");
        assert_eq!(open_args("LaterTerm", "/tmp/a b").0, args);

        // Terminal/iTerm 认 odoc，不开新实例
        assert_eq!(open_args("terminal", "/tmp").0, ["-a", "Terminal", "/tmp"]);
        assert_eq!(open_args("iterm2", "/tmp").0, ["-a", "iTerm", "/tmp"]);

        // 带目录参数的终端走 --args + -n
        assert_eq!(
            open_args("ghostty", "/tmp").0,
            ["-na", "Ghostty", "--args", "--working-directory=/tmp"]
        );
        assert_eq!(
            open_args("wezterm", "/tmp").0,
            ["-na", "WezTerm", "--args", "start", "--cwd", "/tmp"]
        );

        // 带点号 = bundle id；裸名字 = 应用名
        assert_eq!(
            open_args("io.github.crazykun.LaterTerm", "/tmp").0,
            ["-b", "io.github.crazykun.LaterTerm", "/tmp"]
        );
        assert_eq!(open_args("Warp", "/tmp").0, ["-a", "Warp", "/tmp"]);
    }

    #[test]
    fn custom_template_quotes_path() {
        assert_eq!(sh_quote("/tmp/a b"), "'/tmp/a b'");
        assert_eq!(sh_quote("it's"), r"'it'\''s'");
        assert_eq!(
            custom("kitty --directory {path}", "/a b"),
            "kitty --directory '/a b'"
        );
    }

    #[test]
    fn auto_preset_resolves_by_laterterm_installation() {
        assert_eq!(resolve_preset("auto", true), "laterterm");
        assert_eq!(resolve_preset("auto", false), "terminal");
        assert_eq!(resolve_preset("AUTO", false), "terminal");
        // 显式选择不受 auto 解析影响
        assert_eq!(resolve_preset("iterm", true), "iterm");
        assert_eq!(resolve_preset("terminal", true), "terminal");
        // open_args 对 auto 也兜底成 LaterTerm（防漏解析）
        assert_eq!(
            open_args("auto", "/tmp").0,
            open_args("laterterm", "/tmp").0
        );
    }

    /// 冒烟：真机跑一次 Launch Services 检测（无断言安装了谁，只验通路）。
    #[test]
    fn detected_terminals_smoke() {
        let list = detected_terminals();
        eprintln!("本机检测到的终端：{list:?}");
        assert!(!list.is_empty(), "至少应有系统自带 Terminal");
        // 第二次调用走缓存，结果一致
        assert_eq!(detected_terminals(), list);
    }
}
