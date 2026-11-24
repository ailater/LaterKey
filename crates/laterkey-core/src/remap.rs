//! 重映射状态机：一个事件进来，一个决定出去，全程不碰系统。
//!
//! 回调（平台层）只负责把 CGEvent 读成 [`Input`]、把 [`Decision`] 写回事件，
//! 这样修饰键的所有时序都能用单元测试覆盖，不必手敲键盘验证。

use crate::flags::{self, ALTERNATE, ANY_MODIFIER, COMMAND, CONTROL, L_ALT};
use crate::keys::{
    self, DELETE, ESCAPE, L, LEFT, RIGHT, RIGHT_COMMAND, RIGHT_CONTROL, SPACE, TAB, V,
};

/// 事件类型。修饰键没有 KeyDown/KeyUp，只有 FlagsChanged。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    FlagsChanged,
    KeyDown,
    KeyUp,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Input {
    pub kind: Kind,
    pub keycode: i64,
    /// 事件自带的修饰键标志（改写前的原始值）
    pub flags: u64,
    /// 按住不放产生的重复事件
    pub repeat: bool,
}

impl Input {
    pub fn flags_changed(keycode: i64, flags: u64) -> Self {
        Self {
            kind: Kind::FlagsChanged,
            keycode,
            flags,
            repeat: false,
        }
    }

    pub fn key_down(keycode: i64, flags: u64) -> Self {
        Self {
            kind: Kind::KeyDown,
            keycode,
            flags,
            repeat: false,
        }
    }

    pub fn key_up(keycode: i64, flags: u64) -> Self {
        Self {
            kind: Kind::KeyUp,
            keycode,
            flags,
            repeat: false,
        }
    }

    pub fn repeated(mut self) -> Self {
        self.repeat = true;
        self
    }
}

/// 应用组：前台应用命中的配置分组，一组一位。
pub mod groups {
    /// 终端：Ctrl 组合保持原义（Ctrl+C 仍是中断）
    pub const TERMINAL: u64 = 1 << 0;
    /// 浏览器：裸 F5 当 ⌘R
    pub const BROWSER: u64 = 1 << 1;
    /// 访达：裸 F2 当回车
    pub const FINDER: u64 = 1 << 2;
    /// 整体透传（远程桌面、虚拟机）：什么都不改
    pub const PASSTHROUGH: u64 = 1 << 3;
}

/// 回调入口一次读出的环境快照。只放能原子读取的值，回调内不加锁、不发消息。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Env {
    pub groups: u64,
}

impl Env {
    pub fn has(&self, group: u64) -> bool {
        self.groups & group != 0
    }
}

/// 吞掉原键后要执行的动作，转给后台线程。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    ClipboardHistory,
    LockScreen,
    /// 呼出 macOS 强制退出窗口（任务管理器对应物）
    TaskManager,
}

/// 一组合成按键：键码 + 标志位。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Chord {
    pub keycode: i64,
    pub flags: u64,
}

/// 对一个事件的处置。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decision {
    /// 原样放行
    Keep,
    /// 吞掉
    Consume,
    /// 就地改写后放行。不注入新事件，进程崩溃也不会留下卡住的修饰键
    Rewrite { keycode: i64, flags: u64 },
    /// 吞掉并执行动作
    Action(Action),
    /// 吞掉并合成另一组按键
    Substitute(Chord),
}

/// 按前台应用限定的单键替换。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Substitute {
    pub keycode: i64,
    pub groups: u64,
    pub to: Chord,
}

/// 生效的规则。由配置编译而来，运行期只读。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rules {
    /// 物理 Ctrl 当作 ⌘ + 字母（terminals 组里保持 ⌃ 原义）
    pub ctrl_as_cmd: bool,
    /// 物理 Win(⌘) 当作 ⌃：终端里的中断/控制键（Win+C = Ctrl+C）
    pub cmd_as_ctrl: bool,
    /// 左 Alt + Tab 切换应用
    pub alt_tab: bool,
    /// Ctrl + ←/→/退格 按词跳转/删除（⌥）
    pub ctrl_arrows: bool,
    /// Win(⌘) + L 锁屏
    pub win_key_lock: bool,
    /// Win(⌘) + V 剪贴板历史
    pub win_key_clipboard: bool,
    /// 任务管理器：物理 Ctrl+Alt+Del(退格) 或物理 Ctrl+Esc 呼出强制退出
    pub task_manager: bool,
    pub substitutes: Vec<Substitute>,
}

impl Default for Rules {
    fn default() -> Self {
        Self {
            ctrl_as_cmd: true,
            cmd_as_ctrl: true,
            alt_tab: true,
            ctrl_arrows: true,
            win_key_lock: true,
            // Win+V 弹剪贴板历史面板；⌘V（Ctrl+V 改写来的）不受影响
            win_key_clipboard: true,
            task_manager: true,
            substitutes: Vec::new(),
        }
    }
}

/// 被吞掉的按键的键码。容量固定：同时按住又没松开的键不会超过几个。
#[derive(Debug, Default)]
struct Swallowed {
    keys: [i64; 4],
    len: u8,
}

impl Swallowed {
    fn has(&self, keycode: i64) -> bool {
        self.keys[..self.len as usize].contains(&keycode)
    }

    fn push(&mut self, keycode: i64) {
        if self.has(keycode) {
            return;
        }
        if self.len as usize == self.keys.len() {
            // 满了丢最旧的
            self.keys.copy_within(1..4, 0);
            self.len = 3;
        }
        self.keys[self.len as usize] = keycode;
        self.len += 1;
    }

    fn take(&mut self, keycode: i64) -> bool {
        let Some(i) = self.keys[..self.len as usize]
            .iter()
            .position(|&k| k == keycode)
        else {
            return false;
        };
        self.keys.copy_within(i + 1..self.len as usize, i);
        self.len -= 1;
        true
    }
}

/// 重映射状态机。只在拦截线程上使用。
#[derive(Debug)]
pub struct Remapper {
    rules: Rules,
    /// 当前物理按住的 Ctrl（L_CTL | R_CTL 设备位）
    ctl_held: u64,
    /// 本轮按住的 Ctrl 正以 ⌘ 的身份出现
    ctl_as_cmd: bool,
    /// 当前物理按住的 Win(⌘)（L_CMD | R_CMD 设备位）
    cmd_held: u64,
    /// 本轮按住的 Win(⌘) 正以 ⌃ 的身份出现
    cmd_as_ctl: bool,
    /// 左 Alt 已配合 Tab 进入切换模式，之后以 ⌘ 的身份出现
    alt_as_cmd: bool,
    swallowed: Swallowed,
}

