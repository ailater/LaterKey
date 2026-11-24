//! 剪贴板历史：纯逻辑的环形缓冲与去重。
//!
//! 时间由调用方以毫秒时间戳传入，本模块不取系统时间，保证纯逻辑可测。
//! 历史仅存内存，退出即清；落盘存储是 ROADMAP P2 的待办项——serde 形状
//! 已备好（JSON 往返有测试），将来落盘直接用。

use serde::{Deserialize, Serialize};

/// 历史上限，满了淘汰最旧一条。
pub const MAX_ENTRIES: usize = 100;
/// 单条文本上限（字节数），超过直接丢弃，不入历史。
pub const MAX_TEXT_BYTES: usize = 1024 * 1024;
/// 单张图片上限（字节数）。5K 全屏截图的 PNG 一般 5–15 MiB，这里留足余量。
pub const MAX_IMAGE_BYTES: usize = 20 * 1024 * 1024;
/// 全部图片条目的总内存预算：图片远大于文本，100 张截图能把内存吃到 GB 级，
/// 超预算从最旧的图片开始淘汰（文本条目不受预算影响）。
pub const MAX_IMAGE_TOTAL_BYTES: usize = 64 * 1024 * 1024;

/// 图片格式：采集时按剪贴板类型定，贴回时按原格式原样写回，不做转码。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ImageFormat {
    Png,
    Tiff,
}

/// 一张剪贴板图片的原始字节。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImageData {
    pub bytes: Vec<u8>,
    pub format: ImageFormat,
}

/// 一条剪贴板记录。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClipboardEntry {
    /// 自增 id，贴回时按它找条目。
    pub id: u64,
    pub text: String,
    /// 最近一次被拷贝/贴回的时间，毫秒时间戳，调用方传入。
    pub captured_at: u64,
    /// 拷贝来源应用的 bundle id。
    pub source_bundle_id: String,
    /// 图片条目时有值；此时 `text` 是给列表和搜索用的描述（如「图片 1920×1080」）。
    #[serde(default)]
    pub image: Option<ImageData>,
}

/// 从新到旧的剪贴板历史：index 0 是最新。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct History {
    /// 下一个要分配的 id，从 1 开始。参与 serde 形状：将来落盘恢复历史时
    /// 接着分配，不与恢复出来的旧条目撞 id。
    next_id: u64,
    entries: Vec<ClipboardEntry>,
}

impl Default for History {
    fn default() -> Self {
        Self::new()
    }
}

impl History {
    pub fn new() -> Self {
        Self {
            next_id: 1,
            entries: Vec::new(),
        }
    }

    /// 记录一次新拷贝，返回条目 id。
    ///
    /// 文本超过 [`MAX_TEXT_BYTES`] 时丢弃，返回 `None`。
    /// 与已有条目内容相同则视为同一条：移到顶部、刷新时间，id 不变。
    /// 否则新建条目插到顶部；满了淘汰最旧一条。
    pub fn record(
        &mut self,
        text: impl Into<String>,
        captured_at: u64,
        source_bundle_id: impl Into<String>,
    ) -> Option<u64> {
        let text = text.into();
        if text.len() > MAX_TEXT_BYTES {
            return None;
        }
        // 先比长度再比内容：不同长度是常态（大部分拷贝互不相同），
        // 把全文比较挡在长度相等之后
        if let Some(pos) = self
            .entries
            .iter()
            .position(|e| e.text.len() == text.len() && e.text == text)
        {
            self.move_to_top(pos, captured_at);
            return Some(self.entries[0].id);
        }
        let id = self.next_id;
        self.next_id += 1;
        self.entries.insert(
            0,
            ClipboardEntry {
                id,
                text,
                captured_at,
                source_bundle_id: source_bundle_id.into(),
                image: None,
            },
        );
        self.inserted();
        Some(id)
    }

