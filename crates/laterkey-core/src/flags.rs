//! CGEventFlags 位定义。高 16 位是「类别」掩码，低 16 位标出「具体是哪个物理键」。
//!
//! 数值对照本机 SDK：类别位见 `CoreGraphics/CGEventTypes.h`，设备位见
//! `IOKit/hidsystem/IOLLEvent.h`（NX_DEVICE*）。

pub const SHIFT: u64 = 0x2_0000;
pub const CONTROL: u64 = 0x4_0000;
pub const ALTERNATE: u64 = 0x8_0000;
pub const COMMAND: u64 = 0x10_0000;
/// fn / 地球键
pub const SECONDARY_FN: u64 = 0x80_0000;

// 设备相关位（低 16 位）。左右键各自一位，换位时必须逐位对应，不能「任一位置位
// 就把左右两位都置上」——否则松开左键时 flags 里左键位还亮着，前后矛盾。
pub const L_SHIFT: u64 = 0x2;
pub const R_SHIFT: u64 = 0x4;
pub const L_CMD: u64 = 0x8;
pub const R_CMD: u64 = 0x10;
pub const L_ALT: u64 = 0x20;
pub const R_ALT: u64 = 0x40;
pub const L_CTL: u64 = 0x1;
pub const R_CTL: u64 = 0x2000;

/// 四个类别位的合集：用来判断「有没有按修饰键」
pub const ANY_MODIFIER: u64 = SHIFT | CONTROL | ALTERNATE | COMMAND;

/// 一组成对的换位：flags 里 from 位置位就换成 to 位，否则清掉 to 位。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pair {
    pub from: u64,
    pub to: u64,
}

/// 左右键成对 + 类别位成对，共三对
const fn pairs(
    l_from: u64,
    l_to: u64,
    r_from: u64,
    r_to: u64,
    mask_from: u64,
    mask_to: u64,
) -> [Pair; 3] {
    [
        Pair {
            from: l_from,
            to: l_to,
        },
        Pair {
            from: r_from,
            to: r_to,
        },
        Pair {
            from: mask_from,
            to: mask_to,
        },
    ]
}

pub const CTL_TO_CMD: [Pair; 3] = pairs(L_CTL, L_CMD, R_CTL, R_CMD, CONTROL, COMMAND);
pub const CTL_TO_ALT: [Pair; 3] = pairs(L_CTL, L_ALT, R_CTL, R_ALT, CONTROL, ALTERNATE);
pub const ALT_TO_CMD: [Pair; 3] = pairs(L_ALT, L_CMD, R_ALT, R_CMD, ALTERNATE, COMMAND);
pub const CMD_TO_CTL: [Pair; 3] = pairs(L_CMD, L_CTL, R_CMD, R_CTL, COMMAND, CONTROL);

/// ⌃↔⌘ 双向互换（Ctrl 和 Win 同时按住时用）。
///
/// 不能把 [`CTL_TO_CMD`] 和 [`CMD_TO_CTL`] 串联着用：第一遍换出来的位会被
/// 第二遍当成输入吃掉（硬件本来就按着的 ⌘ 位因此丢失）。这里按原始位一次
/// 算完：⌃ 位→⌘ 位、⌘ 位→⌃ 位，两边同时按住时净效果为零。
pub fn swap_both_ways(flags: u64) -> u64 {
    let mut out = flags & !(L_CTL | R_CTL | L_CMD | R_CMD | CONTROL | COMMAND);
    if flags & L_CTL != 0 {
        out |= L_CMD;
    }
    if flags & R_CTL != 0 {
        out |= R_CMD;
    }
    if flags & CONTROL != 0 {
        out |= COMMAND;
    }
    if flags & L_CMD != 0 {
        out |= L_CTL;
    }
    if flags & R_CMD != 0 {
        out |= R_CTL;
    }
    if flags & COMMAND != 0 {
        out |= CONTROL;
    }
    out
}