impl Remapper {
    pub fn new(rules: Rules) -> Self {
        Self {
            rules,
            ctl_held: 0,
            ctl_as_cmd: false,
            cmd_held: 0,
            cmd_as_ctl: false,
            alt_as_cmd: false,
            swallowed: Swallowed::default(),
        }
    }

    pub fn rules(&self) -> &Rules {
        &self.rules
    }

    /// 当前按住的物理 Ctrl 是否正以 ⌘ 的身份出现。
    /// 平台层用它换算**鼠标事件**的修饰键：⌘+点击是 Mac 的跳定义/多选/新标签，
    /// 对应 Windows 的 Ctrl+点击。点击事件不改的话，应用收到的是 ⌃+点击（右键菜单）。
    pub fn ctrl_as_cmd(&self) -> bool {
        self.ctl_as_cmd
    }

    /// 清空跟踪状态。漏看了一段事件流（安全输入期间、拦截层被系统禁用）之后调用。
    pub fn reset(&mut self) {
        self.ctl_held = 0;
        self.ctl_as_cmd = false;
        self.cmd_held = 0;
        self.cmd_as_ctl = false;
        self.alt_as_cmd = false;
        self.swallowed = Swallowed::default();
    }

    pub fn on_input(&mut self, ev: Input, env: &Env) -> Decision {
        // 1) 修饰键自身的按下/松开
        if ev.kind == Kind::FlagsChanged {
            return match ev.keycode {
                keys::CONTROL | RIGHT_CONTROL => self.on_ctrl(ev, env),
                keys::COMMAND | RIGHT_COMMAND => self.on_cmd(ev, env),
                keys::OPTION => self.on_left_alt(ev),
                _ => Decision::Keep,
            };
        }

        // 2) Alt+Tab 切换模式：Tab 以 ⌘ 身份放行，其余按键一律吞掉
        if self.alt_as_cmd {
            if ev.flags & L_ALT == 0 {
                // 自愈：左 Alt 的松开事件没送到，按已松开处理
                self.alt_as_cmd = false;
            } else if ev.keycode == TAB {
                return rewrite_if_changed(ev, flags::swap(ev.flags, &flags::ALT_TO_CMD));
            } else {
                // 切换器开着时其余按键没有意义；不吞的话会被当成 ⌘ 快捷键
                return Decision::Consume;
            }
        }

        // 3) 整体透传的应用：什么都不改
        if env.has(groups::PASSTHROUGH) {
            return Decision::Keep;
        }

        // 4) 之前被吞掉的键：KeyUp 跟着吞，按住重复也吞
        if self.swallowed.has(ev.keycode) {
            match ev.kind {
                Kind::KeyUp => {
                    self.swallowed.take(ev.keycode);
                    return Decision::Consume;
                }
                Kind::KeyDown if ev.repeat => return Decision::Consume,
                // 没等到 KeyUp 的新按下：忘掉旧记录，按新按键处理
                _ => {
                    self.swallowed.take(ev.keycode);
                }
            }
        }

        let raw = ev.flags;
        let mut flags = raw;
        if self.ctl_as_cmd && self.cmd_as_ctl {
            flags = flags::swap_both_ways(flags);
        } else if self.ctl_as_cmd {
            flags = flags::swap(flags, &flags::CTL_TO_CMD);
        } else if self.cmd_as_ctl {
            flags = flags::swap(flags, &flags::CMD_TO_CTL);
        }

        // 5) Alt+Tab 进入：左 Alt 按住时按下 Tab
        if self.rules.alt_tab && ev.kind == Kind::KeyDown && ev.keycode == TAB && raw & L_ALT != 0 {
            self.alt_as_cmd = true;
            return rewrite_if_changed(ev, flags::swap(flags, &flags::ALT_TO_CMD));
        }

        // 6) Win 键动作：必须是「仅物理 ⌘」。用 raw 判断，这样被改写成 ⌘ 的
        //    Ctrl+V 不会被误当成 Win+V
        if ev.kind == Kind::KeyDown && !ev.repeat && bare(raw, COMMAND) {
            let action = match ev.keycode {
                V if self.rules.win_key_clipboard => Some(Action::ClipboardHistory),
                L if self.rules.win_key_lock => Some(Action::LockScreen),
                _ => None,
            };
            if let Some(action) = action {
                self.swallowed.push(ev.keycode);
                return Decision::Action(action);
            }
        }

        // 6.5) 任务管理器（macOS 强制退出窗口）：物理 Ctrl+Alt+Del(退格) 或
        //      物理 Ctrl+Esc。同样用 raw 物理位判断，与改写无关；组合里不含
        //      物理 ⌘，不会被上面的 Win 键动作截走。
        if self.rules.task_manager && ev.kind == Kind::KeyDown && !ev.repeat {
            // 修饰键必须精确等于（bare 是「含其中之一」，会把 Ctrl+退格 也吸进来）
            let mods = raw & ANY_MODIFIER;
            let hit = (ev.keycode == DELETE && mods == CONTROL | ALTERNATE)
                || (ev.keycode == ESCAPE && mods == CONTROL);
            if hit {
                self.swallowed.push(ev.keycode);
                return Decision::Action(Action::TaskManager);
            }
        }

        // 7) 单键替换（F2 → 回车等）：无修饰键时按前台应用限定
        if ev.kind == Kind::KeyDown && !ev.repeat && raw & ANY_MODIFIER == 0 {
            if let Some(sub) = self
                .rules
                .substitutes
                .iter()
                .find(|s| s.keycode == ev.keycode && env.groups & s.groups != 0)
            {
                self.swallowed.push(ev.keycode);
                return Decision::Substitute(sub.to);
            }
        }

        // 8) Ctrl 的例外组合。此刻 Ctrl 已以 ⌘ 的身份出现，但这些组合要保持原义
        //    或改投 ⌥，所以直接用 raw 改写，不走上面的 ctl→cmd
        if self.ctl_as_cmd && bare(raw, CONTROL) {
            match ev.keycode {
                // ⌃Space 是系统「选择上一个输入法」；换成 ⌘Space 会变成 Spotlight。
                // 物理 Win+Space 经 ⌘→⌃ 换位后也落到 ⌃Space，两条路都能切输入法
                SPACE => return Decision::Keep,
                LEFT | RIGHT | DELETE if self.rules.ctrl_arrows => {
                    return Decision::Rewrite {
                        keycode: ev.keycode,
                        flags: flags::swap(raw, &flags::CTL_TO_ALT),
                    };
                }
                _ => {}
            }
        }

        if flags != raw {
            Decision::Rewrite {
                keycode: ev.keycode,
                flags,
            }
        } else {
            Decision::Keep
        }
    }

