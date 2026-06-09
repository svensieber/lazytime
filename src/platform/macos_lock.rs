use std::sync::mpsc;

use core_foundation::base::TCFType;
use core_foundation::dictionary::CFDictionary;
use core_foundation::string::CFString;
use core_foundation_sys::base::{CFGetTypeID, CFTypeRef};
use core_foundation_sys::dictionary::{CFDictionaryGetValue, CFDictionaryRef};
use core_foundation_sys::number::{CFBooleanGetTypeID, CFBooleanGetValue, CFBooleanRef};

use super::types::{LockEvent, LockSource};

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn CGSessionCopyCurrentDictionary() -> CFDictionaryRef;
}

pub fn spawn_lock_monitor(tx_lock: mpsc::Sender<LockEvent>) {
    std::thread::spawn(move || {
        tracing::info!("macos lock monitor: session polling active");
        let mut last_locked = current_locked_state();
        loop {
            std::thread::sleep(std::time::Duration::from_millis(800));
            let locked = current_locked_state();
            if locked != last_locked {
                let event = if locked {
                    LockEvent::Locked(LockSource::MacosSession)
                } else {
                    LockEvent::Unlocked(LockSource::MacosSession)
                };
                let _ = tx_lock.send(event);
                last_locked = locked;
            }
        }
    });
}

fn current_locked_state() -> bool {
    let dict_ref = unsafe { CGSessionCopyCurrentDictionary() };
    if dict_ref.is_null() {
        return false;
    }

    let dict: CFDictionary = unsafe { CFDictionary::wrap_under_create_rule(dict_ref) };
    let key = CFString::new("CGSSessionScreenIsLocked");
    let value =
        unsafe { CFDictionaryGetValue(dict.as_concrete_TypeRef(), key.as_CFTypeRef() as *const _) };
    if value.is_null() {
        return false;
    }

    let value_ref = value as CFTypeRef;
    let is_boolean = unsafe { CFGetTypeID(value_ref) == CFBooleanGetTypeID() };
    is_boolean && unsafe { CFBooleanGetValue(value as CFBooleanRef) }
}
