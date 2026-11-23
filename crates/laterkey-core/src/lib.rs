//! LaterKey 纯逻辑层：重映射规则与修饰键状态机。
//!
//! 不依赖任何系统 API，禁止 unsafe。平台层把事件读成 [`Input`] 交给
//! [`Remapper`]，拿回一个 [`Decision`] 去执行。

#![forbid(unsafe_code)]

pub mod config;
pub mod flags;
pub mod keys;
pub mod remap;

pub use config::{compile, parse, parse_chord, Compiled, Config, GroupTable};
pub use remap::{groups, Action, Chord, Decision, Env, Input, Kind, Remapper, Rules, Substitute};
