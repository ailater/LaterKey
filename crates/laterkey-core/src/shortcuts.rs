//! 快捷键速览的数据模型与显示换算。纯逻辑，不碰系统 API。
//!
//! 数据源（平台层采集）统一归一成 [`ShortcutEntry`]：修饰键一律是
//! CGEventFlags 风格的类别位。显示换算负责回答用户真正关心的问题：
//! 「我这只 Windows 布局键盘上该按什么」——物理 Ctrl 产生 ⌘、物理 Win
//! 产生 ⌃，所以同一条 ⌘C，物理键口径下要显示成 Ctrl+C。

use crate::flags;
use crate::keys;

/// 一条快捷键（速览窗口的一行）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShortcutEntry {
    /// 菜单项名或系统功能名，如「拷贝」「调度中心」
    pub title: String,
    pub key: ShortcutKey,
    /// CGEventFlags 类别位（SHIFT/CONTROL/ALTERNATE/COMMAND/SECONDARY_FN），
    /// 不含设备位
    pub modifiers: u64,
}

/// 主键：菜单给的是字符，系统快捷键只有虚拟键码。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ShortcutKey {
    /// 菜单项自带的快捷键字符（"C"、"F1"、特殊符号），显示前先过 [`normalize_char`]
    Char(String),
    VirtualKey(i64),
}

/// 一组快捷键（一个菜单或一个系统功能分组）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShortcutGroup {
    pub title: String,
    pub entries: Vec<ShortcutEntry>,
}

/// 显示口径。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Style {
    /// macOS 原生命名：Command+Shift+C
    Mac,
    /// 物理键名（按当前互换规则换算）：Ctrl+Shift+C。默认口径——
    /// 用户看速览就是要知道「这只 Windows 键盘该按什么」
    #[default]
    Physical,
}

/// 影响显示换算的运行期开关。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DisplayRules {
    /// 物理 Ctrl 当作 ⌘：⌘ 显示为 Ctrl
    pub ctrl_as_cmd: bool,
    /// 物理 Win 当作 ⌃：⌃ 显示为 Win
    pub cmd_as_ctrl: bool,
}

impl Default for DisplayRules {
    fn default() -> Self {
        Self {
            ctrl_as_cmd: true,
            cmd_as_ctrl: true,
        }
    }
}

/// AXMenuItemCmdModifiers 掩码 → CGEventFlags 类别位。
///
/// 本机（macOS 27.2）实测含义：0=⌘、1=⇧⌘、2=⌥⌘、3=⇧⌥⌘、4=⌃⌘、24=fn，
/// 即 bit0=⇧、bit1=⌥、bit2=⌃ 按位组合，⌘ 是默认基底；bit3（8）置位表示
/// 「⌘ 不参与」，bit4（16）置位表示带 fn。
pub fn ax_modifiers_to_flags(mask: i64) -> u64 {
    let mut out = 0;
    if mask & 0b0_0001 != 0 {
        out |= flags::SHIFT;
    }
    if mask & 0b0_0010 != 0 {
        out |= flags::ALTERNATE;
    }
    if mask & 0b0_0100 != 0 {
        out |= flags::CONTROL;
    }
    // ⌘ 是基底：bit3 没置位就带着
    if mask & 0b0_1000 == 0 {
        out |= flags::COMMAND;
    }
    if mask & 0b1_0000 != 0 {
        out |= flags::SECONDARY_FN;
    }
    out
}

/// 菜单里的快捷键字符 → 文字形式的键名（物理键口径用）。
///
/// 特殊符号和控制字符都换成文字：键帽符号只在 Mac 口径里用（见
/// [`keycap_char`]），文字口径不混排符号。
pub fn normalize_char(ch: &str) -> String {
    match ch {
        " " => "空格".into(),
        "\x1b" => "Esc".into(),
        "\t" => "Tab".into(),
        "\r" | "\n" => "回车".into(),
        "\x08" => "退格".into(),
        _ => match ch.trim() {
            "⎋" | "␛" => "Esc".into(),
            "⇥" => "Tab".into(),
            "↩" | "⏎" | "↖" => "回车".into(),
            "⌫" => "退格".into(),
            "⌦" => "Del".into(),
            "␣" => "空格".into(),
            "⇧" => "Shift".into(),
            "⇪" => "CapsLock".into(),
            other => other.trim().to_string(),
        },
    }
}

