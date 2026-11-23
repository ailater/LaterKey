//! CoreFoundation 小工具：RAII 包装、字符串转换、类型判断。

use std::ffi::{c_char, c_void, CStr};
use std::ptr::null_mut;

use super::ffi::*;

/// CoreFoundation 对象的 RAII 包装，Drop 时 CFRelease。
///
/// 只包「自己拥有引用」的对象：Create / Copy / Retain 系列的返回值。
/// `CFArrayGetValueAtIndex`、`CFDictionaryGetValue` 这类不增加引用计数的返回值
/// 不能包，否则会过度释放。
pub struct Cf<T = c_void>(pub *mut T);

impl<T> Cf<T> {
    /// 接管一个归自己所有的引用。
    ///
    /// # Safety
    ///
    /// `ptr` 必须是调用方拥有引用的对象（或 NULL），之后不能再由别人释放。
    pub unsafe fn owned(ptr: *mut T) -> Self {
        Self(ptr)
    }

    pub fn as_cf(&self) -> CFTypeRef {
        self.0.cast_const().cast()
    }

    pub fn is_null(&self) -> bool {
        self.0.is_null()
    }
}

impl<T> Drop for Cf<T> {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: 构造时已保证这是一个自己拥有的引用
            unsafe { CFRelease(self.as_cf()) };
        }
    }
}

/// 常量字符串 → CFString（`c"AXChildren"` 这种字面量）。
pub fn cfstr(lit: &CStr) -> Cf {
    // SAFETY: 字面量以 NUL 结尾
    unsafe { Cf::owned(CFStringCreateWithCString(null_mut(), lit.as_ptr(), UTF8) as *mut c_void) }
}

/// CFString → Rust String。
///
/// # Safety
///
/// `s` 必须是 NULL 或一个有效的 CFString。
pub unsafe fn to_rust(s: CFStringRef) -> Option<String> {
    if s.is_null() {
        return None;
    }
    let units = CFStringGetLength(s);
    if units == 0 {
        return Some(String::new());
    }
    // UTF-16 单元数 → UTF-8 最坏情况（每单元 4 字节）+ NUL。SDK 里的
    // CFStringGetMaximumSizeForString 是宏不是函数，所以这里自己算。
    let cap = units * 4 + 1;
    let mut buf = vec![0 as c_char; cap.max(1) as usize];
    if !CFStringGetCString(s, buf.as_mut_ptr(), cap, UTF8) {
        return None;
    }
    // SAFETY: CFStringGetCString 保证写入了 NUL
    Some(CStr::from_ptr(buf.as_ptr()).to_string_lossy().into_owned())
}

// 以下类型判断都要求传入有效的 CF 对象（不能是 NULL）：CFGetTypeID(NULL) 会崩。

/// # Safety
///
/// `cf` 必须是一个有效的 CF 对象。
pub unsafe fn is_string(cf: CFTypeRef) -> bool {
    CFGetTypeID(cf) == CFStringGetTypeID()
}

/// # Safety
///
/// `cf` 必须是一个有效的 CF 对象。
pub unsafe fn is_boolean(cf: CFTypeRef) -> bool {
    CFGetTypeID(cf) == CFBooleanGetTypeID()
}

/// # Safety
///
/// `cf` 必须是一个有效的 CF 对象。
pub unsafe fn is_number(cf: CFTypeRef) -> bool {
    CFGetTypeID(cf) == CFNumberGetTypeID()
}

/// # Safety
///
/// `cf` 必须是一个有效的 CF 对象。
pub unsafe fn is_url(cf: CFTypeRef) -> bool {
    CFGetTypeID(cf) == CFURLGetTypeID()
}

/// # Safety
///
/// `cf` 必须是一个有效的 CF 对象。
pub unsafe fn is_array(cf: CFTypeRef) -> bool {
    CFGetTypeID(cf) == CFArrayGetTypeID()
}

/// # Safety
///
/// `cf` 必须是一个有效的 CF 对象。
pub unsafe fn is_dictionary(cf: CFTypeRef) -> bool {
    CFGetTypeID(cf) == CFDictionaryGetTypeID()
}

/// 字典里某个键的值。返回值不带引用计数，存活期由字典保证。
///
/// # Safety
///
/// `dict` 必须是有效的 CFDictionary，且在返回值用完之前保持存活。
pub unsafe fn dict_get(dict: CFTypeRef, key: CFTypeRef) -> Option<CFTypeRef> {
    let v = CFDictionaryGetValue(dict, key);
    (!v.is_null()).then_some(v)
}

/// CFNumber → 整数。按 SInt64 取，任何整数类型都转得出来。
///
/// 不是 CFNumber 就返回 None。**必须**先校验：`CFNumberGetValue` 收到别的类型
/// 不会返回 false，而是抛 Objective-C 异常，Rust 这边接不住，进程直接终止。
///
/// # Safety
///
/// `number` 必须是一个有效的 CF 对象。
pub unsafe fn number_to_i64(number: CFTypeRef) -> Option<i64> {
    if !is_number(number) {
        return None;
    }
    let mut out: i64 = 0;
    (CFNumberGetValue(number, NUMBER_SINT64, &mut out as *mut i64 as *mut c_void)).then_some(out)
}

/// CFNumber → 浮点。同上，类型不符返回 None 而不是抛异常。
///
/// # Safety
///
/// `number` 必须是一个有效的 CF 对象。
pub unsafe fn number_to_f64(number: CFTypeRef) -> Option<f64> {
    if !is_number(number) {
        return None;
    }
    let mut out: f64 = 0.0;
    (CFNumberGetValue(number, NUMBER_FLOAT64, &mut out as *mut f64 as *mut c_void)).then_some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cfstr_round_trips() {
        let s = cfstr(c"访达 AXDockItem");
        let back = unsafe { to_rust(s.as_cf()) };
        assert_eq!(back.as_deref(), Some("访达 AXDockItem"));
    }

    #[test]
    fn empty_string_is_some_empty() {
        let s = cfstr(c"");
        assert_eq!(unsafe { to_rust(s.as_cf()) }, Some(String::new()));
        assert_eq!(unsafe { to_rust(std::ptr::null()) }, None);
    }

    #[test]
    fn type_checks() {
        let s = cfstr(c"x");
        assert!(unsafe { is_string(s.as_cf()) });
        assert!(!unsafe { is_boolean(s.as_cf()) });
        assert!(!unsafe { is_url(s.as_cf()) });
        assert!(!unsafe { is_array(s.as_cf()) });
        assert!(!unsafe { is_dictionary(s.as_cf()) });
        // dict_get 只能用于真正的字典：传别的类型会抛 ObjC 异常（与
        // CFNumberGetValue 同类），调用前必须先用 is_dictionary 校验
        assert!(unsafe { is_boolean(kCFBooleanTrue) });
    }

    #[test]
    fn numbers_convert() {
        let text = cfstr(c"not a number");
        assert!(!unsafe { is_number(text.as_cf()) });
        // 类型不符必须返回 None：直接调 CFNumberGetValue 会抛 ObjC 异常终止进程
        assert!(unsafe { number_to_i64(text.as_cf()) }.is_none());
        assert!(unsafe { number_to_f64(text.as_cf()) }.is_none());
        assert!(!unsafe { is_boolean(text.as_cf()) });
    }

    #[test]
    fn null_cf_is_a_noop() {
        let none: Cf = unsafe { Cf::owned(std::ptr::null_mut()) };
        assert!(none.is_null());
        drop(none);
    }
}
