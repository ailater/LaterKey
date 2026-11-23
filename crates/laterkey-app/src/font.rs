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

/// 把系统中文字体挂成 egui 的首选字体族（内置字体留作缺字 fallback）。
/// 找不到字体就不动——退回 egui 默认字体（中文会显示为方框）。
pub fn setup(ctx: &egui::Context) {
    let Some(bytes) = CANDIDATES.iter().find_map(|p| std::fs::read(p).ok()) else {
        return;
    };
    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert(
        "system-cjk".into(),
        egui::FontData::from_owned(bytes).into(),
    );
    for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
        fonts
            .families
            .entry(family)
            .or_default()
            .insert(0, "system-cjk".into());
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