/// 菜单里的快捷键字符 → Mac 口径的原生键帽符号，与系统菜单里显示的一致。
///
/// 字形由速览的键帽字体链提供：⎋⌫⌦ 在 SF 里，␣⇥↩ 在 Apple Symbols 里
/// （两者都逐个码位核对过，见 font.rs）。
pub fn keycap_char(ch: &str) -> String {
    match ch {
        " " => "␣".into(),
        "\x1b" => "⎋".into(),
        "\t" => "⇥".into(),
        "\r" | "\n" => "↩".into(),
        "\x08" => "⌫".into(),
        _ => match ch.trim() {
            "␛" => "⎋".into(),
            "⏎" | "↖" => "↩".into(),
            other => other.trim().to_string(),
        },
    }
}

/// 虚拟键码 → 文字形式的键名。数字与字母回退大写字符。
pub fn virtual_key_name(code: i64) -> Option<String> {
    let named = match code {
        keys::RETURN => "回车",
        keys::TAB => "Tab",
        keys::SPACE => "空格",
        keys::DELETE => "退格",
        0x75 => "Del",
        keys::ESCAPE => "Esc",
        keys::LEFT => "←",
        keys::RIGHT => "→",
        keys::UP => "↑",
        keys::DOWN => "↓",
        0x73 => "Home",
        0x77 => "End",
        0x74 => "PageUp",
        0x79 => "PageDown",
        0x7A => "F1",
        0x78 => "F2",
        0x63 => "F3",
        0x76 => "F4",
        0x60 => "F5",
        0x61 => "F6",
        0x62 => "F7",
        0x64 => "F8",
        0x65 => "F9",
        0x6D => "F10",
        0x67 => "F11",
        0x6F => "F12",
        _ => {
            return keys::NAMED
                .iter()
                .find(|(_, k)| *k == code)
                .map(|(name, _)| name.to_uppercase())
        }
    };
    Some(named.into())
}

/// 虚拟键码 → Mac 口径的原生键帽符号。数字、字母、F 键与文字口径相同。
pub fn virtual_key_symbol(code: i64) -> Option<String> {
    let symbol = match code {
        keys::RETURN => "↩",
        keys::TAB => "⇥",
        keys::SPACE => "␣",
        keys::DELETE => "⌫",
        0x75 => "⌦",
        keys::ESCAPE => "⎋",
        0x73 => "↖",
        0x77 => "↘",
        0x74 => "⇞",
        0x79 => "⇟",
        _ => return virtual_key_name(code),
    };
    Some(symbol.into())
}

/// 主键的文字形式键名。读不出来返回 None（调用方跳过该条）。
pub fn key_label(key: &ShortcutKey) -> Option<String> {
    match key {
        ShortcutKey::Char(ch) => {
            let normalized = normalize_char(ch);
            (!normalized.is_empty()).then_some(normalized)
        }
        ShortcutKey::VirtualKey(code) => virtual_key_name(*code),
    }
}

/// 主键的 Mac 口径键帽符号。读不出来返回 None（调用方跳过该条）。
pub fn key_symbol(key: &ShortcutKey) -> Option<String> {
    match key {
        ShortcutKey::Char(ch) => {
            let symbol = keycap_char(ch);
            (!symbol.is_empty()).then_some(symbol)
        }
        ShortcutKey::VirtualKey(code) => virtual_key_symbol(*code),
    }
}