    /// 记录一张图片拷贝，返回条目 id。
    ///
    /// 超过 [`MAX_IMAGE_BYTES`] 拒收；图片总内存超出 [`MAX_IMAGE_TOTAL_BYTES`]
    /// 时从最旧的图片开始淘汰给新图腾地方（单张上限低于总预算，删光必然放得下）。
    /// 与已有图片逐字节相同则视为同一条：移顶、刷新时间，id 不变——贴回写回
    /// 板上的就是这份字节，再次采集命中这里，不成环。
    pub fn record_image(
        &mut self,
        bytes: Vec<u8>,
        format: ImageFormat,
        captured_at: u64,
        source_bundle_id: impl Into<String>,
        label: impl Into<String>,
    ) -> Option<u64> {
        if bytes.len() > MAX_IMAGE_BYTES {
            return None;
        }
        if let Some(pos) = self.entries.iter().position(|e| {
            e.image
                .as_ref()
                .is_some_and(|img| img.bytes.len() == bytes.len() && img.bytes == bytes)
        }) {
            self.move_to_top(pos, captured_at);
            return Some(self.entries[0].id);
        }
        while self.total_image_bytes() + bytes.len() > MAX_IMAGE_TOTAL_BYTES {
            let victim = self.entries.iter().rposition(|e| e.image.is_some())?;
            self.entries.remove(victim);
        }
        let id = self.next_id;
        self.next_id += 1;
        self.entries.insert(
            0,
            ClipboardEntry {
                id,
                text: label.into(),
                captured_at,
                source_bundle_id: source_bundle_id.into(),
                image: Some(ImageData { bytes, format }),
            },
        );
        self.inserted();
        Some(id)
    }

    /// 现存图片条目的总字节数（内存预算的账本）。
    fn total_image_bytes(&self) -> usize {
        self.entries
            .iter()
            .filter_map(|e| e.image.as_ref())
            .map(|img| img.bytes.len())
            .sum()
    }

    /// 插顶后的公共收尾：满了淘汰最旧一条。
    fn inserted(&mut self) {
        if self.entries.len() > MAX_ENTRIES {
            self.entries.pop();
        }
    }

    /// 贴回成功后按 id 置顶并刷新时间。
    ///
    /// 写回剪贴板后采集会再次采到同一段文本，走 [`History::record`]
    /// 的去重分支时它已在顶部，只刷新时间，不会成环。
    pub fn touch(&mut self, id: u64, now: u64) -> bool {
        match self.entries.iter().position(|e| e.id == id) {
            Some(pos) => {
                self.move_to_top(pos, now);
                true
            }
            None => false,
        }
    }

    /// 把 pos 上的条目移到顶部并刷新时间。
    fn move_to_top(&mut self, pos: usize, captured_at: u64) {
        let mut entry = self.entries.remove(pos);
        entry.captured_at = captured_at;
        self.entries.insert(0, entry);
    }

    /// 按 id 取条目。
    pub fn get(&self, id: u64) -> Option<&ClipboardEntry> {
        self.entries.iter().find(|e| e.id == id)
    }