/// 按 `pairs` 把 flags 里的一组位换成另一组。
///
/// 前提是同一张表里 from 的位与 to 的位互不重叠（现有三张表都满足）。
pub fn swap(flags: u64, pairs: &[Pair]) -> u64 {
    let mut out = flags;
    for p in pairs {
        out = (out & !(p.from | p.to)) | if flags & p.from != 0 { p.to } else { 0 };
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn swap_maps_each_bit_pair() {
        // 左 ⌃ 按下后的原始 flags → 左 ⌘
        assert_eq!(swap(CONTROL | L_CTL, &CTL_TO_CMD), COMMAND | L_CMD);
        // 右 ⌃ → 右 ⌘，左右不混
        assert_eq!(swap(CONTROL | R_CTL, &CTL_TO_CMD), COMMAND | R_CMD);
        assert_eq!(
            swap(CONTROL | L_CTL | R_CTL, &CTL_TO_CMD),
            COMMAND | L_CMD | R_CMD
        );
    }

    #[test]
    fn swap_leaves_unrelated_bits_alone() {
        let flags = 0x100 | SHIFT | L_SHIFT | SECONDARY_FN;
        assert_eq!(swap(flags, &CTL_TO_CMD), flags);
    }

    #[test]
    fn swap_clears_target_bits_when_source_absent() {
        // 松开事件：from 全灭，to 也必须全灭
        assert_eq!(swap(COMMAND | L_CMD, &CTL_TO_CMD), 0);
        assert_eq!(swap(0, &ALT_TO_CMD), 0);
    }

    #[test]
    fn swap_ctrl_to_alt_keeps_shift() {
        let flags = SHIFT | L_SHIFT | CONTROL | L_CTL;
        assert_eq!(
            swap(flags, &CTL_TO_ALT),
            SHIFT | L_SHIFT | ALTERNATE | L_ALT
        );
    }

    #[test]
    fn swap_both_ways_cancels_when_both_held() {
        let raw = CONTROL | L_CTL | COMMAND | L_CMD;
        assert_eq!(swap_both_ways(raw), raw);
        assert_eq!(swap_both_ways(0), 0);
        // 只按一边时等价于单向换位
        assert_eq!(swap_both_ways(CONTROL | L_CTL), COMMAND | L_CMD);
        assert_eq!(swap_both_ways(COMMAND | L_CMD), CONTROL | L_CTL);
        // 其他位不动
        assert_eq!(
            swap_both_ways(SHIFT | L_SHIFT | SECONDARY_FN),
            SHIFT | L_SHIFT | SECONDARY_FN
        );
        // 混合：⌃⌘⇧ 同时按住
        assert_eq!(
            swap_both_ways(SHIFT | L_SHIFT | CONTROL | L_CTL | COMMAND | L_CMD),
            SHIFT | L_SHIFT | COMMAND | L_CMD | CONTROL | L_CTL
        );
    }

    #[test]
    fn constants_match_sdk_headers() {
        // 钉住关键数值：这些常量一旦与 SDK 不一致，整个状态机都会静默出错
        assert_eq!(CONTROL, 0x4_0000);
        assert_eq!(COMMAND, 0x10_0000);
        assert_eq!(ALTERNATE, 0x8_0000);
        assert_eq!(R_CTL, 0x2000);
        assert_eq!(L_CMD, 0x8);
        assert_eq!(R_CMD, 0x10);
        assert_eq!(L_ALT, 0x20);
        assert_eq!(R_ALT, 0x40);
        // 左 Shift 是 0x2 而不是 0x1：0x1 是左 Ctrl。两者写混不会报错，
        // 只会让 ⌃⇧ 组合悄悄出错，所以逐个钉死。
        assert_eq!(L_CTL, 0x1);
        assert_eq!(L_SHIFT, 0x2);
        assert_eq!(R_SHIFT, 0x4);
        assert_ne!(L_SHIFT, L_CTL);
    }

    #[test]
    fn device_bits_do_not_overlap() {
        let bits = [L_SHIFT, R_SHIFT, L_CMD, R_CMD, L_ALT, R_ALT, L_CTL, R_CTL];
        for (i, a) in bits.iter().enumerate() {
            for b in &bits[..i] {
                assert_eq!(a & b, 0, "设备位 {a:#x} 与 {b:#x} 重叠");
            }
        }
    }
}
