//! 裸 FFI 声明。常量与函数签名逐个对照本机 macOS 27.2 SDK 头文件核实。
//!
//! 沿用 LaterScreen `mac_fnkey_tap.rs` 的做法：不引入绑定 crate，用到什么声明什么。

#![allow(non_upper_case_globals)]

use std::ffi::{c_char, c_void};

use crate::geometry::{CGPoint, CGRect};

pub type CGEventRef = *mut c_void;
pub type CGEventSourceRef = *mut c_void;
pub type CGEventTapProxy = *const c_void;
pub type CFMachPortRef = *mut c_void;
pub type CFRunLoopRef = *mut c_void;
pub type CFRunLoopSourceRef = *mut c_void;
pub type CFTypeRef = *const c_void;
pub type CFStringRef = *const c_void;
pub type AXUIElementRef = *mut c_void;
pub type AXValueRef = *mut c_void;

// ---- CGEventType（CGEventTypes.h）
pub const EVENT_LEFT_MOUSE_DOWN: u32 = 1;
pub const EVENT_LEFT_MOUSE_UP: u32 = 2;
pub const EVENT_LEFT_MOUSE_DRAGGED: u32 = 6;
pub const EVENT_KEY_DOWN: u32 = 10;
pub const EVENT_KEY_UP: u32 = 11;
pub const EVENT_FLAGS_CHANGED: u32 = 12;
/// 回调超时被系统禁用（通知型事件，event 为 NULL）
pub const EVENT_TAP_DISABLED_BY_TIMEOUT: u32 = 0xFFFF_FFFE;
/// 被用户输入禁用（同上）
pub const EVENT_TAP_DISABLED_BY_USER_INPUT: u32 = 0xFFFF_FFFF;

// ---- CGEventField（CGEventTypes.h）
pub const FIELD_AUTOREPEAT: u32 = 8;
pub const FIELD_KEYCODE: u32 = 9;
/// 事件来源自定义数据：用来标记自己注入的合成事件
pub const FIELD_USER_DATA: u32 = 42;

// ---- CGEventTapLocation / Placement / Options
pub const TAP_HID: u32 = 0;
pub const TAP_HEAD_INSERT: u32 = 0;
pub const TAP_OPTION_DEFAULT: u32 = 0;

// ---- CGEventSourceStateID
pub const SOURCE_STATE_HID: i32 = 1;

// ---- CGWindowListOption
pub const LIST_ON_SCREEN_ONLY: u32 = 1 << 0;

// ---- CoreFoundation 杂项
/// kCFStringEncodingUTF8
pub const UTF8: u32 = 0x0800_0100;
/// kCFNumberSInt64Type / kCFNumberFloat64Type：按较宽的类型取值，任何整数/浮点都能转出来
pub const NUMBER_SINT64: isize = 4;
pub const NUMBER_FLOAT64: isize = 6;

// ---- AXValueType
pub const AX_VALUE_CGRECT: u32 = 3;

