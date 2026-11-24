//! 剪贴板（NSPasteboard）轮询采集。
//!
//! changeCount 是板上内容的版本号，变了才去取内容——大多数轮询只花一次
//! 属性读的代价。取内容前先查类型表：密码管理器（1Password、Bitwarden…）
//! 拷贝的一次性口令会带 org.nspasteboard.ConcealedType / TransientType
//! 标记（<https://nspasteboard.org> 约定），带标记的一律不采。
//!
//! 只在后台线程调用，不在拦截回调里。

use objc2::rc::autoreleasepool;
use objc2_app_kit::{NSPasteboard, NSPasteboardTypeString};
use objc2_foundation::{NSData, NSString};

use laterkey_core::ImageFormat;

/// 「隐藏内容」标记：密码管理器拷贝的口令、密钥之类，不许落历史。
pub const CONCEALED_TYPE: &str = "org.nspasteboard.ConcealedType";
/// 「一次性内容」标记：拷贝即失效的验证码之类，采了也没用，不采。
pub const TRANSIENT_TYPE: &str = "org.nspasteboard.TransientType";

/// 采到的一段内容。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ClipContent {
    /// 带禁采标记（Concealed/Transient）。
    Concealed,
    /// 板上没有可采的内容（文件引用等）。
    Nothing,
    /// 一段文本。
    Text(String),
    /// 一张图片：原始字节原样入库，贴回时原样写回，不转码。
    Image { bytes: Vec<u8>, format: ImageFormat },
}

/// 板上内容的版本号。取不到（理论上不会）返回 0。
pub fn change_count() -> u64 {
    autoreleasepool(|_pool| {
        let pb = NSPasteboard::generalPasteboard();
        pb.changeCount().max(0) as u64
    })
}

/// 取板上的内容：先查类型表（带禁采标记就不读字符串），再读
/// NSPasteboardTypeString。
pub fn read() -> Option<ClipContent> {
    autoreleasepool(|_pool| {
        let pb = NSPasteboard::generalPasteboard();
        // 类型表先摊平成字符串，判断走下面的纯函数（可单测）
        let names: Vec<String> = pb.types()?.iter().map(|t| t.to_string()).collect();
        if is_concealed(&names) {
            return Some(ClipContent::Concealed);
        }
        // SAFETY: NSPasteboardTypeString 是系统导出的常量字符串
        match pb.stringForType(unsafe { NSPasteboardTypeString }) {
            Some(string) => Some(ClipContent::Text(string.to_string())),
            None => read_image(&pb),
        }
    })
}

/// 板上没有文本时找图片：PNG 优先（浏览器「复制图像」），再 TIFF
///（系统截图存剪贴板的类型）。都不在就 Nothing。
fn read_image(pb: &NSPasteboard) -> Option<ClipContent> {
    for (ty, format) in [
        ("public.png", ImageFormat::Png),
        ("public.tiff", ImageFormat::Tiff),
    ] {
        // NSData 只读拷成 Vec，绑定里这个方法是 safe 的
        if let Some(data) = pb.dataForType(&NSString::from_str(ty)) {
            return Some(ClipContent::Image {
                bytes: data.to_vec(),
                format,
            });
        }
    }
    Some(ClipContent::Nothing)
}

/// 类型表里有没有禁采标记：任一项精确命中就拦。
pub fn is_concealed(types: &[String]) -> bool {
    types
        .iter()
        .any(|t| t == CONCEALED_TYPE || t == TRANSIENT_TYPE)
}

/// 文本值不值得入库：空串和纯空白不要（误碰空格/回车拷一下不该进历史）。
pub fn worth_keeping(text: &str) -> bool {
    !text.trim().is_empty()
}

/// 把一张图片写回剪贴板（清板后按原格式放类型与字节）。贴回历史图片条目用。
/// 写完采集会再次采到同一份字节，走历史的去重，不成环。
pub fn write_image(bytes: &[u8], format: ImageFormat) -> bool {
    autoreleasepool(|_pool| {
        let pb = NSPasteboard::generalPasteboard();
        pb.clearContents();
        let ty = match format {
            ImageFormat::Png => "public.png",
            ImageFormat::Tiff => "public.tiff",
        };
        // NSData 只借字节，绑定里这个方法是 safe 的
        pb.setData_forType(Some(&NSData::with_bytes(bytes)), &NSString::from_str(ty))
    })
}

/// 把一段文本写回剪贴板（清板后只放 NSPasteboardTypeString）。
/// 贴回历史条目用。写完采集会再次采到同一段文本，走历史的去重，不成环。
pub fn write_string(text: &str) -> bool {
    autoreleasepool(|_pool| {
        let pb = NSPasteboard::generalPasteboard();
        pb.clearContents();
        // SAFETY: NSPasteboardTypeString 是系统导出的常量字符串
        pb.setString_forType(&NSString::from_str(text), unsafe { NSPasteboardTypeString })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn concealed_flags_are_detected() {
        assert!(is_concealed(&[CONCEALED_TYPE.into()]));
        assert!(is_concealed(&[TRANSIENT_TYPE.into()]));
        assert!(is_concealed(&[
            "public.utf8-plain-text".into(),
            CONCEALED_TYPE.into(),
        ]));
        // 普通文本类型不拦
        assert!(!is_concealed(&["public.utf8-plain-text".into()]));
        assert!(!is_concealed(&[]));
        // 名字相近但不完全一样的标记不拦（按约定精确匹配）
        assert!(!is_concealed(&["org.nspasteboard.Concealed".into()]));
    }

    #[test]
    fn only_worthwhile_text_is_kept() {
        assert!(worth_keeping("hello"));
        assert!(worth_keeping(" x "), "两端的空白是拷贝的一部分，保留");
        assert!(!worth_keeping(""));
        assert!(!worth_keeping("   "));
        assert!(!worth_keeping(" \t\r\n"));
    }
}
