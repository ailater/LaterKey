//! 配置：TOML 文件 → 规则。不写配置文件就用默认值；写错了直接报错退出。
//!
//! 对一个键盘重映射器来说，「带着一份没生效的配置运行」比「不运行」更糟，
//! 所以解析失败不降级、不猜。

use serde::Deserialize;

use crate::flags::{ALTERNATE, COMMAND, CONTROL, SHIFT};
use crate::keys;
use crate::remap::{groups, Chord, Rules, Substitute};

/// 配置文件示例，内容与默认值完全一致。有测试保证它能解析、能编译，
/// 并且编译结果与不带配置文件时相同。
pub const EXAMPLE: &str = r#"
[remap]
ctrl_as_cmd = true            # 物理 Ctrl 当作 ⌘：Ctrl+C/V 复制粘贴（终端里也一样，中断用 Win+C）
cmd_as_ctrl = true            # 物理 Win(⌘) 当作 ⌃：终端里 Win+C = Ctrl+C 中断
alt_tab = true                # 左 Alt+Tab 切换应用
ctrl_arrows = true            # Ctrl+←/→/退格 按词跳转/删除（⌥）
win_key_lock = true           # Win(⌘)+L 锁屏
win_key_clipboard = true      # Win(⌘)+V 剪贴板历史面板：搜索历史、选中贴回
task_manager = true           # Ctrl+Alt+Del 或 Ctrl+Esc 呼出强制退出窗口
settings_hotkey = true          # Ctrl+Alt+K 全局唤出设置面板
shortcuts_peek = true         # 长按物理 Ctrl 2 秒呼出快捷键速览浮窗
dock_click_minimize = true    # 再点一次前台应用的 Dock 图标 → 最小化

# [apps] 不写就用内置列表：常见终端、浏览器、访达，以及整体不改的远程桌面/虚拟机。
# 要改就整段写出来（terminals / browsers / finder / passthrough，都是 bundle id 数组），
# 写了的那一项会替换内置列表。

[[substitute]]
key = "F2"
when = "finder"               # 仅当前台是访达
to = "Return"

[[substitute]]
key = "F5"
when = "browsers"             # 仅当前台是浏览器
to = "Cmd+R"
"#;

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default)]
    pub remap: RemapConfig,
    #[serde(default)]
    pub apps: AppsConfig,
    /// 不写就是默认的两条（F2 → 回车、F5 → ⌘R）；写 `substitute = []` 可全部关掉
    #[serde(default = "default_substitutes")]
    pub substitute: Vec<SubstituteConfig>,
}

/// 默认的单键替换：用户点名要的两条，范围收得很窄（只在前台是访达/浏览器时）。
fn default_substitutes() -> Vec<SubstituteConfig> {
    vec![
        SubstituteConfig {
            key: "F2".into(),
            r#when: "finder".into(),
            to: "Return".into(),
        },
        SubstituteConfig {
            key: "F5".into(),
            r#when: "browsers".into(),
            to: "Cmd+R".into(),
        },
    ]
}

