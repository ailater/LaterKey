//! LaterKey 纯逻辑层：重映射规则与修饰键状态机、剪贴板历史。
//!
//! 不依赖任何系统 API，禁止 unsafe。平台层把事件读成 [`Input`] 交给
//! [`Remapper`]，拿回一个 [`Decision`] 去执行。剪贴板历史的
//! 数据结构在 [`clipboard`]，文件读写由 app 侧负责。

#![forbid(unsafe_code)]

pub mod clipboard;
pub mod config;
pub mod flags;
pub mod keys;
pub mod remap;
pub mod shortcuts;

pub use clipboard::{
    png_dimensions, ClipboardEntry, History, ImageData, ImageFormat, MAX_ENTRIES, MAX_IMAGE_BYTES,
    MAX_IMAGE_TOTAL_BYTES, MAX_TEXT_BYTES,
};
pub use config::{compile, parse, parse_chord, CheatsheetConfig, Compiled, Config, GroupTable};
pub use remap::{
    groups, Action, Chord, Decision, Env, Input, Kind, Remapper, Rules, Substitute, TerminalOpen,
};
pub use shortcuts::{
    ax_modifiers_to_flags, format_chord, format_chord_parts, key_symbol, normalize_char,
    virtual_key_name, DisplayRules, ShortcutEntry, ShortcutGroup, ShortcutKey, Style,
};
