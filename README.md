# LaterKey

让 Windows 布局的普通键盘在 macOS 上按 Windows 的习惯工作。Rust 编写，纯用户态，只需要「辅助功能」权限，不装驱动，不依赖 Karabiner。

> **当前阶段：原型。** 代码已写完，单元测试与真机自测通过；真实按键下的效果还需要人工实测，见[实测清单](#实测清单)。

## 功能

| 物理按键 | 效果 | 说明 |
|---|---|---|
| Ctrl + C / V / X / Z … | ⌘ + 同键 | 与「终端」组里的应用例外：保持 ⌃ 原义 |
| Win(⌘) + C / W / A … | ⌃ + 同键 | 终端里的中断/控制键：Win+C = Ctrl+C。与上一条同时成立，不需要识别前台应用 |
| Ctrl + ←/→、Ctrl + ⌫ | ⌥ + 同键 | 按词跳转、按词删除 |
| Ctrl + Space | 不变 | 系统的输入法切换（Win+Space 经换位也落到 ⌃Space） |
| 左 Alt + Tab | ⌘ + Tab | 系统的应用切换器。右 Alt 不动 |
| Win(⌘) + L | 锁屏 | 先于 Win→⌃ 换位生效 |
| Win(⌘) + V | 剪贴板历史 | **尚未实现**，默认关闭 |
| F2 | 回车 | 仅当前台是访达 |
| F5 | ⌘R | 仅当前台是浏览器 |
| 再点一次前台应用的 Dock 图标 | 最小化它的窗口 | 再点一次还原 |

## 快速开始

```bash
cargo build --release
cargo test --workspace                             # 纯单元测试，不需要任何权限
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check

target/release/laterkey doctor                     # 只读诊断：权限、前台应用、Dock 图标、锁屏通路
target/release/laterkey run --only none --for 10s  # 自检：装上拦截层但不改任何事件
target/release/laterkey run --for 5m               # 真正启用，5 分钟后自动退出
```

从终端应用里运行 `run`：**Win+C（=⌃C）可中断/停止本程序**。原型阶段默认 5 分钟后自动退出，`--forever` 取消。

| 参数 | 作用 |
|---|---|
| `--only keys` / `--only dock` / `--only none` | 只启用某一部分。`none` 是自检，不改任何事件 |
| `--for 30s` / `5m` / `1h`，或 `--forever` | 运行多久 |
| `-v` / `--verbose` | 逐条打印改写与吞掉的事件。只有键码和标志位，不含输入内容 |
| `-c path.toml` | 指定配置文件 |

## 实测清单

以下几项依赖真实按键下的系统行为，自动化测试覆盖不到，需要人工确认。建议每项先单独跑。

> 2026-11-23 真机已验证 ✅：1（复制粘贴）、3（按词跳转）、5（F2/F5）、7（锁屏）。待确认：2（输入法有没有切换）、4（切换器画面有没有弹出）、6（Dock）。

```bash
target/release/laterkey run --only keys --for 2m -v   # 键盘
target/release/laterkey run --only dock --for 2m      # Dock
```

| # | 操作 | 期望 | 把握 |
|---|---|---|---|
| 1 | 在普通应用里按 Ctrl+C、Ctrl+V | 复制、粘贴 | 高 |
| 2 | 在「终端」里按 Ctrl+C | 仍然是中断 | 高 |
| 3 | 按 Ctrl+Space | 切换输入法 | 待实测 |
| 4 | 按 Ctrl+←/→ | 按词跳转 | 待实测 |
| 5 | 按住左 Alt 按 Tab，松开 Alt | 弹出应用切换器并切换 | **待实测，风险最高** |
| 6 | 访达里选中文件按 F2；浏览器里按 F5 | 进入重命名；刷新 | 中高 |
| 7 | 再点一次前台应用的 Dock 图标 | 窗口最小化；再点还原 | 中高 |
| 8 | 按 Win(⌘)+L | 立即锁屏。**放最后测** | 高 |

第 3、5 项的不确定点相同：Ctrl 按下时已经被改成 ⌘，随后的 Space / Tab 能否被系统快捷键正确识别。如果不行，退路是「修饰键按下先扣住，看到下一个键再决定以哪个身份发出」，状态机的 `Decision` 留了扩展位。

## 配置

不写配置文件就用默认规则。要自定义就创建 `~/.config/laterkey/config.toml`（或用 `-c` 指定）：

```toml
[remap]
ctrl_to_cmd = true            # Ctrl+C/V/X/Z… 当作 ⌘
alt_tab = true                # 左 Alt+Tab 切换应用
ctrl_arrows = true            # Ctrl+←/→/⌫ 按词跳转/删除（⌥）
win_key_lock = true           # Win(⌘)+L 锁屏
win_key_clipboard = false     # Win(⌘)+V 剪贴板历史：面板未实现，保持 false
dock_click_minimize = true    # 再点一次前台应用的 Dock 图标 → 最小化

# [apps] 不写就用内置列表：常见终端、浏览器、访达，以及整体不改的远程桌面/虚拟机。
# 要改就整段写出来，例如：
# [apps]
# terminals = ["com.apple.Terminal", "com.googlecode.iterm2"]
# passthrough = ["com.vmware.fusion", "com.parallels.desktop.console"]

[[substitute]]
key = "F2"
when = "finder"               # 仅当前台是访达
to = "Return"

[[substitute]]
key = "F5"
when = "browsers"             # 仅当前台是浏览器
to = "Cmd+R"
```

上面的内容就是默认值，不写配置文件也是这套规则。各项**没写就沿用默认，写了就整个替换**（不是追加）。**写错的配置会直接报错退出**，不会带着一份没生效的规则运行。

## 结构

```
crates/
├── laterkey-core/          # 纯逻辑：规则、修饰键状态机、配置。禁止 unsafe，可完整单测
├── laterkey-platform-mac/  # 裸 FFI：事件拦截、前台应用跟踪、合成按键、锁屏、Dock
└── laterkey-app/           # 命令行 laterkey：run / doctor / lock
```

- **就地改写，不注入事件。** 拦截层把 Ctrl 的键码和标志位改成 ⌘ 后原样放行。进程崩溃也不会留下卡住的修饰键，系统恢复成原样。
- **回调只调用一个纯函数。** 拦截回调里只读原子量、决策、把结果写回事件；耗时的活（锁屏、读窗口、最小化）都转给后台线程。回调超时会被系统禁用拦截层。
- **拦截层在独立线程上**，不在主线程。每个按键都要过回调，不能被界面拖慢。
- **合成按键带标记**（F5 → ⌘R）。标记事件经过拦截层时原样放行，不会再进一次状态机。
- **安全输入开启时自动暂停**（密码框、锁屏界面），恢复时清空状态机。
- **日志只记键码和决策计数**，不记输入内容。

## 路线图

待办与方案见 [docs/ROADMAP.md](docs/ROADMAP.md)。下一项重点：**快捷键速览**（当前应用+系统的全部快捷键，可在 Mac 符号与物理键 Ctrl/Win/Alt 两种口径间切换）。

## 已知限制

- **Alt+Tab、Ctrl+Space 尚未经真实按键验证**，见实测清单。
- **复制与中断的分工是固定的**：GUI 快捷键用物理 Ctrl（Ctrl+C 复制），终端控制键用物理 Win（Win+C = Ctrl+C 中断）。这是 Windows 键盘的原生分工，同一个应用（如 ZCode/VS Code）的编辑区和内嵌终端因此同时正确；若某个应用里你确实需要原生 ⌘ 快捷键，物理 Win 键现在就是它。
- **Dock 图标缓存有最多 2 秒延迟**。刚启动或刚退出的应用，图标位置可能还没更新，这期间的点击交给系统处理。
- **Dock 开了放大或自动隐藏时未验证。**
- Dock 偶尔会对同一个应用显示两个图标（属性完全相同）。程序只认第一个，重复的那个交给系统。
- 还没有：剪贴板历史面板、托盘与设置界面、`.app` 打包、开机自启、配置热加载。所以 `win_key_clipboard` 必须保持关闭，否则 ⌘V 会被吞掉。
