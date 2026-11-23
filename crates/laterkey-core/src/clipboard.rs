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
            },
        );
        if self.entries.len() > MAX_ENTRIES {
            self.entries.pop();
        }
        Some(id)
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
    fn empty_history_basics() {
        let mut h = History::new();
        assert!(h.is_empty());
        assert_eq!(h.len(), 0);
        assert!(h.get(1).is_none());
        assert!(!h.touch(1, 0));
        assert_eq!(h.record("", 1, "com.x"), Some(1), "空文本也算一次拷贝");
    }
}
