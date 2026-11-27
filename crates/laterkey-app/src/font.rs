//! 面板字体：运行时读系统字体，不捆绑字体文件（CJK 字体动辄十 MB 以上）。
//!
//! 两处界面的字体要求不同，各用各的字体族：
//! - **设置面板**（Proportional / Monospace）：中文与拉丁正文由同一款 CJK
//!   字体渲染，SF 只作符号兜底。曾让 SF 渲染拉丁追求「原生观感」，但 egui
//!   0.35 对每个字体 ascent `round_ui()` 取整后定位基线，SF 与冬青黑的取整
//!   错位**随字号变化**，FontTweak 任何单一补偿值都无法全字号对齐，两轮均被
//!   用户打回——要基线齐，就别把两种字体排在同一个排版单元里；
//! - **快捷键速览**（按系统菜单的样子排）：功能名与标题用系统苹方-简（主标题
//!   用中黑），键名列用 SF，␣⇥↩ 这类 SF 没有的键帽字形回退到 Apple Symbols
//!   （系统菜单也是这么回退的，逐个码位核对过）。键名列的修饰键与主键是两个
//!   独立的排版单元，同样不混字体。
//!
//! 苹方不随系统字体目录分发，而是放在字体资源里、且是 24 个字面共享字形数据
//! 的 TTC（整文件 70 MB+）。egui 的每个 FontData 独占一份字节，直接塞 TTC 会把
//! 这 70 MB 全部常驻内存，所以运行时只把要用的两个字面拼成独立字体（见 [`ttc`]），
//! 不落盘、不进仓库。

use std::path::PathBuf;

use eframe::egui;

/// macOS 上支持中文的候选字体（设置面板与速览的兜底）。
const CANDIDATES: &[&str] = &[
    "/System/Library/Fonts/PingFang.ttc",
    "/System/Library/Fonts/Hiragino Sans GB.ttc",
    "/System/Library/Fonts/STHeiti Light.ttc",
    "/System/Library/Fonts/Supplemental/Songti.ttc",
];

/// 系统字体 SF：速览键名列的主字体，设置面板只拿它兜底符号。
const SYMBOL_FONT: &str = "/System/Library/Fonts/SFNS.ttf";

/// SF 没有 ␣(U+2423) ⇥(U+21E5) ↩(U+21A9) ⇞ ⇟，系统菜单从这份字体回退。
const KEYCAP_FALLBACK_FONT: &str = "/System/Library/Fonts/Apple Symbols.ttf";

/// 苹方所在字体资源的根目录；子目录名带序号，再下一层是哈希，只能按通配找。
const FONT_ASSET_ROOT: &str = "/System/Library/AssetsV2";
const FONT_ASSET_PREFIX: &str = "com_apple_MobileAsset_Font";
const PINGFANG_FILE: &str = "PingFang.ttc";
const PINGFANG_REGULAR: &str = "PingFangSC-Regular";
const PINGFANG_SEMIBOLD: &str = "PingFangSC-Semibold";

/// 速览键名列的字体族：SF 独占（字母数字与 ⌘⌃⇧⌥ 同一字体），缺字形回退
/// Apple Symbols。中文键名段不在这里排，由调用方拆出来用 [`SHEET_FAMILY`]。
pub const KEYCAPS_FAMILY: &str = "laterkey-keycaps";
/// 速览功能名与分组标题的字体族：苹方-简常规，中文与拉丁同一字体。
pub const SHEET_FAMILY: &str = "laterkey-sheet";
/// 速览主标题（应用名、「系统」）的字体族：苹方-简中黑。
pub const SHEET_BOLD_FAMILY: &str = "laterkey-sheet-bold";

pub fn keycaps_font(size: f32) -> egui::FontId {
    egui::FontId::new(size, egui::FontFamily::Name(KEYCAPS_FAMILY.into()))
}

pub fn sheet_font(size: f32) -> egui::FontId {
    egui::FontId::new(size, egui::FontFamily::Name(SHEET_FAMILY.into()))
}

pub fn sheet_bold_font(size: f32) -> egui::FontId {
    egui::FontId::new(size, egui::FontFamily::Name(SHEET_BOLD_FAMILY.into()))
}

/// 从系统 TTC 里取出的苹方-简两个字面，已拼成独立字体字节。
pub struct SheetFonts {
    pub regular: Vec<u8>,
    pub semibold: Vec<u8>,
}

/// 启动时装配字体。找不到任何系统字体就只剩 egui 内置字体（中文会是方框），
/// 但所有自定义字体族仍然绑好——egui 对没绑字体的族直接 panic。
pub fn setup(ctx: &egui::Context) {
    ctx.set_fonts(definitions(None));
}

