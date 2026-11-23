//! macOS 虚拟键码（kVK_*）。数值对照本机 SDK 的 `Carbon/HIToolbox/Events.h`。
//!
//! 虚拟键码对应**物理键位**，不随输入法和键盘布局变化，所以拦截层只认它。
//! 普通外接键盘上，Windows 布局的 Win / Alt / Ctrl 在 macOS 里就是 ⌘ / ⌥ / ⌃，
//! 键码与 Mac 键盘相同。

pub const A: i64 = 0x00;
pub const C: i64 = 0x08;
pub const L: i64 = 0x25;
pub const Q: i64 = 0x0C;
pub const R: i64 = 0x0F;
pub const V: i64 = 0x09;

pub const RETURN: i64 = 0x24;
pub const TAB: i64 = 0x30;
pub const SPACE: i64 = 0x31;
pub const DELETE: i64 = 0x33;
pub const ESCAPE: i64 = 0x35;

pub const RIGHT_COMMAND: i64 = 0x36;
pub const COMMAND: i64 = 0x37;
pub const OPTION: i64 = 0x3A;
pub const CONTROL: i64 = 0x3B;
pub const RIGHT_OPTION: i64 = 0x3D;
pub const RIGHT_CONTROL: i64 = 0x3E;

pub const F2: i64 = 0x78;
pub const F5: i64 = 0x60;

pub const LEFT: i64 = 0x7B;
pub const RIGHT: i64 = 0x7C;
pub const DOWN: i64 = 0x7D;
pub const UP: i64 = 0x7E;

/// 配置里可写的键名 → 虚拟键码。查找前统一转小写。
pub const NAMED: &[(&str, i64)] = &[
    ("return", RETURN),
    ("enter", RETURN),
    ("tab", TAB),
    ("space", SPACE),
    ("delete", DELETE),
    ("backspace", DELETE),
    ("escape", ESCAPE),
    ("esc", ESCAPE),
    ("left", LEFT),
    ("right", RIGHT),
    ("up", UP),
    ("down", DOWN),
    ("home", 0x73),
    ("end", 0x77),
    ("pageup", 0x74),
    ("pagedown", 0x79),
    ("forwarddelete", 0x75),
    ("f1", 0x7A),
    ("f2", F2),
    ("f3", 0x63),
    ("f4", 0x76),
    ("f5", F5),
    ("f6", 0x61),
    ("f7", 0x62),
    ("f8", 0x64),
    ("f9", 0x65),
    ("f10", 0x6D),
    ("f11", 0x67),
    ("f12", 0x6F),
    ("0", 0x1D),
    ("1", 0x12),
    ("2", 0x13),
    ("3", 0x14),
    ("4", 0x15),
    ("5", 0x17),
    ("6", 0x16),
    ("7", 0x1A),
    ("8", 0x1C),
    ("9", 0x19),
    ("a", A),
    ("b", 0x0B),
    ("c", C),
    ("d", 0x02),
    ("e", 0x0E),
    ("f", 0x03),
    ("g", 0x05),
    ("h", 0x04),
    ("i", 0x22),
    ("j", 0x26),
    ("k", 0x28),
    ("l", L),
    ("m", 0x2E),
    ("n", 0x2D),
    ("o", 0x1F),
    ("p", 0x23),
    ("q", Q),
    ("r", R),
    ("s", 0x01),
    ("t", 0x11),
    ("u", 0x20),
    ("v", V),
    ("w", 0x0D),
    ("x", 0x07),
    ("y", 0x10),
    ("z", 0x06),
];

/// 键名转虚拟键码；找不到返回 None。
pub fn named(name: &str) -> Option<i64> {
    let lower = name.trim().to_ascii_lowercase();
    NAMED
        .iter()
        .find(|(n, _)| *n == lower)
        .map(|(_, code)| *code)
}
