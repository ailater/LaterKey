# LaterKey

让 Windows 布局的普通键盘在 macOS 上按 Windows 的习惯工作。纯用户态实现，不装驱动、不依赖 Karabiner，只需要辅助功能权限。

![LaterKey 设置面板](docs/images/settings.png)

## 安装

```bash
brew install --cask crazykun/ailater/laterkey
```

也可以从 [Releases](https://github.com/ailater/LaterKey/releases) 下载 dmg，把 LaterKey.app 拖进 /Applications。

装完两步就能用：

1. **授权**：系统设置 › 隐私与安全性 › 辅助功能 › 添加 LaterKey.app（只需这一次，后续升级不用重新授权）；
2. **打开 LaterKey.app**：菜单栏出现托盘图标，功能立即生效。想开机自启就执行 `laterkey install`。

## 功能对照

核心是一组**修饰键对调**：macOS 的图形界面快捷键用 ⌘，终端控制键用 ⌃；Windows 键盘上这两个角色分别由 Ctrl 和 Win 承担。对调之后，**图形界面和终端同时符合 Windows 习惯**，不需要识别当前在哪个应用。

| 你按的物理键 | 被当成 | 承担的角色 |
|---|---|---|
| **Ctrl** | ⌘ Command | 复制粘贴等所有界面快捷键 |
| **Win** | ⌃ Control | 终端中断等控制键 |
| **Alt（左）** | ⌥ Option | 与 Tab 组合时切换应用 |

### 键盘操作

| Windows 习惯的操作 | 在 LaterKey 里这样按 | 实际效果 |
|---|---|---|
| 复制 / 粘贴 / 剪切 | Ctrl+C / Ctrl+V / Ctrl+X | ⌘C / ⌘V / ⌘X |
| 撤销 / 重做 | Ctrl+Z / Ctrl+Shift+Z | ⌘Z / ⌘⇧Z |
| 全选 / 保存 / 查找 / 新建 | Ctrl+A / S / F / N | ⌘A / ⌘S / ⌘F / ⌘N |
| 关闭窗口或标签页 | Ctrl+W | ⌘W |
| **终端里中断命令** | **Win+C** | ⌃C（SIGINT） |
| 终端 EOF / 挂起 | Win+D / Win+Z | ⌃D / ⌃Z |
| 切换应用 | 左 Alt+Tab | ⌘Tab 系统切换器 |
| 按词移动光标 | Ctrl+← / Ctrl+→ | ⌥← / ⌥→ |
| 按词删除 | Ctrl+Backspace | ⌥⌫ |
| 切换输入法 | Ctrl+Space 或 Win+Space | ⌃Space |
| 锁屏 | Win+L | 立即锁定屏幕 |
| 重命名文件（访达里） | F2 | 回车进入重命名 |
| 刷新页面（浏览器里） | F5 | ⌘R |

### 鼠标操作

| Windows 习惯的操作 | 在 LaterKey 里这样按 | 实际效果 |
|---|---|---|
| 跳转到定义（VS Code / TRAE / Zed 等） | Ctrl+鼠标左键 | ⌘+点击 |
| 新标签页打开链接（浏览器） | Ctrl+点击链接 | ⌘+点击 |
| 多选文件（访达等） | Ctrl+点击 | ⌘+点击 |
| 最小化前台应用的窗口 | 再点一次它的 Dock 图标 | 窗口最小化，再点还原 |

以上每一项都可以在托盘菜单或设置面板里**单独开关**，改动立即生效。

## 使用

- **托盘菜单**：逐项开关功能、打开设置面板、查看权限状态、退出；
- **设置面板**：与托盘同步的开关界面，含权限卡片（辅助功能 / 监听事件的状态、跳转与一键检测）；
- **常驻自启**：`laterkey install` 注册 LaunchAgent（登录自启、异常退出自动拉起），`laterkey uninstall` 卸载；
- **命令行**：`laterkey run`（前台运行）、`laterkey doctor`（诊断权限与环境）、`laterkey lock`（立即锁屏）。

## 常见问题

**为什么要授权两个权限？** 只有「辅助功能」是必须的（拦截按键、Dock 操作都靠它）。「监听事件 ListenEvent」是 macOS 15+ 的独立权限项，仅诊断用，不授权也不影响功能。

**升级后要重新授权吗？** 不用。发布包用同一个自签名证书签名，辅助功能授权绑定证书身份，跨版本升级保持有效。

**和 Karabiner 冲突吗？** 两者都会改写键盘事件，**不要同时用**。迁移到 LaterKey 后建议退出 Karabiner。

## 已知限制

- Win+V 剪贴板历史尚未实现（开关默认关闭，勿开）；
- Dock 开了图标放大或自动隐藏时，「再点最小化」未经验证；
- 终端组/浏览器组的应用列表目前需手改 `~/.config/laterkey/config.toml`；
- 更多计划见 [docs/ROADMAP.md](docs/ROADMAP.md)（快捷键速览、剪贴板历史等）。

## License

MIT