/// 把速览正文字体换成系统苹方-简。首次打开速览时调用一次。
///
/// 找不到字体资源、或文件形状不认识（系统升级后目录/字面名变了）就保持
/// 启动时的字体并返回 false——字体是锦上添花，不能因为它出错。
pub fn upgrade_sheet_fonts(ctx: &egui::Context) -> bool {
    match load_sheet_fonts() {
        Some(fonts) => {
            ctx.set_fonts(definitions(Some(&fonts)));
            true
        }
        None => false,
    }
}

/// 读系统 TTC，取出苹方-简常规与中黑两个字面。75 MB 的原文件读完即丢，
/// 常驻的只有拼出来的两份（各 12 MB 左右）。
pub fn load_sheet_fonts() -> Option<SheetFonts> {
    let path = pingfang_path()?;
    let ttc = std::fs::read(path).ok()?;
    let regular = ttc::face_index(&ttc, PINGFANG_REGULAR)?;
    let semibold = ttc::face_index(&ttc, PINGFANG_SEMIBOLD)?;
    Some(SheetFonts {
        regular: ttc::extract_face(&ttc, regular)?,
        semibold: ttc::extract_face(&ttc, semibold)?,
    })
}

fn pingfang_path() -> Option<PathBuf> {
    let bundles = std::fs::read_dir(FONT_ASSET_ROOT).ok()?;
    bundles
        .flatten()
        .filter(|e| {
            e.file_name()
                .to_string_lossy()
                .starts_with(FONT_ASSET_PREFIX)
        })
        .filter_map(|bundle| std::fs::read_dir(bundle.path()).ok())
        .flatten()
        .flatten()
        .map(|asset| asset.path().join("AssetData").join(PINGFANG_FILE))
        .find(|path| path.is_file())
}

fn insert_font(fonts: &mut egui::FontDefinitions, name: &str, path: &str) -> bool {
    match std::fs::read(path) {
        Ok(bytes) => {
            fonts
                .font_data
                .insert(name.into(), egui::FontData::from_owned(bytes).into());
            true
        }
        Err(_) => false,
    }
}

/// 字体族 = 自己的字体（按优先级，缺哪个跳哪个）+ 一段兜底。
fn family(fonts: &egui::FontDefinitions, own: &[&str], fallback: &[String]) -> Vec<String> {
    own.iter()
        .filter(|name| fonts.font_data.contains_key(**name))
        .map(|name| (*name).to_string())
        .chain(fallback.iter().cloned())
        .collect()
}

/// 整套字体定义。`sheet` 是已经取好的苹方字面；没有就退回设置面板的 CJK 字体。
fn definitions(sheet: Option<&SheetFonts>) -> egui::FontDefinitions {
    let mut fonts = egui::FontDefinitions::default();
    let builtin = fonts.families[&egui::FontFamily::Proportional].clone();
    insert_font(&mut fonts, "system-symbols", SYMBOL_FONT);
    insert_font(&mut fonts, "apple-symbols", KEYCAP_FALLBACK_FONT);
    for path in CANDIDATES {
        if insert_font(&mut fonts, "system-cjk", path) {
            break;
        }
    }
    // 速览正文：苹方-简（取到时）→ 设置面板的 CJK 字体 → SF → 内置
    let mut sheet_regular = vec!["system-cjk", "system-symbols"];
    let mut sheet_bold = sheet_regular.clone();
    if let Some(sheet) = sheet {
        fonts.font_data.insert(
            "pingfang-regular".into(),
            egui::FontData::from_owned(sheet.regular.clone()).into(),
        );
        fonts.font_data.insert(
            "pingfang-semibold".into(),
            egui::FontData::from_owned(sheet.semibold.clone()).into(),
        );
        sheet_regular.insert(0, "pingfang-regular");
        sheet_bold.insert(0, "pingfang-semibold");
    }
    let settings = ["system-cjk", "system-symbols"];
    for kind in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
        let fallback = fonts.families[&kind].clone();
        fonts
            .families
            .insert(kind, family(&fonts, &settings, &fallback));
    }
    let keycaps = ["system-symbols", "apple-symbols"];
    for (name, own) in [
        (KEYCAPS_FAMILY, &keycaps[..]),
        (SHEET_FAMILY, &sheet_regular[..]),
        (SHEET_BOLD_FAMILY, &sheet_bold[..]),
    ] {
        fonts.families.insert(
            egui::FontFamily::Name(name.into()),
            family(&fonts, own, &builtin),
        );
    }
    fonts
}

