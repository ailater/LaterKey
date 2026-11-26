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

/// macOS 系统字体（SF NS）：只作**符号兜底**（⌘ ⌃ ⇧ ⌥ 等 CJK 字体没有的
/// 字符），不渲染正文拉丁。曾让 SF 渲染拉丁追求「原生观感」，但 egui 对每
/// 字体 ascent 取整的错位随字号变化（14pt 与 11.5pt 错位方向都不同），
/// FontTweak 单一补偿值无法全字号对齐——中英文混排必须同一字体渲染。
const SYMBOL_FONT: &str = "/System/Library/Fonts/SFNS.ttf";

/// 键帽字体族：SF 独占（符号 ⌘⌃⇧⌥ 与字母数字同一字体），给速览浮窗的
/// 键名用。与正文分开排版——egui 0.35 混字体行内逐字形按各自字体度量取整
/// 定位，SF 与 CJK 字体混排会高低不齐（2026-11-26 用户反馈），所以键名
/// 里的符号/字母段与中文段要拆开各画各的（见 cheatsheet 的 draw_key_text）。
pub const KEYCAPS_FAMILY: &str = "laterkey-keycaps";

/// 键帽字体。系统 SF 缺席时退回正文族（符号会缺字形，但不崩）。
pub fn keycaps_font(size: f32) -> egui::FontId {
    egui::FontId::new(size, egui::FontFamily::Name(KEYCAPS_FAMILY.into()))
}

/// 把系统字体挂成 egui 的字体族（内置字体留作缺字 fallback）。
/// 顺序：CJK（中文与拉丁正文同一字体，基线天然一致）→ SF（仅符号）→ 内置兜底。
/// 找不到任何字体就不动——退回 egui 默认字体（中文会显示为方框）。
pub fn setup(ctx: &egui::Context) {
    let symbols = std::fs::read(SYMBOL_FONT).ok();
    let cjk = CANDIDATES.iter().find_map(|p| std::fs::read(p).ok());
    if symbols.is_none() && cjk.is_none() {
        // 没有系统字体可用：键帽族也要绑（egui 对未注册的族直接 panic），
        // 绑内置拉丁兜底——符号会缺字形，但好歹不崩
        let mut fonts = egui::FontDefinitions::default();
        fonts.families.insert(
            egui::FontFamily::Name(KEYCAPS_FAMILY.into()),
            vec!["Ubuntu-Light".into()],
        );
        ctx.set_fonts(fonts);
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
        *list = ["system-cjk", "system-symbols"]
            .iter()
            .filter(|name| fonts.font_data.contains_key(**name))
            .map(|name| (*name).to_string())
            .chain(list.drain(..))
            .collect();
    }
    // 键帽族：只有 SF + 内置（拉丁/数字与键帽符号同字体，绝无 CJK 混入；
    // CJK 键名段由调用方拆出来用正文族画）
    fonts.families.insert(
        egui::FontFamily::Name(KEYCAPS_FAMILY.into()),
        ["system-symbols", "Ubuntu-Light"]
            .iter()
            .filter(|name| fonts.font_data.contains_key(**name))
            .map(|name| (*name).to_string())
            .collect(),
    );
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
