//! User-facing transient notifications surfaced via the statusbar warning
//! flash. Library code calls [`warn`]; the binary installs a handler at
//! startup that routes the message into its `State`. Decoupled this way so
//! library crates do not need to know about the binary's state struct.

use std::sync::atomic::{AtomicPtr, Ordering};

type Handler = fn(&str);

static HANDLER: AtomicPtr<()> = AtomicPtr::new(std::ptr::null_mut());

/// Installs the process-wide warning handler. Replacing an existing handler
/// is allowed; the new handler wins. Pass a function pointer (not a closure
/// that captures state) so this stays allocation-free.
pub fn set_handler(handler: Handler) {
    HANDLER.store(handler as *mut (), Ordering::Release);
}

/// Removes any installed handler. Subsequent [`warn`] calls become no-ops.
pub fn clear_handler() {
    HANDLER.store(std::ptr::null_mut(), Ordering::Release);
}

/// Surfaces a short warning string to the user. Cost when no handler is
/// installed: one relaxed atomic load + null check.
pub fn warn(msg: &str) {
    let ptr = HANDLER.load(Ordering::Acquire);
    if ptr.is_null() {
        return;
    }
    let handler: Handler = unsafe { std::mem::transmute(ptr) };
    handler(msg);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static LAST: Mutex<Option<String>> = Mutex::new(None);

    fn record(msg: &str) {
        *LAST.lock().unwrap() = Some(msg.to_string());
    }

    #[test]
    fn warn_with_no_handler_is_noop() {
        clear_handler();
        warn("nothing");
    }

    #[test]
    fn handler_receives_message() {
        *LAST.lock().unwrap() = None;
        set_handler(record);
        warn("hello");
        assert_eq!(LAST.lock().unwrap().as_deref(), Some("hello"));
        clear_handler();
    }
}
