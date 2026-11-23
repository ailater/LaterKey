# LaterKey 待办（ROADMAP）

按优先级排序。已完成的见 git 历史与 README。

## P0 · 收尾验证（阻塞日常使用）

- [ ] **LaterKey.app 辅助功能授权**（需用户手动点）：系统设置 › 隐私与安全性 › 辅助功能 › + › /Applications/LaterKey.app。授权后启动 LaunchAgent 并验证托盘、全部功能。launchd 子进程不继承其他应用的 TCC 授权，这是唯一卡点。
- [ ] **Dock 再点最小化**实测（9 项需求里最后一项未真机验证）。
- [ ] **Karabiner 卸载**：配置已备份（`~/.config/karabiner/karabiner.json.bak-laterkey-*`、`/tmp/karabiner-backup-before-uninstall-*.json`）。用户级 profile 已切到空映射 `laterkey-test-off`；root 服务与 DriverKit 扩展的移除命令需用户用 sudo 执行。

## P1 · 快捷键速览 + 物理键映射对照（2026-11-23 用户提出）

> 参照用户给的 CheatSheet 式截图：左栏当前应用的全部菜单快捷键，右栏系统快捷键（调度中心/键盘/输入法/屏幕快照/聚焦/辅助功能等）。LaterKey 的特色是**一键切换显示口径**：
>
> | 显示口径 | 「拷贝」显示为 | 「锁屏」显示为 |
> |---|---|---|
> | Mac 符号 | ⌘C | ⌃⌘Q |
> | **物理键（Ctrl/Win/Alt）** | **Ctrl+C** | **Win+Ctrl+Q** |
>
> 因为我们的互换模型：物理 Ctrl 产生 ⌘、物理 Win 产生 ⌃。用户看快捷键提示时想知道的是「我这只 Windows 键盘上该按什么」。

### 数据源（2026-11-23 本机只读探测已验证）

1. **当前应用快捷键**：辅助功能 API 遍历菜单栏。`AXApplication → AXMenuBar → AXChildren（各菜单）→ AXChildren（菜单项）`，菜单项上读：
   - `AXTitle`：名称；
   - `AXMenuItemCmdChar`：快捷键字符（`C`、`V`、`⎋`…）；
   - `AXMenuItemCmdModifiers`：修饰键掩码。实测含义：**0=⌘，1=⇧⌘，2=⌥⌘，3=⇧⌥⌘，4=⌃⌘，24=fn**（如「开始听写 fn+D」）。是位掩码：bit0=⇧、bit1=⌥、bit2=⌃，⌘ 为默认基底，fn 另有一套编码；
   - `AXMenuItemCmdVirtualKey`：虚拟键码（部分项有，如 Esc=53）。
   - 实测 App Cleaner 的菜单完整可读（8 个菜单、含 Apple 菜单的强制退出/锁屏/退出登录）。**待测**：Electron 类应用（ZCode/VS Code）的菜单 AX 可读性。
2. **系统快捷键**：`~/Library/Preferences/com.apple.symbolichotkeys.plist` 的 `AppleSymbolicHotKeys` 字典。每条 `{enabled, value:{parameters:[字符, 虚拟键码, 修饰键]}}`，修饰键是 CGEventFlags 风格（⇧0x20000、⌃0x40000、⌥0x80000、⌘0x100000）。实测可读：聚焦搜索 = Space+0x100000 = ⌘Space、向左/右移动空间 = ⌃←/⌃→ 等。
   - **注意**：实施时用 CFPreferences（cfprefsd 实时值）而不是直接读文件——直接读 plist 可能拿到落盘延迟的旧值。
   - id→名称对照表需内置（32=调度中心、33=应用程序窗口、36=显示桌面、64=聚焦搜索、65=访达搜索、60/61=移动空间、160=启动台、118/119=切换桌面 1/2…），plist 里只有 id 没有名字。

### 显示切换的转换规则

按当前 toggles 决定怎么换算（开关不同，显示就不同）：
- `ctrl_as_cmd` 开：⌘ → 显示为 **Ctrl**；
- `cmd_as_ctrl` 开：⌃ → 显示为 **Win**；
- ⌥ → 显示为 **Alt**（恒等，macOS 上物理 Alt 就是 ⌥）；
- 两者同时出现的组合（如 ⌃⌘Q）按位各自换算：Win+Ctrl+Q；
- Alt+Tab 特例：⌘Tab 在切换器语境显示为 Alt+Tab（左 Alt+Tab 被映射成 ⌘Tab）。

### 工程拆分

- `laterkey-core`：快捷键条目模型（标题/字符/虚拟键码/修饰键）、Mac↔物理键的显示换算（纯函数，可单测）；
- `laterkey-platform-mac`：AX 菜单遍历、symbolic hotkeys 读取（CFPreferences）；
- `laterkey-app`：egui 速览窗口（两栏：当前应用 / 系统，分组小标题）、顶部切换器（Mac 符号 / 物理键）、托盘菜单入口「快捷键速览」；
- 触发方式待定：托盘菜单项先行；是否支持「长按物理 Ctrl 弹出」（CheatSheet 是长按 ⌘）需要拦截层加一个「按住超过阈值且期间无其他键」的手势，单独评估。

## P2 · 剪贴板历史（Win+V）

原始需求项。`NSPasteboard.changeCount` 轮询采集 → 历史存储 → 面板选择 → 写回剪贴板并模拟粘贴。注意密码管理器标记的敏感内容（`org.nspasteboard.ConcealedType` 等）不记录。面板拿到焦点后中文输入法才能打字搜索；贴回时要等原应用重新激活再发 ⌘V。

## P3 · 分发与体验

- [ ] GitHub Actions release 工作流（参照 LaterScreen `.github/workflows/release.yml`：自签名证书 secret 注入、universal 胖包、dmg）。
- [ ] homebrew-ailater 的 cask（与 lscreen/latermd/laterterm 并列）。
- [ ] 配置热加载（手改 config.toml 后自动生效；目前只有界面开关会热更新）。
- [ ] 终端组/浏览器组的应用列表设置界面（现在只能手改 config.toml）。