    /// 从新到旧的所有条目。
    pub fn entries(&self) -> &[ClipboardEntry] {
        &self.entries
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// 从 PNG 字节里读 IHDR 宽高，给图片条目生成「图片 宽×高」描述用。
/// 解析不动（不是 PNG/太短）返回 None，显示退化为「图片」。
pub fn png_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    const SIG: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    if bytes.len() < 24 || bytes[..8] != SIG || &bytes[12..16] != b"IHDR" {
        return None;
    }
    let w = u32::from_be_bytes(bytes[16..20].try_into().ok()?);
    let h = u32::from_be_bytes(bytes[20..24].try_into().ok()?);
    Some((w, h))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// record 三条，返回 (历史, [id0, id1, id2])，时间戳依次 1000/2000/3000。
    fn three_entries() -> (History, [u64; 3]) {
        let mut h = History::new();
        let a = h.record("a", 1000, "com.a").unwrap();
        let b = h.record("b", 2000, "com.b").unwrap();
        let c = h.record("c", 3000, "com.c").unwrap();
        (h, [a, b, c])
    }

    #[test]
    fn record_keeps_newest_first() {
        let (h, _) = three_entries();
        let texts: Vec<&str> = h.entries().iter().map(|e| e.text.as_str()).collect();
        assert_eq!(texts, ["c", "b", "a"]);
        assert_eq!(h.entries()[0].source_bundle_id, "com.c");
        assert_eq!(h.len(), 3);
    }

    #[test]
    fn duplicate_moves_to_top_without_new_id() {
        let (mut h, [a, b, _]) = three_entries();
        // 再拷贝一次 "a"：移到顶部，id 不变，时间刷新
        let id = h.record("a", 9000, "com.x").unwrap();
        assert_eq!(id, a);
        assert_eq!(h.entries()[0].id, a);
        assert_eq!(h.entries()[0].captured_at, 9000);
        assert_eq!(h.entries()[0].source_bundle_id, "com.a", "去重不换来源");
        assert_eq!(h.len(), 3, "去重不新增条目");
        assert_eq!(h.entries()[1].text, "c");
        assert_ne!(h.entries()[1].id, b);
    }

    #[test]
    fn max_entries_evicts_oldest() {
        let mut h = History::new();
        for i in 0..MAX_ENTRIES as u64 {
            h.record(format!("t{i}"), i, "com.x");
        }
        assert_eq!(h.len(), MAX_ENTRIES);
        assert_eq!(h.entries()[0].text, format!("t{}", MAX_ENTRIES - 1));
        assert_eq!(h.entries()[MAX_ENTRIES - 1].text, "t0");
        // 再来一条新的：最旧的 t0 被淘汰
        h.record("fresh", 9999, "com.x");
        assert_eq!(h.len(), MAX_ENTRIES);
        assert_eq!(h.entries()[0].text, "fresh");
        assert!(h.get(1).is_none(), "t0 的 id=1 已被淘汰");
        assert_eq!(h.entries()[MAX_ENTRIES - 1].text, "t1");
    }

    #[test]
    fn oversized_text_is_rejected() {
        let mut h = History::new();
        let ok = "x".repeat(MAX_TEXT_BYTES);
        let too_big = "x".repeat(MAX_TEXT_BYTES + 1);
        assert!(h.record(ok, 1, "com.x").is_some(), "恰好 1 MiB 收");
        assert!(h.record(too_big, 2, "com.x").is_none(), "超 1 MiB 丢弃");
        assert_eq!(h.len(), 1);
        assert_eq!(h.next_id, 2, "拒收不消耗 id");
    }

    #[test]
    fn json_round_trip() {
        let (mut h, _) = three_entries();
        h.record("d".repeat(10), 4000, "com.d");
        let text = serde_json::to_string(&h).unwrap();
        let mut back: History = serde_json::from_str(&text).unwrap();
        assert_eq!(back, h);
        // 恢复后 id 接着分配，不与旧条目相撞
        let id = back.record("new", 5000, "com.e").unwrap();
        assert!(back.get(id).is_some());
        assert_eq!(back.len(), 5);
    }

    #[test]
    fn touch_moves_entry_to_top() {
        let (mut h, [a, _, _]) = three_entries();
        let now = 10_000;
        assert!(h.touch(a, now));
        assert_eq!(h.entries()[0].id, a);
        assert_eq!(h.entries()[0].captured_at, now);
        assert_eq!(h.len(), 3);
        // 不存在的 id 返回 false，历史不动
        assert!(!h.touch(999, now));
        assert_eq!(h.entries()[0].id, a);
    }

    #[test]
    fn paste_back_then_recapture_does_not_loop() {
        let (mut h, [_, b, _]) = three_entries();
        // 贴回 b：置顶
        assert!(h.touch(b, 4000));
        assert_eq!(h.entries()[0].text, "b");
        // 写回剪贴板被再次采到：与顶部相同，天然去重，不成环
        let id = h.record("b", 5000, "com.self").unwrap();
        assert_eq!(id, b);
        assert_eq!(h.entries()[0].text, "b");
        assert_eq!(h.len(), 3);
    }

    #[test]
    fn image_entries_basic_and_dedupe() {
        let mut h = History::new();
        let png = vec![0x89, b'P', b'N', b'G'];
        let id = h
            .record_image(png.clone(), ImageFormat::Png, 1000, "com.a", "图片")
            .unwrap();
        assert_eq!(h.len(), 1);
        assert_eq!(h.entries()[0].text, "图片");
        assert_eq!(
            h.entries()[0].image.as_ref().unwrap().format,
            ImageFormat::Png
        );
        // 同一张图再拷一次：去重移顶，id 不变
        let again = h
            .record_image(png, ImageFormat::Png, 2000, "com.b", "图片")
            .unwrap();
        assert_eq!(again, id);
        assert_eq!(h.len(), 1);
        assert_eq!(h.entries()[0].captured_at, 2000);
        assert_eq!(h.entries()[0].source_bundle_id, "com.a", "去重不换来源");
        // 贴回置顶后再次采集不成环（与文本同款语义）
        assert!(h.touch(id, 3000));
        assert_eq!(
            h.record_image(
                h.entries()[0].image.as_ref().unwrap().bytes.clone(),
                ImageFormat::Png,
                4000,
                "com.self",
                "图片"
            )
            .unwrap(),
            id
        );
        assert_eq!(h.len(), 1);
    }

    #[test]
    fn oversized_image_is_rejected_without_consuming_id() {
        let mut h = History::new();
        let ok = vec![0u8; MAX_IMAGE_BYTES];
        let too_big = vec![0u8; MAX_IMAGE_BYTES + 1];
        assert!(h
            .record_image(ok, ImageFormat::Tiff, 1, "com.x", "图片")
            .is_some());
        let next = h.next_id;
        assert!(h
            .record_image(too_big, ImageFormat::Tiff, 2, "com.x", "图片")
            .is_none());
        assert_eq!(h.next_id, next, "拒收不消耗 id");
        assert_eq!(h.len(), 1);
    }

    #[test]
    fn image_budget_evicts_oldest_images_only() {
        let mut h = History::new();
        h.record("文本条目", 1, "com.x");
        // 每张恰好单张上限（20 MiB）且字节互不相同（相同会走去重）：三张共 60 MiB
        let mut big = vec![7u8; MAX_IMAGE_BYTES];
        for (at, name) in [(2u64, "图片A"), (3, "图片B"), (4, "图片C")] {
            big[0] = at as u8;
            h.record_image(big.clone(), ImageFormat::Png, at, "com.x", name);
        }
        assert_eq!(h.len(), 4);
        // 第四张放不下（60+20 > 64）：最旧的「图片A」被淘汰，文本条目保留
        h.record_image(
            vec![8u8; MAX_IMAGE_BYTES],
            ImageFormat::Png,
            5,
            "com.x",
            "图片D",
        );
        assert!(h.entries().iter().all(|e| e.text != "图片A"));
        for kept in ["文本条目", "图片B", "图片C", "图片D"] {
            assert!(h.entries().iter().any(|e| e.text == kept), "{kept} 应保留");
        }
        assert!(h.total_image_bytes() <= MAX_IMAGE_TOTAL_BYTES);
    }

    #[test]
    fn png_dimensions_reads_ihdr() {
        let mut png = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
        png.extend_from_slice(&[0, 0, 0, 13]); // IHDR 长度
        png.extend_from_slice(b"IHDR");
        png.extend_from_slice(&1920u32.to_be_bytes());
        png.extend_from_slice(&1080u32.to_be_bytes());
        assert_eq!(png_dimensions(&png), Some((1920, 1080)));
        assert_eq!(png_dimensions(&png[..20]), None, "太短");
        assert_eq!(png_dimensions(b"not a png at all......"), None);
    }

    #[test]
    fn empty_history_basics() {
        let mut h = History::new();
        assert!(h.is_empty());
        assert_eq!(h.len(), 0);
        assert!(h.get(1).is_none());
        assert!(!h.touch(1, 0));
        assert_eq!(h.record("", 1, "com.x"), Some(1), "空文本也算一次拷贝");
    }
}
