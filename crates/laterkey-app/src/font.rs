//! 设置面板的中文字体：运行时读系统字体（苹方等），不捆绑字体文件
//! （CJK 字体动辄 10MB+）。方案同 LaterScreen 的 font.rs。

use eframe::egui;

/// macOS 上支持中文的候选字体。PingFang.ttc 是 LaterScreen 生产验证过的。
const CANDIDATES: &[&str] = &[
    "/System/Library/Fonts/PingFang.ttc",
    "/System/Library/Fonts/Hiragino Sans GB.ttc",
    "/System/Library/Fonts/STHeiti Light.ttc",
    "/System/Library/Fonts/Supplemental/Songti.ttc",
];

/// macOS 系统字体（SF NS）：拉丁与苹果符号（⌘ ⌃ ⇧ ⌥ ⌫）都齐全，字形与
/// 系统一致。排在苹方之前——苹方虽含基本拉丁，但用 SF 渲染更精致，且
/// 「SF + 苹方」本来就是 macOS 原生组合，混排基线协调。
const SYMBOL_FONT: &str = "/System/Library/Fonts/SFNS.ttf";

/// 把系统字体挂成 egui 的字体族（内置字体留作缺字 fallback）。
/// 顺序：SF（拉丁/苹果符号）→ 苹方（中文）→ 内置兜底。
/// 找不到任何字体就不动——退回 egui 默认字体（中文会显示为方框）。
pub fn setup(ctx: &egui::Context) {
    let symbols = std::fs::read(SYMBOL_FONT).ok();
    let cjk = CANDIDATES.iter().find_map(|p| std::fs::read(p).ok());
    if symbols.is_none() && cjk.is_none() {
        return;
    }
    let mut fonts = egui::FontDefinitions::default();
    if let Some(bytes) = symbols {
        fonts.font_data.insert(
            "system-symbols".into(),
            egui::FontData::from_owned(bytes).into(),
        );
    }
    if let Some(bytes) = cjk {
        fonts.font_data.insert(
            "system-cjk".into(),
            egui::FontData::from_owned(bytes).into(),
        );
    }
    for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
        let list = fonts.families.entry(family).or_default();
        // 插入顺序决定 fallback 次序：先 SF 后苹方（push 到尾部，位于
        // 内置字体之前还要再调——直接重建整个列表最直白）
        *list = ["system-symbols", "system-cjk"]
            .iter()
            .filter(|name| fonts.font_data.contains_key(**name))
            .map(|name| (*name).to_string())
            .chain(list.drain(..))
            .collect();
    }
    ctx.set_fonts(fonts);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn at_least_one_cjk_font_exists_on_this_mac() {
        let found = CANDIDATES.iter().any(|p| std::path::Path::new(p).exists());
        assert!(found, "本机应至少有一个候选中文字体");
    }
}