mod ttc {
    //! TrueType Collection：按 PostScript 名找字面，再把该字面的表目录指向的数据
    //! 原样拼成一个独立的 sfnt 字体文件。只搬字节和查边界，不解析字形——
    //! 表内的偏移都是相对表头的，整表搬运后照样成立（唯一例外是 DSIG，它记的是
    //! 相对整个文件的偏移，但读字体的库不校验它）。
    //!
    //! 所有越界、截断、形状不对的输入都返回 None：egui 解析字体失败是直接
    //! panic，交给它的字节必须先在这里过一遍结构检查。

    fn u16(bytes: &[u8], at: usize) -> Option<u16> {
        Some(u16::from_be_bytes(bytes.get(at..at + 2)?.try_into().ok()?))
    }

    fn u32(bytes: &[u8], at: usize) -> Option<u32> {
        Some(u32::from_be_bytes(bytes.get(at..at + 4)?.try_into().ok()?))
    }

    /// 一个字面在 TTC 里的表目录项。
    struct Table {
        tag: [u8; 4],
        checksum: u32,
        offset: usize,
        length: usize,
    }

    /// 第 `index` 个字面的表目录起点。TTC 头：标签、版本、字面数、各字面的偏移。
    fn face_at(ttc: &[u8], index: u32) -> Option<usize> {
        if ttc.get(..4)? != b"ttcf" || index >= u32(ttc, 8)? {
            return None;
        }
        let at = usize::try_from(u32(ttc, 12 + 4 * index as usize)?).ok()?;
        let entries = 12 + 16 * u16(ttc, at + 4)? as usize;
        (at + entries <= ttc.len()).then_some(at)
    }

    fn tables(ttc: &[u8], face: usize) -> Option<Vec<Table>> {
        (0..u16(ttc, face + 4)? as usize)
            .map(|k| {
                let at = face + 12 + 16 * k;
                let offset = usize::try_from(u32(ttc, at + 8)?).ok()?;
                let length = usize::try_from(u32(ttc, at + 12)?).ok()?;
                ttc.get(offset..offset.checked_add(length)?)?;
                Some(Table {
                    tag: ttc.get(at..at + 4)?.try_into().ok()?,
                    checksum: u32(ttc, at + 4)?,
                    offset,
                    length,
                })
            })
            .collect()
    }

