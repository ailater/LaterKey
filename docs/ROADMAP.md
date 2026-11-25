# LaterKey 待办（ROADMAP）

按优先级排序。已完成的见 git 历史与 README。

## P0 · 收尾验证（阻塞日常使用）

- [ ] **LaterKey.app 辅助功能授权**（需用户手动点）：系统设置 › 隐私与安全性 › 辅助功能 › + › /Applications/LaterKey.app。授权后启动 LaunchAgent 并验证托盘、全部功能。launchd 子进程不继承其他应用的 TCC 授权，这是唯一卡点。
- [ ] **Dock 再点最小化**实测（9 项需求里最后一项未真机验证）。
- [ ] **Karabiner 卸载**：配置已备份（`~/.config/karabiner/karabiner.json.bak-laterkey-*`、`/tmp/karabiner-backup-before-uninstall-*.json`）。用户级 profile 已切到空映射 `laterkey-test-off`；root 服务与 DriverKit 扩展的移除命令需用户用 sudo 执行。

## P1 · 快捷键速览 + 物理键映射对照（2026-11-23 用户提出；2026-11-25 已实现）

### 待执行工作流（2026-11-25 晚更新；W1–W3/W6 已完成，剩 W4/W5）

> 2026-11-25 晚进展：**W1 查出「面板撑不起全高」的真根因**——egui 0.35 的
> `ui.horizontal()` 只分配 `interact_size.y`（18px）高，内部 `available_height()`
> 恒为 18，此前 `Frame+set_min_height` 与 `ui.scope` 两版都栽在同一根因上；
> 已改 `allocate_ui_with_layout` 显式分配全高，并加**无头布局测试**锁定回归
> （`begin_pass/end_pass + tessellate` 对图元做几何断言，锁屏/无 GPU 也能跑）。
> 期间屏幕锁定无法截屏，像素级自检截图（ui4/ui5）本身也证实了缺陷（面板底
> 只到内容高度）。

- [x] **W1 · 布局核对**：无头测试证实修复后面板底从控件行下方铺到窗口底部（628px/660）；行快捷键不叠压、右上角迷你控件正常已由截图确认。**待补**：屏幕解锁后对部署实例做一次真机视觉复核（合成手势抓窗）。
- [x] **W2 · 清理临时验证产物**：`src/bin/cheatsheet_stress.rs` 已删（含目录）、`gui.rs` 的 `run_event_loop` 已改回私有、`/tmp` 探针与截图已清。
- [x] **W3 · 全量门禁**：fmt / clippy(-D warnings) / test(135 通过) / release build 全绿。
- [ ] **W4 · 真机手势验收**（用户侧）：长按 Ctrl 3 秒弹出、松开即消失；期间敲键/点鼠标不触发；托盘菜单「快捷键速览」手动呼出 + × 关闭；两种口径切换内容正确（⌘C↔Ctrl+C）。
- [ ] **W5 · 提交发版**：验收后升版本号、打 tag、走 Release + cask 流程（提交随本次先落 main）。
- [x] **W6 · 托盘勾选缺陷**：`gui.rs` 的 `set_field` 已补 `task_manager` / `settings_hotkey` 分支（字段名与 statusbar.rs 菜单表一致）。

- 部署注意（2026-11-25 实测）：`package-macos.sh --install` 的 bootout 可能把旧实例变成**孤儿进程**（launchd 不再管理但进程存活，仍持有实例锁）→ 新实例启动即干净退出、`KeepAlive SuccessfulExit=false` 不重启，表现为「装完没生效」。处理：`kill` 孤儿进程 + `launchctl kickstart -k gui/$(id -u)/io.github.crazykun.laterkey`。

- [x] 已实现（2026-11-25）：左栏当前应用全部菜单快捷键（AX 遍历），右栏系统快捷键（symbolichotkeys），顶部「物理键 / Mac 原生」口径切换（egui 面板用文字不用键帽符号，见字体定案）。**触发 = 长按物理 Ctrl 3 秒**（期间敲键/点鼠标即作废，松开即关；tap 只记按住起点，worker 每轮裁决）+ 托盘菜单「快捷键速览」手动入口；浮窗 with_active(false) 不抢前台焦点，关闭只隐藏视口（同 Win+V 面板的 CGL 教训）。设置面板「按键映射」与托盘菜单各有一个开关（`shortcuts_peek`）。
- 实施中的真机发现（2026-11-25，本机 macOS 27.2）：
  - **Electron 应用（ZCode/飞书）默认不暴露 AX 树**：`AXMenuBar` 读不到，必须先对 AX 应用元素置 `AXManualAccessibility = true` 才读得到，且树是异步构建的——首次失败延时 300ms 重试一次（shortcuts.rs）。
  - **菜单树结构**：菜单栏子项（如「文件」）下挂的是 `AXMenu` 元素，菜单项在 AXMenu 的 AXChildren 里；只展开一层子菜单（`父 › 子`），条目平铺进父组。
  - **60/61 对照修正**：实测是输入法切换（选择上一个/下一个输入法），不是 ROADMAP 旧文写的「移动空间」；移动空间是 79/81。
  - 系统快捷键只列内置表认识且已启用的 id（CFPreferences 实时值）；本机 plist 里没有截屏组（28–31/184）条目，属正常——未启用或未落盘的不显示。
- 剩余缺口（低优先）：系统快捷键只覆盖内置表认识的 id（未知 id 不显示，宁缺勿错）；口径选择不跨会话记忆（每次打开回到物理键口径）。

## P2 · 剪贴板历史（Win+V）

- [x] 已实现（2026-11-23）：worker 50ms 轮询 `NSPasteboard.changeCount` 采集，类型表先拦密码管理器标记（`org.nspasteboard.ConcealedType` / `TransientType`）再读字符串；core 环形历史（100 条、同内容去重置顶、单条 1 MiB 拒收）；egui 第二视口面板（搜索、↑↓/鼠标选择、清空，标题栏保留否则成不了 key window）；贴回经 worker 状态机完成——写回剪贴板 → 激活打开面板前快照的原应用 → 每轮轮询等前台切回（1.5s 超时放弃）→ 合成 ⌘V。默认开启，⌘V 粘贴不受影响（有守卫测试）。**缺口**：历史仅存内存，重启即清，落盘存储未做（serde 形状已备好）。待真机验证：面板自动聚焦、贴回激活时序。

## P3 · 分发与体验

- [ ] GitHub Actions release 工作流（参照 LaterScreen `.github/workflows/release.yml`：自签名证书 secret 注入、universal 胖包、dmg）。
- [x] homebrew-ailater 的 cask（与 lscreen/latermd/laterterm 并列）：已上线，且 release 工作流在 tag 发布后自动 bump `Casks/laterkey.rb` 的 version 与 dmg sha256（commit「laterkey <版本>」推 master）。需在 LaterKey 仓库配置 **`secrets.AILATER_TAP_TOKEN`**（crazykun/homebrew-ailater 的 repo 写权限 token）；未配置时该步输出 warning 并跳过，不影响发布。
- [ ] 配置热加载（手改 config.toml 后自动生效；目前只有界面开关会热更新）。
- [ ] 终端组/浏览器组的应用列表设置界面（现在只能手改 config.toml）。