pub type CGEventTapCallBack =
    unsafe extern "C" fn(CGEventTapProxy, u32, CGEventRef, *mut c_void) -> CGEventRef;

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    pub fn CGEventTapCreate(
        tap: u32,
        place: u32,
        options: u32,
        events_of_interest: u64,
        callback: CGEventTapCallBack,
        user_info: *mut c_void,
    ) -> CFMachPortRef;
    pub fn CGEventTapEnable(tap: CFMachPortRef, enable: bool);
    pub fn CGEventGetIntegerValueField(event: CGEventRef, field: u32) -> i64;
    pub fn CGEventSetIntegerValueField(event: CGEventRef, field: u32, value: i64);
    pub fn CGEventGetFlags(event: CGEventRef) -> u64;
    pub fn CGEventSetFlags(event: CGEventRef, flags: u64);
    pub fn CGEventGetLocation(event: CGEventRef) -> CGPoint;
    #[cfg(test)]
    pub fn CGEventCreateMouseEvent(
        source: CGEventSourceRef,
        mouse_type: u32,
        mouse_cursor_position: CGPoint,
        mouse_button: u32,
    ) -> CGEventRef;
    pub fn CGEventCreateKeyboardEvent(
        source: CGEventSourceRef,
        virtual_key: u16,
        key_down: bool,
    ) -> CGEventRef;
    pub fn CGEventSetType(event: CGEventRef, ty: u32);
    pub fn CGEventPost(tap: u32, event: CGEventRef);
    pub fn CGEventSourceCreate(state_id: i32) -> CGEventSourceRef;
    pub fn CGEventSourceSetLocalEventsSuppressionInterval(source: CGEventSourceRef, seconds: f64);
    #[cfg(test)]
    pub fn CGEventSourceFlagsState(state_id: i32) -> u64;
    pub fn CGPreflightListenEventAccess() -> bool;
    pub fn CGPreflightPostEventAccess() -> bool;
    pub fn CGWindowListCopyWindowInfo(option: u32, relative_to: u32) -> CFTypeRef;
    pub fn CGRectMakeWithDictionaryRepresentation(dict: CFTypeRef, rect: *mut CGRect) -> bool;

    pub static kCGWindowOwnerPID: CFStringRef;
    pub static kCGWindowLayer: CFStringRef;
    pub static kCGWindowAlpha: CFStringRef;
    pub static kCGWindowBounds: CFStringRef;
}

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    pub static kCFRunLoopCommonModes: CFStringRef;
    pub static kCFBooleanTrue: CFTypeRef;
    pub fn CFRunLoopGetCurrent() -> CFRunLoopRef;
    pub fn CFRunLoopRun();
    pub fn CFRunLoopStop(rl: CFRunLoopRef);
    pub fn CFRetain(cf: CFTypeRef) -> CFTypeRef;
    pub fn CFRelease(cf: CFTypeRef);
    pub fn CFMachPortCreateRunLoopSource(
        allocator: *const c_void,
        port: CFMachPortRef,
        order: isize,
    ) -> CFRunLoopSourceRef;
    pub fn CFRunLoopAddSource(rl: CFRunLoopRef, src: CFRunLoopSourceRef, mode: CFStringRef);
    pub fn CFRunLoopRemoveSource(rl: CFRunLoopRef, src: CFRunLoopSourceRef, mode: CFStringRef);
    pub fn CFMachPortInvalidate(port: CFMachPortRef);
    pub fn CFStringCreateWithCString(
        allocator: *const c_void,
        c_str: *const c_char,
        encoding: u32,
    ) -> CFStringRef;
    pub fn CFStringGetLength(s: CFStringRef) -> isize;
    pub fn CFStringGetCString(
        s: CFStringRef,
        buffer: *mut c_char,
        buffer_size: isize,
        encoding: u32,
    ) -> bool;
    pub fn CFArrayGetCount(array: CFTypeRef) -> isize;
    // 返回值不带引用计数，存活期由数组保证
    pub fn CFArrayGetValueAtIndex(array: CFTypeRef, idx: isize) -> CFTypeRef;
    // 返回值不带引用计数，存活期由字典保证
    pub fn CFDictionaryGetValue(dict: CFTypeRef, key: CFTypeRef) -> CFTypeRef;
    pub fn CFBooleanGetValue(value: CFTypeRef) -> bool;
    pub fn CFNumberGetValue(number: CFTypeRef, the_type: isize, value_ptr: *mut c_void) -> bool;
    pub fn CFURLCopyFileSystemPath(url: CFTypeRef, path_style: isize) -> CFStringRef;
    pub fn CFGetTypeID(cf: CFTypeRef) -> usize;
    pub fn CFStringGetTypeID() -> usize;
    pub fn CFBooleanGetTypeID() -> usize;
    pub fn CFArrayGetTypeID() -> usize;
    pub fn CFDictionaryGetTypeID() -> usize;
    pub fn CFNumberGetTypeID() -> usize;
    pub fn CFURLGetTypeID() -> usize;
    pub static kCFTypeDictionaryKeyCallBacks: CFDictionaryKeyCallBacks;
    pub static kCFTypeDictionaryValueCallBacks: CFDictionaryValueCallBacks;
    pub fn CFDictionaryCreate(
        allocator: *const c_void,
        keys: *const *const c_void,
        values: *const *const c_void,
        num_values: isize,
        key_callbacks: *const CFDictionaryKeyCallBacks,
        value_callbacks: *const CFDictionaryValueCallBacks,
    ) -> CFTypeRef;
    // 走 cfprefsd 的实时值（系统快捷键可能被用户改过，直接读 plist 会拿到落盘延迟的旧值）
    pub fn CFPreferencesCopyAppValue(key: CFStringRef, application_id: CFStringRef) -> CFTypeRef;
}

/// CFDictionary 的 callbacks 是 opaque 结构体，只取地址、不访问字段。
#[repr(C)]
pub struct CFDictionaryKeyCallBacks {
    _opaque: [usize; 5],
}
#[repr(C)]
pub struct CFDictionaryValueCallBacks {
    _opaque: [usize; 5],
}

#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    pub fn AXIsProcessTrusted() -> bool;
    pub fn AXIsProcessTrustedWithOptions(options: CFTypeRef) -> bool;
    pub static kAXTrustedCheckOptionPrompt: CFStringRef;
    pub fn AXUIElementCreateApplication(pid: i32) -> AXUIElementRef;
    pub fn AXUIElementCopyAttributeValue(
        element: AXUIElementRef,
        attribute: CFStringRef,
        value: *mut CFTypeRef,
    ) -> i32;
    pub fn AXUIElementSetAttributeValue(
        element: AXUIElementRef,
        attribute: CFStringRef,
        value: CFTypeRef,
    ) -> i32;
    pub fn AXUIElementSetMessagingTimeout(element: AXUIElementRef, timeout: f32) -> i32;
    pub fn AXValueGetValue(value: AXValueRef, the_type: u32, value_ptr: *mut c_void) -> bool;
    pub fn AXValueGetTypeID() -> usize;
}

// IsSecureEventInputEnabled 由 HIToolbox 导出，但没有公开头文件，只能手写声明。
// HIToolbox 是 Carbon 的子框架，链接器在顶层找不到它，所以挂到 Carbon 伞框架上。
#[link(name = "Carbon", kind = "framework")]
extern "C" {
    pub fn IsSecureEventInputEnabled() -> bool;
}
