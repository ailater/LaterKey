//! 常驻安装：把二进制复制到固定位置，装 LaunchAgent（登录自启 + 崩溃拉起）。

use std::path::PathBuf;

/// LaunchAgent 的 Label
pub const LABEL: &str = "io.github.crazykun.laterkey";

pub fn agent_dir() -> PathBuf {
    home().join("Library/LaunchAgents")
}

pub fn install_bin_dir() -> PathBuf {
    home().join("Library/Application Support/laterkey")
}

pub fn log_dir() -> PathBuf {
    home().join("Library/Logs/laterkey")
}

fn home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp"))
}

/// 当前可执行文件所属的 .app 内二进制路径（经符号链接解析）。
/// brew 的 /opt/homebrew/bin/laterkey 是指向 .app 的软链，解析后也在 .app 里。
fn app_binary() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let exe = exe.canonicalize().unwrap_or(exe);
    let in_bundle = exe
        .ancestors()
        .any(|a| a.extension().is_some_and(|e| e == "app"));
    in_bundle.then_some(exe)
}

/// 生成 LaunchAgent 的 plist 内容。
pub fn plist(bin: &std::path::Path) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>{LABEL}</string>
    <key>ProgramArguments</key>
    <array>
        <string>{bin}</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <dict>
        <key>SuccessfulExit</key>
        <false/>
    </dict>
    <key>ThrottleInterval</key>
    <integer>5</integer>
    <key>StandardOutPath</key>
    <string>{log}/stdout.log</string>
    <key>StandardErrorPath</key>
    <string>{log}/stderr.log</string>
    <key>ProcessType</key>
    <string>Interactive</string>
</dict>
</plist>
"#,
        bin = bin.display(),
        log = log_dir().display(),
    )
}

/// 安装：复制二进制、写 plist、加载。已加载时先卸掉再装（幂等）。
pub fn install() -> Result<String, String> {
    let log = log_dir();
    std::fs::create_dir_all(&log).map_err(|e| format!("创建 {} 失败：{e}", log.display()))?;
    std::fs::create_dir_all(agent_dir()).map_err(|e| format!("创建 LaunchAgents 目录失败：{e}"))?;

    // 常驻目标：优先 .app 本体——launchd 不继承任何应用的 TCC 授权，授权主体是
    // /Applications/LaterKey.app（自签名证书身份，跨版本升级不失效）。复制一份
    // 裸二进制副本会脱离这个身份，跑起来还是没权限。
    let dest: PathBuf;
    let copied: bool;
    match app_binary() {
        Some(app_bin) => {
            dest = app_bin;
            copied = false;
        }
        None => {
            // 开发态（cargo 构建的裸二进制）：复制到固定位置
            let source = std::env::current_exe().map_err(|e| format!("找不到自己的位置：{e}"))?;
            let bin_dir = install_bin_dir();
            std::fs::create_dir_all(&bin_dir)
                .map_err(|e| format!("创建 {} 失败：{e}", bin_dir.display()))?;
            let d = bin_dir.join("laterkey");
            if d.exists() {
                let _ = std::fs::remove_file(&d);
            }
            std::fs::copy(&source, &d).map_err(|e| format!("复制二进制失败：{e}"))?;
            dest = d;
            copied = true;
        }
    }

    // 先卸掉旧的 LaunchAgent（幂等），再写 plist、加载
    let _ = bootout();
    let plist_path = agent_dir().join(format!("{LABEL}.plist"));
    std::fs::write(&plist_path, plist(&dest)).map_err(|e| format!("写 plist 失败：{e}"))?;

    let domain = format!("gui/{}", unsafe { libc::getuid() });
    let out = std::process::Command::new("launchctl")
        .args(["bootstrap", &domain, &plist_path.to_string_lossy()])
        .output()
        .map_err(|e| format!("调不起 launchctl：{e}"))?;
    if !out.status.success() {
        return Err(format!(
            "launchctl bootstrap 失败：{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        ));
    }

    Ok(format!(
        "已安装常驻（裸启动 = 菜单栏托盘）：\n  目标 {}{}\n  LaunchAgent {}\n  日志 {}\n\n登录自动启动，崩溃/被杀后自动拉起（{LABEL}）。\n首次运行需在 系统设置 › 隐私与安全性 › 辅助功能 里授权 LaterKey.app。",
        dest.display(),
        if copied { "（开发态副本）" } else { "" },
        plist_path.display(),
        log.display(),
    ))
}

/// 卸载：卸 LaunchAgent、删 plist、删二进制。
pub fn uninstall() -> Result<String, String> {
    let _ = bootout();
    let plist_path = agent_dir().join(format!("{LABEL}.plist"));
    let _ = std::fs::remove_file(&plist_path);
    let dest = install_bin_dir().join("laterkey");
    let _ = std::fs::remove_file(&dest);
    Ok(format!("已卸载常驻（{LABEL}）。本次运行的实例需要另行停止"))
}

fn bootout() -> std::io::Result<std::process::Output> {
    // 第二参数必须是完整 service path（gui/<uid>/<label>）或 plist 路径。
    // 之前只传 LABEL 会被当成 plist 路径而失败：旧服务卸不掉，再次 bootstrap
    // 报「Bootstrap failed: 5: Input/output error」（2026-11-23 用户实踩）。
    let service = format!("gui/{}/{}", unsafe { libc::getuid() }, LABEL);
    std::process::Command::new("launchctl")
        .args(["bootout", &service])
        .output()
}
