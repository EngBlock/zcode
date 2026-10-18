//! Apple USB devices attaching and detaching, from IOKit's notifications,
//! so the destination menu follows an iPhone being plugged in or out
//! without listing devices every few seconds (`devicectl list devices`
//! costs ~60 ms of CPU and wakes CoreDevice each time).
//!
//! One thread watches for the whole process: it registers for the first
//! match and the termination of `IOUSBHostDevice` services with Apple's
//! vendor id and runs its own run loop, and each notification wakes every
//! receiver from [`changes`]. Devices on the network have no notification;
//! the slow poll and the refreshes on activation and on opening the
//! destination menu find those.

use std::ffi::{CStr, c_char, c_void};
use std::sync::{Mutex, Once};

use futures::channel::mpsc::{UnboundedReceiver, UnboundedSender, unbounded};

/// Apple's USB vendor id.
const APPLE_VENDOR_ID: i32 = 0x05ac;

static LISTENERS: Mutex<Vec<UnboundedSender<()>>> = Mutex::new(Vec::new());
static WATCH: Once = Once::new();

/// A stream that yields each time an Apple USB device attaches or detaches.
/// The first call starts the watching thread.
pub fn changes() -> UnboundedReceiver<()> {
    let (sender, receiver) = unbounded();
    LISTENERS.lock().unwrap().push(sender);
    WATCH.call_once(|| {
        let spawned = std::thread::Builder::new().name("zcode-usb".into()).spawn(|| {
            if let Err(err) = watch() {
                ::log::debug!("USB device notifications: {err}");
            }
        });
        if let Err(err) = spawned {
            ::log::debug!("USB device notifications: {err}");
        }
    });
    receiver
}

fn notify_listeners() {
    LISTENERS.lock().unwrap().retain(|sender| sender.unbounded_send(()).is_ok());
}

type IoObject = u32;
type CFTypeRef = *const c_void;

#[link(name = "IOKit", kind = "framework")]
unsafe extern "C" {
    fn IONotificationPortCreate(main_port: u32) -> *mut c_void;
    fn IONotificationPortGetRunLoopSource(port: *mut c_void) -> CFTypeRef;
    fn IOServiceMatching(name: *const c_char) -> *mut c_void;
    fn IOServiceAddMatchingNotification(
        port: *mut c_void,
        notification_type: *const c_char,
        matching: *mut c_void,
        callback: extern "C" fn(*mut c_void, IoObject),
        refcon: *mut c_void,
        notification: *mut IoObject,
    ) -> i32;
    fn IOIteratorNext(iterator: IoObject) -> IoObject;
    fn IOObjectRelease(object: IoObject) -> i32;
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    static kCFRunLoopDefaultMode: CFTypeRef;
    fn CFRunLoopGetCurrent() -> *mut c_void;
    fn CFRunLoopAddSource(run_loop: *mut c_void, source: CFTypeRef, mode: CFTypeRef);
    fn CFRunLoopRun();
    fn CFStringCreateWithCString(allocator: CFTypeRef, string: *const c_char, encoding: u32) -> CFTypeRef;
    fn CFNumberCreate(allocator: CFTypeRef, number_type: isize, value: *const c_void) -> CFTypeRef;
    fn CFDictionarySetValue(dictionary: *mut c_void, key: CFTypeRef, value: CFTypeRef);
    fn CFRetain(object: CFTypeRef) -> CFTypeRef;
    fn CFRelease(object: CFTypeRef);
}

const UTF8: u32 = 0x0800_0100;
const NUMBER_SINT32: isize = 3;
const FIRST_MATCH: &CStr = c"IOServiceFirstMatch";
const TERMINATED: &CStr = c"IOServiceTerminate";

/// Releases what `iterator` holds, which re-arms its notification.
fn drain(iterator: IoObject) {
    loop {
        // SAFETY: `iterator` is a notification iterator IOKit gave us.
        let object = unsafe { IOIteratorNext(iterator) };
        if object == 0 {
            break;
        }
        // SAFETY: each object IOIteratorNext returns is ours to release.
        unsafe { IOObjectRelease(object) };
    }
}

extern "C" fn device_changed(_: *mut c_void, iterator: IoObject) {
    drain(iterator);
    notify_listeners();
}

/// Registers for Apple USB devices attaching and detaching, then runs this
/// thread's run loop, which delivers the notifications, for good.
fn watch() -> Result<(), String> {
    // SAFETY: plain IOKit and CoreFoundation calls, made and kept on this
    // thread; every object created stays alive as long as the run loop.
    unsafe {
        let port = IONotificationPortCreate(0);
        if port.is_null() {
            return Err("IONotificationPortCreate failed".into());
        }
        let matching = IOServiceMatching(c"IOUSBHostDevice".as_ptr());
        if matching.is_null() {
            return Err("IOServiceMatching failed".into());
        }
        let key = CFStringCreateWithCString(std::ptr::null(), c"idVendor".as_ptr(), UTF8);
        let vendor = CFNumberCreate(std::ptr::null(), NUMBER_SINT32, (&APPLE_VENDOR_ID as *const i32).cast());
        CFDictionarySetValue(matching, key, vendor);
        CFRelease(key);
        CFRelease(vendor);
        // Each registration consumes a reference to the dictionary.
        CFRetain(matching);
        for kind in [FIRST_MATCH, TERMINATED] {
            let mut iterator: IoObject = 0;
            let status = IOServiceAddMatchingNotification(
                port,
                kind.as_ptr(),
                matching,
                device_changed,
                std::ptr::null_mut(),
                &mut iterator,
            );
            if status != 0 {
                return Err(format!("IOServiceAddMatchingNotification: {status:#x}"));
            }
            // The devices already attached; draining arms the notification.
            drain(iterator);
        }
        CFRunLoopAddSource(CFRunLoopGetCurrent(), IONotificationPortGetRunLoopSource(port), kCFRunLoopDefaultMode);
        ::log::debug!("watching Apple USB devices");
        CFRunLoopRun();
    }
    Ok(())
}
