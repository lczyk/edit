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

/// Serialises tests that install or clear the handler.
///
/// There is one handler for the whole process, so two tests touching it in
/// parallel see each other's state -- one clearing it mid-flight makes the
/// other's `warn` vanish. Every test that cares takes this, including
/// [`crate::sanity::capture::trips`].
#[cfg(any(test, feature = "sanity"))]
pub fn serialise_tests<R>(f: impl FnOnce() -> R) -> R {
    use std::sync::Mutex;
    static LOCK: Mutex<()> = Mutex::new(());
    // A test that panicked while holding this poisoned it; its state is of no
    // interest, but the lock still has to be usable by everyone after it.
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    f()
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
        serialise_tests(|| {
            clear_handler();
            warn("nothing");
        });
    }

    #[test]
    fn handler_receives_message() {
        serialise_tests(|| {
            *LAST.lock().unwrap() = None;
            set_handler(record);
            warn("hello");
            assert_eq!(LAST.lock().unwrap().as_deref(), Some("hello"));
            clear_handler();
        });
    }
}