/// 修饰键的显示名。物理口径按 Windows 惯例 Win 最先（⌃⌘ 显示为
/// Win+Ctrl），其余 Ctrl→Shift→Alt；Mac 口径用系统键帽符号
/// （fn⌃⌥⇧⌘，见 [`format_chord`]），一般不再经过这里。
pub fn modifier_labels(modifiers: u64, style: Style, d: &DisplayRules) -> Vec<String> {
    let push = |out: &mut Vec<String>, on: bool, label: &str| {
        if on {
            out.push(label.to_string());
        }
    };
    let mut out = Vec::new();
    if style == Style::Mac {
        push(&mut out, modifiers & flags::SECONDARY_FN != 0, "fn");
        push(&mut out, modifiers & flags::CONTROL != 0, "Control");
        push(&mut out, modifiers & flags::ALTERNATE != 0, "Option");
        push(&mut out, modifiers & flags::SHIFT != 0, "Shift");
        push(&mut out, modifiers & flags::COMMAND != 0, "Command");
    } else {
        push(&mut out, modifiers & flags::SECONDARY_FN != 0, "fn");
        push(
            &mut out,
            modifiers & flags::CONTROL != 0,
            if d.cmd_as_ctrl { "Win" } else { "Control" },
        );
        push(
            &mut out,
            modifiers & flags::COMMAND != 0,
            if d.ctrl_as_cmd { "Ctrl" } else { "Command" },
        );
        push(&mut out, modifiers & flags::SHIFT != 0, "Shift");
        push(&mut out, modifiers & flags::ALTERNATE != 0, "Alt");
    }
    out
}

/// 一条快捷键拆成「修饰键」与「主键」两段显示文本。
///
/// 速览把两段分开定位（修饰键右对齐、主键左对齐，与系统菜单一致），所以
/// 不能只给一个拼好的串。Mac 口径的修饰键是键帽符号 `fn ⌃⌥⇧⌘`，主键是
/// 原生键帽符号；物理键口径是 `Ctrl+Shift+` 加文字键名。主键读不出来返回
/// None（调用方跳过该条）。
pub fn format_chord_parts(
    modifiers: u64,
    key: &ShortcutKey,
    style: Style,
    d: &DisplayRules,
) -> Option<(String, String)> {
    match style {
        Style::Mac => {
            let mut mods = String::new();
            if modifiers & flags::SECONDARY_FN != 0 {
                mods.push_str("fn ");
            }
            if modifiers & flags::CONTROL != 0 {
                mods.push('⌃');
            }
            if modifiers & flags::ALTERNATE != 0 {
                mods.push('⌥');
            }
            if modifiers & flags::SHIFT != 0 {
                mods.push('⇧');
            }
            if modifiers & flags::COMMAND != 0 {
                mods.push('⌘');
            }
            Some((mods, key_symbol(key)?))
        }
        Style::Physical => {
            let mut labels = modifier_labels(modifiers, style, d);
            if !labels.is_empty() {
                labels.push(String::new());
            }
            Some((labels.join("+"), key_label(key)?))
        }
    }
}

