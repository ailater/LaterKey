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
        <string>run</string>
        <string>--forever</string>
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
    let source = std::env::current_exe().map_err(|e| format!("找不到自己的位置：{e}"))?;
    let bin_dir = install_bin_dir();
    let log = log_dir();
    std::fs::create_dir_all(&bin_dir)
        .map_err(|e| format!("创建 {} 失败：{e}", bin_dir.display()))?;
    std::fs::create_dir_all(&log).map_err(|e| format!("创建 {} 失败：{e}", log.display()))?;
    std::fs::create_dir_all(agent_dir()).map_err(|e| format!("创建 LaunchAgents 目录失败：{e}"))?;

    let dest = bin_dir.join("laterkey");
    // 先卸掉旧的 LaunchAgent，否则它会在我们替换二进制时把旧进程又拉起来
    let _ = bootout();
    if dest.exists() {
        std::fs::remove_file(&dest)
            .map_err(|e| format!("删除旧二进制 {} 失败：{e}", dest.display()))?;
    }
    std::fs::copy(&source, &dest).map_err(|e| format!("复制二进制失败：{e}"))?;

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
        "已安装常驻：\n  二进制 {}\n  LaunchAgent {}\n  日志 {}\n\n登录自动启动，崩溃/被杀后自动拉起（{LABEL}）",
        dest.display(),
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
    let domain = format!("gui/{}", unsafe { libc::getuid() });
    std::process::Command::new("launchctl")
        .args(["bootout", &domain, LABEL])
        .output()
}