    fn find<'a>(tables: &'a [Table], tag: &[u8; 4]) -> Option<&'a Table> {
        tables.iter().find(|t| &t.tag == tag)
    }

    /// 字面的 PostScript 名（name 表 id 6），只认 Windows 平台的 Unicode 记录。
    fn postscript_name(ttc: &[u8], index: u32) -> Option<String> {
        let tables = tables(ttc, face_at(ttc, index)?)?;
        let name = find(&tables, b"name")?;
        let at = name.offset;
        let count = u16(ttc, at + 2)? as usize;
        let strings = at + u16(ttc, at + 4)? as usize;
        (0..count).find_map(|k| {
            let record = at + 6 + 12 * k;
            if u16(ttc, record)? != 3 || u16(ttc, record + 6)? != 6 {
                return None;
            }
            let length = u16(ttc, record + 8)? as usize;
            let start = strings + u16(ttc, record + 10)? as usize;
            let units: Vec<u16> = ttc
                .get(start..start + length)?
                .as_chunks::<2>()
                .0
                .iter()
                .map(|c| u16::from_be_bytes([c[0], c[1]]))
                .collect();
            String::from_utf16(&units).ok()
        })
    }

    /// PostScript 名等于 `name` 的字面序号。字面数封顶 256，防止对着垃圾数据空转。
    pub fn face_index(ttc: &[u8], name: &str) -> Option<u32> {
        (0..u32(ttc, 8)?.min(256)).find(|&i| postscript_name(ttc, i).is_some_and(|n| n == name))
    }

    /// 把第 `index` 个字面拼成独立的 sfnt 字体文件。缺任何必需的表都返回 None。
    pub fn extract_face(ttc: &[u8], index: u32) -> Option<Vec<u8>> {
        let face = face_at(ttc, index)?;
        let tables = tables(ttc, face)?;
        for required in [b"head", b"hhea", b"maxp", b"hmtx", b"cmap", b"name"] {
            find(&tables, required)?;
        }
        let outlines = (find(&tables, b"glyf").is_some() && find(&tables, b"loca").is_some())
            || find(&tables, b"CFF ").is_some();
        if !outlines {
            return None;
        }

        let count = tables.len();
        let mut out = Vec::with_capacity(
            12 + 16 * count + tables.iter().map(|t| (t.length + 3) & !3).sum::<usize>(),
        );
        // 头部 12 字节：sfnt 版本照抄字面自己的，搜索字段按规范算（读字体的库不看）
        let (mut search, mut selector) = (1usize, 0u16);
        while search * 2 <= count {
            search *= 2;
            selector += 1;
        }
        out.extend_from_slice(ttc.get(face..face + 4)?);
        out.extend_from_slice(&(count as u16).to_be_bytes());
        out.extend_from_slice(&((16 * search) as u16).to_be_bytes());
        out.extend_from_slice(&selector.to_be_bytes());
        out.extend_from_slice(&((16 * (count - search)) as u16).to_be_bytes());
        // 表数据从表头 + 表目录末尾开始
        let mut at = out.len() + 16 * count;
        let mut body = Vec::with_capacity(at);
        for table in &tables {
            out.extend_from_slice(&table.tag);
            out.extend_from_slice(&table.checksum.to_be_bytes());
            out.extend_from_slice(&(at as u32).to_be_bytes());
            out.extend_from_slice(&(table.length as u32).to_be_bytes());
            body.extend_from_slice(ttc.get(table.offset..table.offset + table.length)?);
            let padded = (table.length + 3) & !3;
            body.resize(body.len() + padded - table.length, 0);
            at += padded;
        }
        out.extend_from_slice(&body);
        Some(out)
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        /// name 表：一条 Windows/Unicode 记录，name id 6 = PostScript 名。
        fn name(ps: &str) -> Vec<u8> {
            let units: Vec<u8> = ps.encode_utf16().flat_map(|u| u.to_be_bytes()).collect();
            let mut table = vec![0, 0, 0, 1, 0, 18];
            table.extend_from_slice(&[0, 3, 0, 1, 0x04, 9, 0, 6]);
            table.extend_from_slice(&(units.len() as u16).to_be_bytes());
            table.extend_from_slice(&[0, 0]);
            table.extend_from_slice(&units);
            table
        }

        /// 一个字面的表目录：版本、表数、(标签, 偏移, 长度) 列表。
        fn face_directory(entries: &[(&[u8; 4], usize, usize)]) -> Vec<u8> {
            let mut dir = vec![0, 1, 0, 0];
            dir.extend_from_slice(&(entries.len() as u16).to_be_bytes());
            dir.extend_from_slice(&[0; 6]);
            for (tag, at, len) in entries {
                dir.extend_from_slice(*tag);
                dir.extend_from_slice(&[0; 4]);
                dir.extend_from_slice(&(*at as u32).to_be_bytes());
                dir.extend_from_slice(&(*len as u32).to_be_bytes());
            }
            dir
        }

        /// 造一个两字面的 TTC：B 在前、A 在后（验证按名字找而不是按顺序），
        /// 两个字面共用一份 head/hhea/maxp/hmtx/cmap/glyf/loca，各有自己的 name 表。
        fn sample_ttc() -> Vec<u8> {
            let shared: [(&[u8; 4], &[u8]); 7] = [
                (b"glyf", &[0xAA, 0xBB]),
                (b"loca", &[0, 1, 0, 2]),
                (b"head", &[0x5F, 0x0F, 0x3C, 0xF5]),
                (b"hhea", &[7; 4]),
                (b"maxp", &[8; 4]),
                (b"hmtx", &[9; 4]),
                (b"cmap", &[1, 2, 3]),
            ];
            let dir_size = 12 + 16 * (shared.len() + 1);
            // 头 12 字节 + 两个字面偏移 8 字节，然后依次是两个表目录、两个 name 表、共享表
            let face_b = 20;
            let face_a = face_b + dir_size;
            let name_a = face_a + dir_size;
            let a_name = name("A-Regular");
            let name_b = name_a + a_name.len();
            let b_name = name("B-Regular");
            let mut shared_at = Vec::new();
            let mut at = name_b + b_name.len();
            for (_, bytes) in shared {
                shared_at.push(at);
                at += bytes.len();
            }
            let mut ttc = b"ttcf".to_vec();
            ttc.extend_from_slice(&[0; 4]);
            ttc.extend_from_slice(&2u32.to_be_bytes());
            ttc.extend_from_slice(&(face_b as u32).to_be_bytes());
            ttc.extend_from_slice(&(face_a as u32).to_be_bytes());
            for (name_at, name) in [(name_b, &b_name), (name_a, &a_name)] {
                let mut entries = vec![(b"name", name_at, name.len())];
                entries.extend(
                    shared
                        .iter()
                        .zip(&shared_at)
                        .map(|((tag, bytes), at)| (*tag, *at, bytes.len())),
                );
                ttc.extend_from_slice(&face_directory(&entries));
            }
            ttc.extend_from_slice(&a_name);
            ttc.extend_from_slice(&b_name);
            for (_, bytes) in shared {
                ttc.extend_from_slice(bytes);
            }
            ttc
        }

        #[test]
        fn finds_faces_by_postscript_name_regardless_of_order() {
            let ttc = sample_ttc();
            assert_eq!(face_index(&ttc, "A-Regular"), Some(1));
            assert_eq!(face_index(&ttc, "B-Regular"), Some(0));
            assert_eq!(face_index(&ttc, "C-Regular"), None);
        }

        #[test]
        fn extraction_copies_tables_verbatim_into_a_standalone_file() {
            let ttc = sample_ttc();
            let index = face_index(&ttc, "A-Regular").unwrap();
            let before = tables(&ttc, face_at(&ttc, index).unwrap()).unwrap();
            let out = extract_face(&ttc, index).unwrap();
            assert_eq!(&out[..4], &[0, 1, 0, 0], "sfnt 版本照抄字面自己的");
            assert_eq!(u16(&out, 4), Some(before.len() as u16));
            let after = tables(&out, 0).unwrap();
            assert_eq!(before.len(), after.len());
            for (before, after) in before.iter().zip(&after) {
                assert_eq!(before.tag, after.tag);
                assert_eq!(
                    &ttc[before.offset..before.offset + before.length],
                    &out[after.offset..after.offset + after.length],
                );
            }
            assert_eq!(out.len() % 4, 0, "表数据按 4 字节对齐");
        }

        #[test]
        fn malformed_input_is_rejected_instead_of_panicking() {
            let ttc = sample_ttc();
            let index = face_index(&ttc, "A-Regular").unwrap();
            assert_eq!(extract_face(&ttc, 2), None, "字面序号越界");
            for cut in [3, 20, 60, ttc.len() - 1] {
                let truncated = &ttc[..cut];
                assert_eq!(face_index(truncated, "A-Regular"), None);
                assert_eq!(extract_face(truncated, index), None);
            }
            assert_eq!(face_index(b"not a font file at all", "A-Regular"), None);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn at_least_one_cjk_font_exists_on_this_mac() {
        let found = CANDIDATES.iter().any(|p| std::path::Path::new(p).exists());
        assert!(found, "本机应至少有一个候选中文字体");
    }

    /// 每个自定义族在任何情况下都要绑上字体：egui 对没绑字体的族直接 panic。
    #[test]
    fn custom_families_are_always_bound() {
        let fake = SheetFonts {
            regular: vec![1],
            semibold: vec![2],
        };
        for sheet in [None, Some(&fake)] {
            let fonts = definitions(sheet);
            for family in [KEYCAPS_FAMILY, SHEET_FAMILY, SHEET_BOLD_FAMILY] {
                let bound = &fonts.families[&egui::FontFamily::Name(family.into())];
                assert!(!bound.is_empty(), "{family} 没绑字体");
            }
        }
    }

    /// 本机的苹方：取出来的两个字面必须能被 egui 真正解析并排出文字。
    /// 字体在 pass 开头解析，解析失败 egui 会 panic，测试随之失败。
    /// 找不到字体资源（别的系统版本、CI）就跳过。
    #[test]
    fn extracted_pingfang_faces_are_accepted_by_egui() {
        let Some(fonts) = load_sheet_fonts() else {
            return;
        };
        assert!(fonts.regular.len() < 40 * 1024 * 1024);
        assert!(fonts.semibold.len() < 40 * 1024 * 1024);
        for (name, bytes) in [("regular", fonts.regular), ("semibold", fonts.semibold)] {
            let ctx = egui::Context::default();
            let mut definitions = egui::FontDefinitions::default();
            definitions
                .font_data
                .insert(name.into(), egui::FontData::from_owned(bytes).into());
            definitions.families.insert(
                egui::FontFamily::Name("under-test".into()),
                vec![name.into()],
            );
            ctx.set_fonts(definitions);
            ctx.begin_pass(egui::RawInput::default());
            let font_id = egui::FontId::new(14.0, egui::FontFamily::Name("under-test".into()));
            let galley = ctx.fonts_mut(|f| {
                f.layout_job(egui::text::LayoutJob::simple(
                    "隐藏 ZCode".into(),
                    font_id,
                    egui::Color32::WHITE,
                    f32::INFINITY,
                ))
            });
            let _ = ctx.end_pass();
            assert_eq!(galley.rows[0].glyphs.len(), 8, "{name}: 中英文都要排出字形");
        }
    }
}