    /// Ctrl 的按下/松开（FlagsChanged，键码 59 / 62）。
    fn on_ctrl(&mut self, ev: Input, env: &Env) -> Decision {
        let bit = if ev.keycode == RIGHT_CONTROL {
            flags::R_CTL
        } else {
            flags::L_CTL
        };
        let pressed = ev.flags & bit != 0;
        let was_held = self.ctl_held;
        let was_cmd = self.ctl_as_cmd;

        if pressed {
            self.ctl_held |= bit;
        } else {
            self.ctl_held &= !bit;
        }
        // 自愈：事件 flags 里已看不到任何 Ctrl，却还认为按着——漏掉了松开事件
        if self.ctl_held != 0 && ev.flags & (flags::L_CTL | flags::R_CTL) == 0 {
            self.ctl_held = 0;
        }

        if self.ctl_held == 0 {
            self.ctl_as_cmd = false;
        } else if was_held == 0 {
            // 本轮第一个 Ctrl 按下时定身份；同一轮里再按第二个 Ctrl 沿用。
            // 身份一旦定了就不再随前台应用变，否则按下和松开会不对称。
            self.ctl_as_cmd = self.rules.ctrl_as_cmd
                && !env.has(groups::TERMINAL)
                && !env.has(groups::PASSTHROUGH);
        }

        if was_cmd || self.ctl_as_cmd {
            Decision::Rewrite {
                keycode: if ev.keycode == RIGHT_CONTROL {
                    keys::RIGHT_COMMAND
                } else {
                    keys::COMMAND
                },
                flags: flags::swap(ev.flags, &flags::CTL_TO_CMD),
            }
        } else {
            Decision::Keep
        }
    }

    /// Win(⌘) 的按下/松开（FlagsChanged，键码 55 / 54）：以 ⌃ 的身份出现。
    fn on_cmd(&mut self, ev: Input, env: &Env) -> Decision {
        let bit = if ev.keycode == RIGHT_COMMAND {
            flags::R_CMD
        } else {
            flags::L_CMD
        };
        let pressed = ev.flags & bit != 0;
        let was_held = self.cmd_held;
        let was_ctl = self.cmd_as_ctl;

        if pressed {
            self.cmd_held |= bit;
        } else {
            self.cmd_held &= !bit;
        }
        // 自愈：事件 flags 里已看不到任何 ⌘，却还认为按着——漏掉了松开事件
        if self.cmd_held != 0 && ev.flags & (flags::L_CMD | flags::R_CMD) == 0 {
            self.cmd_held = 0;
        }

        if self.cmd_held == 0 {
            self.cmd_as_ctl = false;
        } else if was_held == 0 {
            // 与 Ctrl 不同，⌘→⌃ 的换位不区分前台应用：终端里的中断键到处都该是它
            self.cmd_as_ctl = self.rules.cmd_as_ctrl && !env.has(groups::PASSTHROUGH);
        }

        if was_ctl || self.cmd_as_ctl {
            Decision::Rewrite {
                keycode: if ev.keycode == RIGHT_COMMAND {
                    keys::RIGHT_CONTROL
                } else {
                    keys::CONTROL
                },
                flags: flags::swap(ev.flags, &flags::CMD_TO_CTL),
            }
        } else {
            Decision::Keep
        }
    }

    /// 左 Alt 的按下/松开（FlagsChanged，键码 58）。
    fn on_left_alt(&mut self, ev: Input) -> Decision {
        let pressed = ev.flags & L_ALT != 0;
        if !self.alt_as_cmd {
            // 普通的 Alt 按下/松开：原样
            return Decision::Keep;
        }
        self.alt_as_cmd = false;
        if pressed {
            // 状态脱节（松开事件被安全输入吃掉后又按下）：当作全新的一次按下
            return Decision::Keep;
        }
        // 应用切换器等的是「⌘ 松开」：把左 Alt 的松开伪装成 ⌘ 松开
        Decision::Rewrite {
            keycode: keys::COMMAND,
            flags: flags::swap(ev.flags, &flags::ALT_TO_CMD),
        }
    }
}

/// flags 里 `mask` 置位且没有别的修饰键
fn bare(flags: u64, mask: u64) -> bool {
    flags & mask != 0 && flags & (ANY_MODIFIER & !mask) == 0
}

