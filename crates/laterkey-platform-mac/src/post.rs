//! 合成按键：单键替换（F5 → ⌘R）与锁屏兜底用。
//!
//! 事件打上 OWN_TAG 再投递到 HID 层：这样它们和真实按键走同一条路（系统快捷键
//! 也认），而拦截回调看到标记就原样放行，不会再进一次状态机。

use std::sync::OnceLock;

use laterkey_core::Chord;

use super::ffi::*;

/// "LKEY"
pub const OWN_TAG: i64 = 0x4C4B_4559;

static SOURCE: OnceLock<usize> = OnceLock::new();

fn source() -> CGEventSourceRef {
    *SOURCE.get_or_init(|| {
        // SAFETY: 只是创建事件源
        unsafe {
            let src = CGEventSourceCreate(SOURCE_STATE_HID);
            if !src.is_null() {
                // 默认 0.25s：投递后本地硬件事件会被延迟，对紧跟其后的真实按键有影响
                CGEventSourceSetLocalEventsSuppressionInterval(src, 0.0);
            }
            src as usize
        }
    }) as CGEventSourceRef
}

/// 投递一组合成按键（按下 + 松开）。返回是否投递成功。
pub fn chord(chord: Chord) -> bool {
    // SAFETY: 事件源进程存活期内有效；创建出的事件由本函数释放
    unsafe {
        let down = CGEventCreateKeyboardEvent(source(), chord.keycode as u16, true);
        let up = CGEventCreateKeyboardEvent(source(), chord.keycode as u16, false);
        if down.is_null() || up.is_null() {
            if !down.is_null() {
                CFRelease(down);
            }
            if !up.is_null() {
                CFRelease(up);
            }
            return false;
        }
        for ev in [down, up] {
            CGEventSetFlags(ev, chord.flags);
            CGEventSetIntegerValueField(ev, FIELD_USER_DATA, OWN_TAG);
        }
        CGEventPost(TAP_HID, down);
        CGEventPost(TAP_HID, up);
        CFRelease(down);
        CFRelease(up);
        true
    }
}

/// 系统当前记录的修饰键状态（HID 层）。自测用。
#[cfg(test)]
pub fn hid_flags_state() -> u64 {
    unsafe { CGEventSourceFlagsState(SOURCE_STATE_HID) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ptr::null_mut;

    /// 只构造、不投递：验证合成事件的键码与标志位读出来是写入的值。
    #[test]
    fn synthetic_event_carries_what_we_set() {
        // SAFETY: 不投递，仅在本进程内读写
        unsafe {
            let ev = CGEventCreateKeyboardEvent(null_mut(), 0x0F, true);
            assert!(!ev.is_null());
            CGEventSetFlags(ev, laterkey_core::flags::COMMAND);
            CGEventSetIntegerValueField(ev, FIELD_USER_DATA, OWN_TAG);
            assert_eq!(CGEventGetIntegerValueField(ev, FIELD_KEYCODE), 0x0F);
            assert_eq!(
                CGEventGetFlags(ev) & laterkey_core::flags::ANY_MODIFIER,
                laterkey_core::flags::COMMAND
            );
            assert_eq!(CGEventGetIntegerValueField(ev, FIELD_USER_DATA), OWN_TAG);
            CFRelease(ev);
        }
    }
}