/// 一条快捷键的完整显示文本：Mac 口径 `⇧⌘C`、`fn D`；物理键口径
/// `Ctrl+Shift+C`。主键读不出来返回 None（调用方跳过该条）。
pub fn format_chord(
    modifiers: u64,
    key: &ShortcutKey,
    style: Style,
    d: &DisplayRules,
) -> Option<String> {
    let (mods, key) = format_chord_parts(modifiers, key, style, d)?;
    Some(mods + &key)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::flags::{ALTERNATE, COMMAND, CONTROL, SECONDARY_FN, SHIFT};

    #[test]
    fn ax_modifier_masks_decode_per_measured_meanings() {
        // 本机实测值逐个钉住：这些数字错一位，速览窗口整列都会标错
        assert_eq!(ax_modifiers_to_flags(0), COMMAND);
        assert_eq!(ax_modifiers_to_flags(1), SHIFT | COMMAND);
        assert_eq!(ax_modifiers_to_flags(2), ALTERNATE | COMMAND);
        assert_eq!(ax_modifiers_to_flags(3), SHIFT | ALTERNATE | COMMAND);
        assert_eq!(ax_modifiers_to_flags(4), CONTROL | COMMAND);
        // 24 = 8(无 ⌘) + 16(fn)：开始听写 fn+D
        assert_eq!(ax_modifiers_to_flags(24), SECONDARY_FN);
        // bit3 单独置位：完全无修饰键
        assert_eq!(ax_modifiers_to_flags(8), 0);
        assert_eq!(ax_modifiers_to_flags(12), CONTROL);
    }

    #[test]
    fn special_chars_become_text() {
        assert_eq!(normalize_char("⎋"), "Esc");
        assert_eq!(normalize_char("↩"), "回车");
        assert_eq!(normalize_char("⌫"), "退格");
        assert_eq!(normalize_char(" C "), "C");
        assert_eq!(normalize_char(""), "");
        assert_eq!(normalize_char(" "), "空格");
        assert_eq!(normalize_char("\x1b"), "Esc");
    }

    #[test]
    fn keycap_forms_keep_system_menu_glyphs() {
        assert_eq!(keycap_char("⎋"), "⎋");
        assert_eq!(keycap_char("\x1b"), "⎋");
        assert_eq!(keycap_char("\t"), "⇥");
        assert_eq!(keycap_char("↩"), "↩");
        assert_eq!(keycap_char("⌫"), "⌫");
        assert_eq!(keycap_char(" "), "␣");
        assert_eq!(keycap_char(" C "), "C");
    }

    #[test]
    fn virtual_keys_have_readable_names() {
        assert_eq!(virtual_key_name(keys::RETURN).as_deref(), Some("回车"));
        assert_eq!(virtual_key_name(keys::ESCAPE).as_deref(), Some("Esc"));
        assert_eq!(virtual_key_name(keys::LEFT).as_deref(), Some("←"));
        assert_eq!(virtual_key_name(0x75).as_deref(), Some("Del"));
        assert_eq!(virtual_key_name(keys::F2).as_deref(), Some("F2"));
        assert_eq!(virtual_key_name(keys::C).as_deref(), Some("C"));
        assert_eq!(virtual_key_name(0x999), None);
    }

    #[test]
    fn virtual_keys_have_keycap_symbols() {
        assert_eq!(virtual_key_symbol(keys::RETURN).as_deref(), Some("↩"));
        assert_eq!(virtual_key_symbol(keys::TAB).as_deref(), Some("⇥"));
        assert_eq!(virtual_key_symbol(keys::SPACE).as_deref(), Some("␣"));
        assert_eq!(virtual_key_symbol(keys::DELETE).as_deref(), Some("⌫"));
        assert_eq!(virtual_key_symbol(0x75).as_deref(), Some("⌦"));
        assert_eq!(virtual_key_symbol(keys::ESCAPE).as_deref(), Some("⎋"));
        assert_eq!(virtual_key_symbol(keys::UP).as_deref(), Some("↑"));
        assert_eq!(virtual_key_symbol(0x67).as_deref(), Some("F11"));
        assert_eq!(virtual_key_symbol(keys::C).as_deref(), Some("C"));
        assert_eq!(virtual_key_symbol(0x999), None);
    }

    #[test]
    fn chords_format_in_both_styles() {
        let d = DisplayRules::default();
        let c = ShortcutKey::Char("C".into());
        // ⌘C：物理口径是 Ctrl+C（互恰好开着时）
        assert_eq!(
            format_chord(COMMAND, &c, Style::Physical, &d).as_deref(),
            Some("Ctrl+C")
        );
        assert_eq!(
            format_chord(COMMAND, &c, Style::Mac, &d).as_deref(),
            Some("⌘C")
        );
        // ⌃⌘Q：物理口径 Win+Ctrl+Q（ROADMAP 的例子）
        let q = ShortcutKey::Char("Q".into());
        assert_eq!(
            format_chord(CONTROL | COMMAND, &q, Style::Physical, &d).as_deref(),
            Some("Win+Ctrl+Q")
        );
        assert_eq!(
            format_chord(CONTROL | COMMAND, &q, Style::Mac, &d).as_deref(),
            Some("⌃⌘Q")
        );
        // ⇧⌥⌘3：物理口径 Ctrl→Shift→Alt（Windows 惯例），Mac 口径 fn→⌃→⌥→⇧→⌘
        let three = ShortcutKey::Char("3".into());
        assert_eq!(
            format_chord(SHIFT | ALTERNATE | COMMAND, &three, Style::Physical, &d).as_deref(),
            Some("Ctrl+Shift+Alt+3")
        );
        assert_eq!(
            format_chord(SHIFT | ALTERNATE | COMMAND, &three, Style::Mac, &d).as_deref(),
            Some("⌥⇧⌘3")
        );
        // fn+D：fn 在最前
        let dd = ShortcutKey::Char("D".into());
        assert_eq!(
            format_chord(SECONDARY_FN, &dd, Style::Physical, &d).as_deref(),
            Some("fn+D")
        );
        assert_eq!(
            format_chord(SECONDARY_FN, &dd, Style::Mac, &d).as_deref(),
            Some("fn D")
        );
    }

    #[test]
    fn chord_parts_split_modifiers_from_the_main_key() {
        let d = DisplayRules::default();
        let c = ShortcutKey::Char("C".into());
        // Mac：修饰键是键帽符号，主键是原生键帽符号
        assert_eq!(
            format_chord_parts(CONTROL | SHIFT, &c, Style::Mac, &d),
            Some(("⌃⇧".into(), "C".into()))
        );
        assert_eq!(
            format_chord_parts(SECONDARY_FN, &c, Style::Mac, &d),
            Some(("fn ".into(), "C".into()))
        );
        assert_eq!(
            format_chord_parts(0, &ShortcutKey::VirtualKey(keys::SPACE), Style::Mac, &d),
            Some((String::new(), "␣".into()))
        );
        // 物理：修饰键带连接符，主键是文字
        assert_eq!(
            format_chord_parts(CONTROL | SHIFT, &c, Style::Physical, &d),
            Some(("Win+Shift+".into(), "C".into()))
        );
        assert_eq!(
            format_chord_parts(
                0,
                &ShortcutKey::VirtualKey(keys::SPACE),
                Style::Physical,
                &d
            ),
            Some((String::new(), "空格".into()))
        );
    }

    #[test]
    fn physical_names_follow_the_toggles() {
        // 互换关掉时：⌘ 就叫 Command、⌃ 就叫 Control——显示回 Mac 原义
        let d = DisplayRules {
            ctrl_as_cmd: false,
            cmd_as_ctrl: false,
        };
        let c = ShortcutKey::Char("C".into());
        assert_eq!(
            format_chord(COMMAND, &c, Style::Physical, &d).as_deref(),
            Some("Command+C")
        );
        let a = ShortcutKey::Char("A".into());
        assert_eq!(
            format_chord(CONTROL, &a, Style::Physical, &d).as_deref(),
            Some("Control+A")
        );
        // 只关一半：各换各的
        let d = DisplayRules {
            ctrl_as_cmd: true,
            cmd_as_ctrl: false,
        };
        assert_eq!(
            format_chord(CONTROL | COMMAND, &a, Style::Physical, &d).as_deref(),
            Some("Control+Ctrl+A")
        );
    }

    #[test]
    fn unreadable_keys_are_skipped() {
        // 空字符与未知键码都该让调用方跳过，而不是渲染出「Ctrl+」
        assert_eq!(
            format_chord(
                COMMAND,
                &ShortcutKey::Char("".into()),
                Style::Physical,
                &DisplayRules::default()
            ),
            None
        );
        assert_eq!(
            format_chord(
                COMMAND,
                &ShortcutKey::VirtualKey(0x999),
                Style::Physical,
                &DisplayRules::default()
            ),
            None
        );
    }
}