fn rewrite_if_changed(ev: Input, flags: u64) -> Decision {
    if flags != ev.flags {
        Decision::Rewrite {
            keycode: ev.keycode,
            flags,
        }
    } else {
        Decision::Keep
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::flags::{L_CMD, L_CTL, R_CMD, R_CTL, SECONDARY_FN};
    use crate::keys::{C, F2, F5, Q, R, RETURN, RIGHT_COMMAND, RIGHT_OPTION};

    const NONE: u64 = 0;
    /// 左 ⌃ 按下后事件自带的 flags
    const CTL_DOWN: u64 = CONTROL | L_CTL;
    /// 左 ⌘ 按下后应有的 flags
    const CMD_L: u64 = COMMAND | L_CMD;
    /// 左 ⌥ 按下后事件自带的 flags
    const ALT_DOWN: u64 = flags::ALTERNATE | L_ALT;
    const SHIFT_DOWN: u64 = flags::SHIFT | flags::L_SHIFT;

    fn plain() -> Env {
        Env { groups: 0 }
    }
    fn term() -> Env {
        Env {
            groups: groups::TERMINAL,
        }
    }
    fn finder() -> Env {
        Env {
            groups: groups::FINDER,
        }
    }
    fn browser() -> Env {
        Env {
            groups: groups::BROWSER,
        }
    }
    fn passthrough() -> Env {
        Env {
            groups: groups::PASSTHROUGH,
        }
    }

    fn with_substitutes() -> Remapper {
        Remapper::new(Rules {
            substitutes: vec![
                Substitute {
                    keycode: F2,
                    groups: groups::FINDER,
                    to: Chord {
                        keycode: RETURN,
                        flags: NONE,
                    },
                },
                Substitute {
                    keycode: F5,
                    groups: groups::BROWSER,
                    to: Chord {
                        keycode: R,
                        flags: COMMAND,
                    },
                },
            ],
            ..Default::default()
        })
    }

    // ---------------- Ctrl 当作 ⌘

    #[test]
    fn ctrl_c_is_cmd_c_and_releases_symmetrically() {
        let mut r = Remapper::new(Rules::default());
        let env = plain();
        // 按下怎么改，松开就必须对称地改，否则系统里会留下一个卡住的 ⌘
        assert_eq!(
            r.on_input(Input::flags_changed(keys::CONTROL, CTL_DOWN), &env),
            Decision::Rewrite {
                keycode: keys::COMMAND,
                flags: CMD_L
            }
        );
        assert_eq!(
            r.on_input(Input::key_down(C, CTL_DOWN), &env),
            Decision::Rewrite {
                keycode: C,
                flags: CMD_L
            }
        );
        assert_eq!(
            r.on_input(Input::key_up(C, CTL_DOWN), &env),
            Decision::Rewrite {
                keycode: C,
                flags: CMD_L
            }
        );
        assert_eq!(
            r.on_input(Input::flags_changed(keys::CONTROL, NONE), &env),
            Decision::Rewrite {
                keycode: keys::COMMAND,
                flags: NONE
            }
        );
        // 松开之后恢复原样
        assert_eq!(r.on_input(Input::key_down(C, NONE), &env), Decision::Keep);
    }

    #[test]
    fn ctrl_shift_combo_keeps_shift() {
        let mut r = Remapper::new(Rules::default());
        let env = plain();
        r.on_input(Input::flags_changed(keys::CONTROL, CTL_DOWN), &env);
        assert_eq!(
            r.on_input(Input::key_down(C, SHIFT_DOWN | CTL_DOWN), &env),
            Decision::Rewrite {
                keycode: C,
                flags: SHIFT_DOWN | CMD_L
            }
        );
    }

    #[test]
    fn right_ctrl_maps_to_right_cmd() {
        let mut r = Remapper::new(Rules::default());
        let env = plain();
        assert_eq!(
            r.on_input(Input::flags_changed(RIGHT_CONTROL, CONTROL | R_CTL), &env),
            Decision::Rewrite {
                keycode: RIGHT_COMMAND,
                flags: COMMAND | R_CMD
            }
        );
        assert_eq!(
            r.on_input(Input::flags_changed(RIGHT_CONTROL, NONE), &env),
            Decision::Rewrite {
                keycode: RIGHT_COMMAND,
                flags: NONE
            }
        );
    }

    #[test]
    fn both_ctrls_stay_consistent() {
        let mut r = Remapper::new(Rules::default());
        let env = plain();
        r.on_input(Input::flags_changed(keys::CONTROL, CTL_DOWN), &env);
        // 再按右 ⌃
        assert_eq!(
            r.on_input(
                Input::flags_changed(RIGHT_CONTROL, CONTROL | L_CTL | R_CTL),
                &env
            ),
            Decision::Rewrite {
                keycode: RIGHT_COMMAND,
                flags: COMMAND | L_CMD | R_CMD
            }
        );
        // 先松左 ⌃：左 ⌘ 位必须清掉，右 ⌘ 位还在
        assert_eq!(
            r.on_input(Input::flags_changed(keys::CONTROL, CONTROL | R_CTL), &env),
            Decision::Rewrite {
                keycode: keys::COMMAND,
                flags: COMMAND | R_CMD
            }
        );
        assert_eq!(
            r.on_input(Input::flags_changed(RIGHT_CONTROL, NONE), &env),
            Decision::Rewrite {
                keycode: RIGHT_COMMAND,
                flags: NONE
            }
        );
        assert_eq!(r.on_input(Input::key_down(C, NONE), &env), Decision::Keep);
    }

    #[test]
    fn plain_keys_and_physical_shortcuts_are_untouched() {
        let mut r = Remapper::new(Rules::default());
        let env = plain();
        assert_eq!(r.on_input(Input::key_down(C, NONE), &env), Decision::Keep);
        assert_eq!(r.on_input(Input::key_up(C, NONE), &env), Decision::Keep);
        // 物理快捷键 ⌥C 原样；⌘ 按下/⌘C 的互换在下面的专门用例里
        assert_eq!(
            r.on_input(Input::key_down(C, ALT_DOWN), &env),
            Decision::Keep
        );
        assert_eq!(
            r.on_input(Input::flags_changed(keys::OPTION, ALT_DOWN), &env),
            Decision::Keep
        );
    }

    #[test]
    fn ctrl_v_is_paste_even_when_history_is_enabled() {
        let mut r = Remapper::new(Rules {
            win_key_clipboard: true,
            ..Default::default()
        });
        let env = plain();
        r.on_input(Input::flags_changed(keys::CONTROL, CTL_DOWN), &env);
        // 被改写成 ⌘ 的 Ctrl+V 是粘贴，不是剪贴板历史
        assert_eq!(
            r.on_input(Input::key_down(V, CTL_DOWN), &env),
            Decision::Rewrite {
                keycode: V,
                flags: CMD_L
            }
        );
    }

    // ---------------- Win(⌘) 当作 ⌃

    #[test]
    fn cmd_becomes_ctrl_symmetrically() {
        let mut r = Remapper::new(Rules::default());
        let env = plain();
        // 物理 ⌘ 按下 → 以 ⌃ 身份出现
        assert_eq!(
            r.on_input(Input::flags_changed(keys::COMMAND, CMD_L), &env),
            Decision::Rewrite {
                keycode: keys::CONTROL,
                flags: CTL_DOWN
            }
        );
        // ⌘C → ⌃C：终端里的中断/控制键
        assert_eq!(
            r.on_input(Input::key_down(C, CMD_L), &env),
            Decision::Rewrite {
                keycode: C,
                flags: CTL_DOWN
            }
        );
        assert_eq!(
            r.on_input(Input::key_up(C, CMD_L), &env),
            Decision::Rewrite {
                keycode: C,
                flags: CTL_DOWN
            }
        );
        assert_eq!(
            r.on_input(Input::flags_changed(keys::COMMAND, NONE), &env),
            Decision::Rewrite {
                keycode: keys::CONTROL,
                flags: NONE
            }
        );
        // 松开之后恢复原样
        assert_eq!(r.on_input(Input::key_down(C, NONE), &env), Decision::Keep);
    }

    #[test]
    fn holding_both_ctrl_and_cmd_cancels_out() {
        let mut r = Remapper::new(Rules::default());
        let env = plain();
        r.on_input(Input::flags_changed(keys::CONTROL, CTL_DOWN), &env);
        r.on_input(Input::flags_changed(keys::COMMAND, CMD_L), &env);
        // 两个都按住：⌃→⌘ 和 ⌘→⌃ 互相抵消，事件原样
        assert_eq!(
            r.on_input(Input::key_down(C, CTL_DOWN | CMD_L), &env),
            Decision::Keep
        );
    }

    #[test]
    fn win_space_swaps_to_ctrl_space_for_ime() {
        let mut r = Remapper::new(Rules::default());
        let env = plain();
        r.on_input(Input::flags_changed(keys::COMMAND, CMD_L), &env);
        assert_eq!(
            r.on_input(Input::key_down(SPACE, CMD_L), &env),
            Decision::Rewrite {
                keycode: SPACE,
                flags: CTL_DOWN
            }
        );
    }

    #[test]
    fn cmd_swap_applies_in_terminals_too() {
        // 终端例外只豁免 Ctrl→⌘ 方向；⌘→⌃ 到处都换，终端里才有中断键
        let mut r = Remapper::new(Rules::default());
        let env = term();
        assert_eq!(
            r.on_input(Input::flags_changed(keys::COMMAND, CMD_L), &env),
            Decision::Rewrite {
                keycode: keys::CONTROL,
                flags: CTL_DOWN
            }
        );
        assert_eq!(
            r.on_input(Input::key_down(C, CMD_L), &env),
            Decision::Rewrite {
                keycode: C,
                flags: CTL_DOWN
            }
        );
    }

    // ---------------- 终端与透传

    #[test]
    fn terminal_keeps_ctrl_meaning() {
        let mut r = Remapper::new(Rules::default());
        let env = term();
        assert_eq!(
            r.on_input(Input::flags_changed(keys::CONTROL, CTL_DOWN), &env),
            Decision::Keep
        );
        assert_eq!(
            r.on_input(Input::key_down(C, CTL_DOWN), &env),
            Decision::Keep
        );
        assert_eq!(r.on_input(Input::key_up(C, CTL_DOWN), &env), Decision::Keep);
        assert_eq!(
            r.on_input(Input::flags_changed(keys::CONTROL, NONE), &env),
            Decision::Keep
        );
    }

    #[test]
    fn ctrl_identity_is_decided_when_pressed() {
        let mut r = Remapper::new(Rules::default());
        // 终端里按下 Ctrl：保持 Ctrl
        assert_eq!(
            r.on_input(Input::flags_changed(keys::CONTROL, CTL_DOWN), &term()),
            Decision::Keep
        );
        // 按住期间用鼠标切到别的应用：这一轮仍是 Ctrl，直到松开
        assert_eq!(
            r.on_input(Input::key_down(C, CTL_DOWN), &plain()),
            Decision::Keep
        );
        assert_eq!(
            r.on_input(Input::flags_changed(keys::CONTROL, NONE), &plain()),
            Decision::Keep
        );
        // 下一次按下时按新的前台应用定身份
        assert_eq!(
            r.on_input(Input::flags_changed(keys::CONTROL, CTL_DOWN), &plain()),
            Decision::Rewrite {
                keycode: keys::COMMAND,
                flags: CMD_L
            }
        );
    }

    #[test]
    fn passthrough_app_changes_nothing() {
        let mut r = with_substitutes();
        let env = passthrough();
        assert_eq!(
            r.on_input(Input::flags_changed(keys::CONTROL, CTL_DOWN), &env),
            Decision::Keep
        );
        assert_eq!(
            r.on_input(Input::key_down(C, CTL_DOWN), &env),
            Decision::Keep
        );
        assert_eq!(
            r.on_input(Input::flags_changed(keys::COMMAND, CMD_L), &env),
            Decision::Keep
        );
        assert_eq!(r.on_input(Input::key_down(C, CMD_L), &env), Decision::Keep);
        assert_eq!(
            r.on_input(Input::flags_changed(keys::OPTION, ALT_DOWN), &env),
            Decision::Keep
        );
        assert_eq!(
            r.on_input(Input::key_down(TAB, ALT_DOWN), &env),
            Decision::Keep
        );
        assert_eq!(r.on_input(Input::key_down(L, CMD_L), &env), Decision::Keep);
        assert_eq!(r.on_input(Input::key_down(F5, NONE), &env), Decision::Keep);
    }

    // ---------------- Ctrl 的例外组合

    #[test]
    fn ctrl_space_stays_ctrl_space() {
        let mut r = Remapper::new(Rules::default());
        let env = plain();
        r.on_input(Input::flags_changed(keys::CONTROL, CTL_DOWN), &env);
        assert_eq!(
            r.on_input(Input::key_down(SPACE, CTL_DOWN), &env),
            Decision::Keep
        );
        assert_eq!(
            r.on_input(Input::key_up(SPACE, CTL_DOWN), &env),
            Decision::Keep
        );
    }

    #[test]
    fn ctrl_arrows_and_backspace_become_option() {
        let mut r = Remapper::new(Rules::default());
        let env = plain();
        r.on_input(Input::flags_changed(keys::CONTROL, CTL_DOWN), &env);
        let opt = flags::ALTERNATE | L_ALT;
        assert_eq!(
            r.on_input(Input::key_down(LEFT, CTL_DOWN), &env),
            Decision::Rewrite {
                keycode: LEFT,
                flags: opt
            }
        );
        assert_eq!(
            r.on_input(Input::key_down(keys::RIGHT, CTL_DOWN), &env),
            Decision::Rewrite {
                keycode: keys::RIGHT,
                flags: opt
            }
        );
        assert_eq!(
            r.on_input(Input::key_down(DELETE, CTL_DOWN), &env),
            Decision::Rewrite {
                keycode: DELETE,
                flags: opt
            }
        );
        // ⌃↑/⌃↓ 跟随互换变成 ⌘↑/⌘↓；Mission Control 用 Win+↑/↓（=⌃↑/⌃↓）
        assert_eq!(
            r.on_input(Input::key_down(keys::UP, CTL_DOWN), &env),
            Decision::Rewrite {
                keycode: keys::UP,
                flags: CMD_L
            }
        );
        assert_eq!(
            r.on_input(Input::key_down(keys::DOWN, CTL_DOWN), &env),
            Decision::Rewrite {
                keycode: keys::DOWN,
                flags: CMD_L
            }
        );
    }

    #[test]
    fn ctrl_arrows_rule_can_be_disabled() {
        let mut r = Remapper::new(Rules {
            ctrl_arrows: false,
            ..Default::default()
        });
        let env = plain();
        r.on_input(Input::flags_changed(keys::CONTROL, CTL_DOWN), &env);
        assert_eq!(
            r.on_input(Input::key_down(LEFT, CTL_DOWN), &env),
            Decision::Rewrite {
                keycode: LEFT,
                flags: CMD_L
            }
        );
    }

    // ---------------- Alt+Tab

    #[test]
    fn alt_tab_turns_option_into_command() {
        let mut r = Remapper::new(Rules::default());
        let env = plain();
        // 单按 Alt 不动
        assert_eq!(
            r.on_input(Input::flags_changed(keys::OPTION, ALT_DOWN), &env),
            Decision::Keep
        );
        // 看到 Tab 才进入切换模式
        assert_eq!(
            r.on_input(Input::key_down(TAB, ALT_DOWN), &env),
            Decision::Rewrite {
                keycode: TAB,
                flags: CMD_L
            }
        );
        assert_eq!(
            r.on_input(Input::key_up(TAB, ALT_DOWN), &env),
            Decision::Rewrite {
                keycode: TAB,
                flags: CMD_L
            }
        );
        // 按住 Alt 再按一次 Tab：继续往后切
        assert_eq!(
            r.on_input(Input::key_down(TAB, ALT_DOWN), &env),
            Decision::Rewrite {
                keycode: TAB,
                flags: CMD_L
            }
        );
        assert_eq!(
            r.on_input(Input::key_up(TAB, ALT_DOWN), &env),
            Decision::Rewrite {
                keycode: TAB,
                flags: CMD_L
            }
        );
        // 左 Alt 松开伪装成 ⌘ 松开
        assert_eq!(
            r.on_input(Input::flags_changed(keys::OPTION, NONE), &env),
            Decision::Rewrite {
                keycode: keys::COMMAND,
                flags: NONE
            }
        );
        assert_eq!(r.on_input(Input::key_down(TAB, NONE), &env), Decision::Keep);
    }

    #[test]
    fn alt_tab_consumes_other_keys_while_switcher_is_open() {
        let mut r = Remapper::new(Rules::default());
        let env = plain();
        r.on_input(Input::flags_changed(keys::OPTION, ALT_DOWN), &env);
        r.on_input(Input::key_down(TAB, ALT_DOWN), &env);
        // 切换器开着时按 Q 会被当成 ⌘Q，必须吞掉
        assert_eq!(
            r.on_input(Input::key_down(Q, ALT_DOWN), &env),
            Decision::Consume
        );
        assert_eq!(
            r.on_input(Input::key_up(Q, ALT_DOWN), &env),
            Decision::Consume
        );
        assert_eq!(
            r.on_input(Input::flags_changed(keys::OPTION, NONE), &env),
            Decision::Rewrite {
                keycode: keys::COMMAND,
                flags: NONE
            }
        );
    }

    #[test]
    fn alt_tab_with_shift_keeps_shift() {
        let mut r = Remapper::new(Rules::default());
        let env = plain();
        r.on_input(Input::flags_changed(keys::OPTION, ALT_DOWN), &env);
        assert_eq!(
            r.on_input(Input::key_down(TAB, SHIFT_DOWN | ALT_DOWN), &env),
            Decision::Rewrite {
                keycode: TAB,
                flags: SHIFT_DOWN | CMD_L
            }
        );
    }

    #[test]
    fn alt_without_tab_is_untouched() {
        let mut r = Remapper::new(Rules::default());
        let env = plain();
        // ⌥a 之类保持 macOS 原义
        assert_eq!(
            r.on_input(Input::flags_changed(keys::OPTION, ALT_DOWN), &env),
            Decision::Keep
        );
        assert_eq!(
            r.on_input(Input::key_down(keys::A, ALT_DOWN), &env),
            Decision::Keep
        );
        assert_eq!(
            r.on_input(Input::flags_changed(keys::OPTION, NONE), &env),
            Decision::Keep
        );
    }

    #[test]
    fn right_alt_tab_is_untouched() {
        let mut r = Remapper::new(Rules::default());
        let env = plain();
        let right = flags::ALTERNATE | flags::R_ALT;
        assert_eq!(
            r.on_input(Input::flags_changed(RIGHT_OPTION, right), &env),
            Decision::Keep
        );
        assert_eq!(
            r.on_input(Input::key_down(TAB, right), &env),
            Decision::Keep
        );
        assert_eq!(
            r.on_input(Input::flags_changed(RIGHT_OPTION, NONE), &env),
            Decision::Keep
        );
    }

    #[test]
    fn alt_tab_rule_can_be_disabled() {
        let mut r = Remapper::new(Rules {
            alt_tab: false,
            ..Default::default()
        });
        let env = plain();
        r.on_input(Input::flags_changed(keys::OPTION, ALT_DOWN), &env);
        assert_eq!(
            r.on_input(Input::key_down(TAB, ALT_DOWN), &env),
            Decision::Keep
        );
        assert_eq!(
            r.on_input(Input::flags_changed(keys::OPTION, NONE), &env),
            Decision::Keep
        );
    }

    // ---------------- Win 键

    #[test]
    fn win_l_locks() {
        let mut r = Remapper::new(Rules::default());
        let env = plain();
        assert_eq!(
            r.on_input(Input::key_down(L, CMD_L), &env),
            Decision::Action(Action::LockScreen)
        );
        assert_eq!(r.on_input(Input::key_up(L, CMD_L), &env), Decision::Consume);
    }

    #[test]
    fn win_l_requires_bare_cmd() {
        let mut r = Remapper::new(Rules::default());
        let env = plain();
        assert_eq!(
            r.on_input(Input::key_down(L, SHIFT_DOWN | CMD_L), &env),
            Decision::Keep
        );
        assert_eq!(
            r.on_input(Input::key_down(L, ALT_DOWN | CMD_L), &env),
            Decision::Keep
        );
        // Ctrl+⌘+L：Ctrl 被当作 ⌘，结果是普通的 ⌘L
        r.on_input(Input::flags_changed(keys::CONTROL, CTL_DOWN), &env);
        assert_eq!(
            r.on_input(Input::key_down(L, CTL_DOWN | CMD_L), &env),
            Decision::Rewrite {
                keycode: L,
                flags: CMD_L
            }
        );
    }

    #[test]
    fn win_l_can_be_disabled() {
        let mut r = Remapper::new(Rules {
            win_key_lock: false,
            ..Default::default()
        });
        assert_eq!(
            r.on_input(Input::key_down(L, CMD_L), &plain()),
            Decision::Keep
        );
    }

    /// 回归：match 分支里的键码常量如果没导入，会被当成「任意键」的绑定，
    /// 结果是 ⌘+任意键都触发锁屏。所以每个 Win 键动作都要有不相关键的用例。
    #[test]
    fn other_keys_with_bare_cmd_are_not_win_actions() {
        let mut r = Remapper::new(Rules {
            win_key_clipboard: true,
            ..Default::default()
        });
        let env = plain();
        // ⌘Q、⌘W、⌘R、⌘C 都是普通的 macOS 快捷键
        for name in ["q", "w", "e", "r", "t", "a", "s", "c", "x", "z"] {
            let keycode = keys::named(name).unwrap();
            assert_eq!(
                r.on_input(Input::key_down(keycode, CMD_L), &env),
                Decision::Keep,
                "键 {name} 不该被当成 Win 键动作"
            );
        }
        // ⌘L 是锁屏，⌘V 是剪贴板历史（这条用例里显式打开了）
        assert_eq!(
            r.on_input(Input::key_down(L, CMD_L), &env),
            Decision::Action(Action::LockScreen)
        );
        assert_eq!(
            r.on_input(Input::key_down(V, CMD_L), &env),
            Decision::Action(Action::ClipboardHistory)
        );
    }

    /// 任务管理器：物理 Ctrl+Alt+Del(退格) 与物理 Ctrl+Esc 都呼出强制退出窗口
    #[test]
    fn ctrl_alt_del_and_ctrl_esc_open_task_manager() {
        let env = plain();
        let mut r = Remapper::new(Rules::default());
        assert_eq!(
            r.on_input(Input::key_down(DELETE, CTL_DOWN | ALT_DOWN), &env),
            Decision::Action(Action::TaskManager)
        );
        let mut r = Remapper::new(Rules::default());
        assert_eq!(
            r.on_input(Input::key_down(ESCAPE, CTL_DOWN), &env),
            Decision::Action(Action::TaskManager)
        );
    }

    #[test]
    fn task_manager_requires_exact_modifiers() {
        let env = plain();
        // flags 改写跟的是修饰键按下状态，每个用例先发 FlagsChanged 再发按键。
        // 物理 Win+Esc：不是任务管理器，走双向互换（→ ⌃Esc）
        let mut r = Remapper::new(Rules::default());
        r.on_input(Input::flags_changed(keys::COMMAND, CMD_L), &env);
        let got = r.on_input(Input::key_down(ESCAPE, CMD_L), &env);
        assert!(
            matches!(got, Decision::Rewrite { .. }),
            "物理 Win+Esc 应走改写，实际 {got:?}"
        );
        // 多了 Shift 也不触发（→ ⌘⇧Esc）
        let mut r = Remapper::new(Rules::default());
        r.on_input(Input::flags_changed(keys::CONTROL, CTL_DOWN), &env);
        let got = r.on_input(Input::key_down(ESCAPE, CTL_DOWN | SHIFT_DOWN), &env);
        assert!(
            matches!(got, Decision::Rewrite { .. }),
            "Ctrl+Shift+Esc 应走改写，实际 {got:?}"
        );
        // 关掉开关后恢复原样（物理 Ctrl+Esc → ⌘Esc）
        let mut r = Remapper::new(Rules {
            task_manager: false,
            ..Default::default()
        });
        r.on_input(Input::flags_changed(keys::CONTROL, CTL_DOWN), &env);
        let got = r.on_input(Input::key_down(ESCAPE, CTL_DOWN), &env);
        assert!(
            matches!(got, Decision::Rewrite { .. }),
            "开关关时 Ctrl+Esc 应走改写，实际 {got:?}"
        );
    }

    #[test]
    fn win_v_is_history_by_default_and_paste_when_disabled() {
        // 默认开：裸 ⌘V（物理 Win+V）弹剪贴板历史
        let mut r = Remapper::new(Rules::default());
        assert_eq!(
            r.on_input(Input::key_down(V, CMD_L), &plain()),
            Decision::Action(Action::ClipboardHistory)
        );

        // 关掉开关：恢复原样，⌘V 就是粘贴
        let mut r = Remapper::new(Rules {
            win_key_clipboard: false,
            ..Default::default()
        });
        assert_eq!(
            r.on_input(Input::key_down(V, CMD_L), &plain()),
            Decision::Keep
        );
    }

    #[test]
    fn held_win_key_does_not_retrigger() {
        let mut r = Remapper::new(Rules::default());
        let env = plain();
        assert_eq!(
            r.on_input(Input::key_down(L, CMD_L), &env),
            Decision::Action(Action::LockScreen)
        );
        // 按住不放的重复事件只吞，不再触发
        assert_eq!(
            r.on_input(Input::key_down(L, CMD_L).repeated(), &env),
            Decision::Consume
        );
    }

    // ---------------- 单键替换

    #[test]
    fn f2_is_return_only_in_finder() {
        let mut r = with_substitutes();
        assert_eq!(
            r.on_input(Input::key_down(F2, NONE), &finder()),
            Decision::Substitute(Chord {
                keycode: RETURN,
                flags: NONE
            })
        );
        assert_eq!(
            r.on_input(Input::key_up(F2, NONE), &finder()),
            Decision::Consume
        );
        // 其他应用里的 F2 保持原义（Excel 编辑、VS Code 重命名）
        assert_eq!(
            r.on_input(Input::key_down(F2, NONE), &plain()),
            Decision::Keep
        );
    }

    #[test]
    fn f5_is_cmd_r_only_in_browsers() {
        let mut r = with_substitutes();
        assert_eq!(
            r.on_input(Input::key_down(F5, NONE), &browser()),
            Decision::Substitute(Chord {
                keycode: R,
                flags: COMMAND
            })
        );
        assert_eq!(
            r.on_input(Input::key_up(F5, NONE), &browser()),
            Decision::Consume
        );
        // 编辑器里的 F5 是启动调试，不动
        assert_eq!(
            r.on_input(Input::key_down(F5, NONE), &plain()),
            Decision::Keep
        );
    }

    #[test]
    fn substitute_needs_bare_key() {
        let mut r = with_substitutes();
        assert_eq!(
            r.on_input(Input::key_down(F5, SHIFT_DOWN), &browser()),
            Decision::Keep
        );
        assert_eq!(
            r.on_input(Input::key_down(F2, CMD_L), &finder()),
            Decision::Keep
        );
    }

    #[test]
    fn substitute_ignores_fn_flag() {
        // 笔记本上按 F5 往往要按着 fn；fn 不算修饰键
        let mut r = with_substitutes();
        assert_eq!(
            r.on_input(Input::key_down(F5, SECONDARY_FN), &browser()),
            Decision::Substitute(Chord {
                keycode: R,
                flags: COMMAND
            })
        );
        assert_eq!(
            r.on_input(Input::key_up(F5, SECONDARY_FN), &browser()),
            Decision::Consume
        );
    }

    #[test]
    fn held_substitute_key_does_not_retrigger() {
        let mut r = with_substitutes();
        let env = browser();
        assert_eq!(
            r.on_input(Input::key_down(F5, NONE), &env),
            Decision::Substitute(Chord {
                keycode: R,
                flags: COMMAND
            })
        );
        assert_eq!(
            r.on_input(Input::key_down(F5, NONE).repeated(), &env),
            Decision::Consume
        );
        assert_eq!(r.on_input(Input::key_up(F5, NONE), &env), Decision::Consume);
    }

    // ---------------- 漏事件后的自愈

    #[test]
    fn missed_key_up_does_not_eat_next_press() {
        let mut r = Remapper::new(Rules::default());
        let env = plain();
        r.on_input(Input::key_down(L, CMD_L), &env);
        // KeyUp 没送到（安全输入期间松开）：下一个普通的 L 必须照常放行
        assert_eq!(r.on_input(Input::key_down(L, NONE), &env), Decision::Keep);
        assert_eq!(r.on_input(Input::key_up(L, NONE), &env), Decision::Keep);
    }

    #[test]
    fn missed_ctrl_release_keeps_press_and_release_symmetric() {
        let mut r = Remapper::new(Rules::default());
        let env = plain();
        r.on_input(Input::flags_changed(keys::CONTROL, CTL_DOWN), &env);
        // 松开事件丢失，直接来了新的一次按下：仍以 ⌘ 身份出现，不重复计数
        assert_eq!(
            r.on_input(Input::flags_changed(keys::CONTROL, CTL_DOWN), &env),
            Decision::Rewrite {
                keycode: keys::COMMAND,
                flags: CMD_L
            }
        );
        assert_eq!(
            r.on_input(Input::flags_changed(keys::CONTROL, NONE), &env),
            Decision::Rewrite {
                keycode: keys::COMMAND,
                flags: NONE
            }
        );
    }

    #[test]
    fn stale_ctrl_state_does_not_touch_unflagged_keys() {
        let mut r = Remapper::new(Rules::default());
        let env = plain();
        r.on_input(Input::flags_changed(keys::CONTROL, CTL_DOWN), &env);
        // 以为 Ctrl 还按着，但事件自己不带 Ctrl 位：原样放行
        assert_eq!(r.on_input(Input::key_down(C, NONE), &env), Decision::Keep);
    }

    #[test]
    fn stale_alt_state_self_heals() {
        let mut r = Remapper::new(Rules::default());
        let env = plain();
        r.on_input(Input::flags_changed(keys::OPTION, ALT_DOWN), &env);
        r.on_input(Input::key_down(TAB, ALT_DOWN), &env);
        // 松开事件丢失后来了一个不带左 Alt 的普通按键
        assert_eq!(r.on_input(Input::key_down(C, NONE), &env), Decision::Keep);
        // 再按 Alt 是全新的一次按下
        assert_eq!(
            r.on_input(Input::flags_changed(keys::OPTION, ALT_DOWN), &env),
            Decision::Keep
        );
    }

    #[test]
    fn reset_clears_everything() {
        let mut r = Remapper::new(Rules::default());
        let env = plain();
        r.on_input(Input::flags_changed(keys::CONTROL, CTL_DOWN), &env);
        r.on_input(Input::flags_changed(keys::COMMAND, CMD_L), &env);
        r.on_input(Input::key_down(L, CMD_L), &env);
        r.reset();
        assert_eq!(
            r.on_input(Input::key_down(C, CTL_DOWN), &env),
            Decision::Keep
        );
        assert_eq!(r.on_input(Input::key_down(C, CMD_L), &env), Decision::Keep);
        assert_eq!(r.on_input(Input::key_up(L, CMD_L), &env), Decision::Keep);
    }

    #[test]
    fn swallowed_list_is_bounded() {
        let mut s = Swallowed::default();
        for k in 1..=6 {
            s.push(k);
        }
        // 容量 4：最旧的两个被挤掉
        assert!(!s.has(1) && !s.has(2));
        assert!(s.has(3) && s.has(4) && s.has(5) && s.has(6));
        assert!(s.take(4));
        assert!(!s.has(4) && s.has(3) && s.has(5));
        assert!(!s.take(99));
        s.push(5);
        assert_eq!(s.len, 3, "重复 push 不占位");
    }
}