impl Default for Config {
    fn default() -> Self {
        Self {
            remap: RemapConfig::default(),
            apps: AppsConfig::default(),
            substitute: default_substitutes(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields, default)]
pub struct RemapConfig {
    pub ctrl_as_cmd: bool,
    pub cmd_as_ctrl: bool,
    pub alt_tab: bool,
    pub ctrl_arrows: bool,
    pub win_key_lock: bool,
    pub win_key_clipboard: bool,
    pub task_manager: bool,
    pub settings_hotkey: bool,
    pub shortcuts_peek: bool,
    pub dock_click_minimize: bool,
}

impl Default for RemapConfig {
    fn default() -> Self {
        Self {
            ctrl_as_cmd: true,
            cmd_as_ctrl: true,
            alt_tab: true,
            ctrl_arrows: true,
            win_key_lock: true,
            // Win+V 剪贴板历史面板默认开；只拦裸 Win+V，⌘V 粘贴不受影响
            win_key_clipboard: true,
            task_manager: true,
            settings_hotkey: true,
            // 长按 Ctrl 速览默认开；CheatSheet 式手势，期间无其他按键才触发
            shortcuts_peek: true,
            dock_click_minimize: true,
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields, default)]
pub struct AppsConfig {
    pub terminals: Vec<String>,
    pub browsers: Vec<String>,
    pub finder: Vec<String>,
    pub passthrough: Vec<String>,
}

impl Default for AppsConfig {
    fn default() -> Self {
        Self {
            terminals: vec![
                "com.apple.Terminal".into(),
                "com.googlecode.iterm2".into(),
                "com.mitchellh.ghostty".into(),
                "com.github.wez.wezterm".into(),
                "org.alacritty".into(),
                "net.kovidgoyal.kitty".into(),
                "dev.warp.Warp-Stable".into(),
                "io.github.crazykun.LaterTerm".into(),
            ],
            browsers: vec![
                "com.google.Chrome".into(),
                "com.microsoft.edgemac".into(),
                "org.mozilla.firefox".into(),
                "com.apple.Safari".into(),
                "com.brave.Browser".into(),
                "company.thebrowser.Browser".into(),
                "com.vivaldi.Vivaldi".into(),
                "com.operasoftware.Opera".into(),
            ],
            finder: vec!["com.apple.finder".into()],
            // 远程桌面、虚拟机里对方多半已经是 Windows/自定义键位，整体透传
            passthrough: vec![
                "com.vmware.fusion".into(),
                "com.parallels.desktop.console".into(),
                "com.microsoft.rdc.macos".into(),
                "com.microsoft.windowsapp".into(),
            ],
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SubstituteConfig {
    /// 被替换的键，如 "F2"
    pub key: String,
    /// 仅当前台应用属于这个组（[apps] 下的组名）
    #[serde(rename = "when")]
    pub r#when: String,
    /// 替换成什么，如 "Return"、"Cmd+R"
    pub to: String,
}

/// bundle id → 应用组位掩码。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GroupTable {
    entries: Vec<(String, u64)>,
}

impl GroupTable {
    /// 同一个 bundle id 可以属于多个组。
    pub fn add(&mut self, bundle_id: impl Into<String>, group: u64) {
        let bundle_id = bundle_id.into();
        if let Some((_, mask)) = self.entries.iter_mut().find(|(id, _)| *id == bundle_id) {
            *mask |= group;
        } else {
            self.entries.push((bundle_id, group));
        }
    }

    pub fn lookup(&self, bundle_id: &str) -> u64 {
        self.entries
            .iter()
            .filter(|(id, _)| id == bundle_id)
            .map(|(_, mask)| *mask)
            .fold(0, |acc, mask| acc | mask)
    }
}

/// 编译后的配置：拦截线程和后台线程运行期只读的东西。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Compiled {
    pub rules: Rules,
    pub groups: GroupTable,
    pub dock_click_minimize: bool,
}

fn group_bit(name: &str) -> Option<u64> {
    match name {
        "terminals" => Some(groups::TERMINAL),
        "browsers" => Some(groups::BROWSER),
        "finder" => Some(groups::FINDER),
        "passthrough" => Some(groups::PASSTHROUGH),
        _ => None,
    }
}

/// 解析一段按键描述，如 "Cmd+R"、"Return"、"Ctrl+Shift+S"。
/// 修饰键不区分顺序和大小写；主键必须恰好一个。
pub fn parse_chord(spec: &str) -> Result<Chord, String> {
    let mut flags = 0;
    let mut key: Option<i64> = None;
    for part in spec.split('+') {
        let part = part.trim();
        if part.is_empty() {
            return Err(format!("按键描述 `{spec}` 有空段"));
        }
        let lower = part.to_ascii_lowercase();
        let modifier = match lower.as_str() {
            "cmd" | "command" | "win" | "super" => Some(COMMAND),
            "ctrl" | "control" => Some(CONTROL),
            "alt" | "opt" | "option" => Some(ALTERNATE),
            "shift" => Some(SHIFT),
            _ => None,
        };
        if let Some(bit) = modifier {
            if flags & bit != 0 {
                return Err(format!("按键描述 `{spec}` 里修饰键重复"));
            }
            flags |= bit;
        } else if let Some(code) = keys::named(part) {
            if key.replace(code).is_some() {
                return Err(format!("按键描述 `{spec}` 有两个主键"));
            }
        } else {
            return Err(format!("按键描述 `{spec}` 里有不认识的键 `{part}`"));
        }
    }
    key.map(|keycode| Chord { keycode, flags })
        .ok_or_else(|| format!("按键描述 `{spec}` 缺主键"))
}

/// 编译配置。所有「写错了」都在这里一次报出来，不带病启动。
pub fn compile(cfg: &Config) -> Result<Compiled, String> {
    let mut substitutes = Vec::new();
    for sub in &cfg.substitute {
        let keycode = keys::named(&sub.key)
            .ok_or_else(|| format!("[[substitute]] 里不认识的键 `{}`", sub.key))?;
        let groups = group_bit(&sub.r#when).ok_or_else(|| {
            format!(
                "[[substitute]] 的 when 只能是 terminals/browsers/finder/passthrough，现在是 `{}`",
                sub.r#when
            )
        })?;
        let to = parse_chord(&sub.to).map_err(|e| format!("[[substitute]] {e}"))?;
        substitutes.push(Substitute {
            keycode,
            groups,
            to,
        });
    }

    let mut groups = GroupTable::default();
    let mut fill = |ids: &[String], bit: u64| -> Result<(), String> {
        for id in ids {
            if id.trim().is_empty() {
                return Err("[apps] 里有空的 bundle id".into());
            }
            groups.add(id.trim(), bit);
        }
        Ok(())
    };
    fill(&cfg.apps.terminals, groups::TERMINAL)?;
    fill(&cfg.apps.browsers, groups::BROWSER)?;
    fill(&cfg.apps.finder, groups::FINDER)?;
    fill(&cfg.apps.passthrough, groups::PASSTHROUGH)?;

    Ok(Compiled {
        rules: Rules {
            ctrl_as_cmd: cfg.remap.ctrl_as_cmd,
            cmd_as_ctrl: cfg.remap.cmd_as_ctrl,
            alt_tab: cfg.remap.alt_tab,
            ctrl_arrows: cfg.remap.ctrl_arrows,
            win_key_lock: cfg.remap.win_key_lock,
            win_key_clipboard: cfg.remap.win_key_clipboard,
            task_manager: cfg.remap.task_manager,
            settings_hotkey: cfg.remap.settings_hotkey,
            shortcuts_peek: cfg.remap.shortcuts_peek,
            substitutes,
        },
        groups,
        dock_click_minimize: cfg.remap.dock_click_minimize,
    })
}

/// 解析配置文本。
pub fn parse(text: &str) -> Result<Config, String> {
    toml::from_str(text).map_err(|e| format!("配置文件解析失败：{e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keys::{F2, F5, R, RETURN};

    #[test]
    fn empty_text_gives_defaults() {
        let cfg = parse("").unwrap();
        assert_eq!(cfg, Config::default());
    }

    #[test]
    fn example_is_exactly_the_defaults() {
        let text = parse(EXAMPLE).unwrap();
        assert_eq!(text, Config::default());
        let compiled = compile(&text).unwrap();
        assert_eq!(compiled, compile(&Config::default()).unwrap());
        assert_eq!(compiled.rules.substitutes.len(), 2);
        assert_eq!(compiled.groups.lookup("com.google.Chrome"), groups::BROWSER);
        assert_eq!(
            compiled.groups.lookup("com.vmware.fusion"),
            groups::PASSTHROUGH
        );
        assert!(compiled.dock_click_minimize);
    }

    #[test]
    fn default_compiles_and_maps_default_apps() {
        let compiled = compile(&Config::default()).unwrap();
        assert_eq!(
            compiled.groups.lookup("com.apple.Terminal"),
            groups::TERMINAL
        );
        assert_eq!(compiled.groups.lookup("com.apple.finder"), groups::FINDER);
        assert_eq!(compiled.groups.lookup("com.apple.Safari"), groups::BROWSER);
        assert_eq!(compiled.groups.lookup("com.unknown.app"), 0);
        // F2 → 回车、F5 → ⌘R 是用户点名要的，默认就有
        assert_eq!(compiled.rules.substitutes.len(), 2);
        assert!(compiled.rules.win_key_clipboard, "Win+V 剪贴板面板默认开");
    }

    #[test]
    fn one_app_can_belong_to_several_groups() {
        let mut t = GroupTable::default();
        t.add("x.y", groups::TERMINAL);
        t.add("x.y", groups::BROWSER);
        t.add("a.b", groups::FINDER);
        assert_eq!(t.lookup("x.y"), groups::TERMINAL | groups::BROWSER);
        assert_eq!(t.lookup("a.b"), groups::FINDER);
        assert_eq!(t.entries.len(), 2);
    }

    #[test]
    fn substitutes_default_when_omitted_and_can_be_disabled() {
        // 只写了 [remap]：单键替换仍是默认两条
        let cfg = parse("[remap]\nalt_tab = false\n").unwrap();
        assert_eq!(cfg.substitute.len(), 2);
        assert!(!compile(&cfg).unwrap().rules.alt_tab);
        // 显式写空数组：全部关掉
        let cfg = parse("substitute = []\n").unwrap();
        assert!(compile(&cfg).unwrap().rules.substitutes.is_empty());
    }

    #[test]
    fn unknown_field_is_rejected() {
        let err = parse("[remap]\nctrl_to_comand = true\n").unwrap_err();
        assert!(err.contains("ctrl_to_comand"), "{err}");
        let err = parse(
            "[[substitute]]\nkey = \"F2\"\nwhen = \"finder\"\nto = \"Return\"\nnote = \"x\"\n",
        )
        .unwrap_err();
        assert!(err.contains("note"), "{err}");
    }

    #[test]
    fn bad_substitute_is_rejected() {
        let cfg = |key: &str, when: &str, to: &str| Config {
            substitute: vec![SubstituteConfig {
                key: key.into(),
                r#when: when.into(),
                to: to.into(),
            }],
            ..Config::default()
        };
        let err = compile(&cfg("F99", "finder", "Return")).unwrap_err();
        assert!(err.contains("F99"), "{err}");
        let err = compile(&cfg("F2", "editor", "Return")).unwrap_err();
        assert!(err.contains("editor"), "{err}");
        let err = compile(&cfg("F2", "finder", "Cmd+Nope")).unwrap_err();
        assert!(err.contains("Nope"), "{err}");
        let err = compile(&cfg("F2", "finder", "Cmd")).unwrap_err();
        assert!(err.contains("主键"), "{err}");
    }

    #[test]
    fn empty_bundle_id_is_rejected() {
        let cfg = Config {
            apps: AppsConfig {
                terminals: vec!["  ".into()],
                ..AppsConfig::default()
            },
            ..Config::default()
        };
        assert!(compile(&cfg).unwrap_err().contains("bundle id"));
    }

    #[test]
    fn substitutes_compile_to_chords() {
        let cfg = parse(EXAMPLE).unwrap();
        let compiled = compile(&cfg).unwrap();
        assert_eq!(
            compiled.rules.substitutes,
            vec![
                Substitute {
                    keycode: F2,
                    groups: groups::FINDER,
                    to: Chord {
                        keycode: RETURN,
                        flags: 0
                    }
                },
                Substitute {
                    keycode: F5,
                    groups: groups::BROWSER,
                    to: Chord {
                        keycode: R,
                        flags: COMMAND
                    }
                },
            ]
        );
    }

    #[test]
    fn chord_parsing() {
        assert_eq!(
            parse_chord("Return").unwrap(),
            Chord {
                keycode: RETURN,
                flags: 0
            }
        );
        assert_eq!(
            parse_chord("Cmd+R").unwrap(),
            Chord {
                keycode: R,
                flags: COMMAND
            }
        );
        assert_eq!(
            parse_chord("R + shift + cmd").unwrap(),
            Chord {
                keycode: R,
                flags: SHIFT | COMMAND
            }
        );
        assert_eq!(parse_chord("win+q").unwrap().flags, COMMAND);
        assert_eq!(parse_chord("opt+Left").unwrap().flags, ALTERNATE);
        assert_eq!(parse_chord("ctrl+F5").unwrap().flags, CONTROL);
        assert!(parse_chord("Cmd+Cmd+R").is_err());
        assert!(parse_chord("R+T").is_err());
        assert!(parse_chord("Cmd+").is_err());
        assert!(parse_chord("").is_err());
    }
}
