//! Debug-only sanity checks. Two macros, both at crate root since
//! `#[macro_export]` ignores module nesting:
//!
//! - [`crate::sanity_check!`] -- soft check. on failure, log a line to
//!   `$TMPDIR/edit/log/sanity-YYYYMMDD.log` and call [`crate::notify::warn`]
//!   with a short summary. execution continues. per call-site dedup with a
//!   1s window so hot-loop checks do not flood. without the `sanity` cargo
//!   feature this expands to nothing -- release builds carry zero cost.
//!
//! - [`crate::sanity_assert!`] -- hard check. with the feature on: same
//!   logging + notify, then panics with the logfile path embedded so the
//!   terminal output points the user at the trail. without the feature it
//!   degrades to a plain `debug_assert!` -- debug builds keep coverage,
//!   release builds carry zero cost.
//!
//! env var `EDIT_SANITY_PANIC=1` promotes every `sanity_check!` trip to a
//! panic on the first non-suppressed firing. useful when bisecting.

#![allow(unused_imports, unused_macros)]

#[cfg(feature = "sanity")]
use std::collections::HashMap;
#[cfg(feature = "sanity")]
use std::fs::OpenOptions;
#[cfg(feature = "sanity")]
use std::io::Write;
#[cfg(feature = "sanity")]
use std::path::PathBuf;
#[cfg(feature = "sanity")]
use std::sync::Mutex;
#[cfg(feature = "sanity")]
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// Records a failed check. Soft-fail by default; panics if
/// `EDIT_SANITY_PANIC=1` is set in the environment.
#[cfg(feature = "sanity")]
pub fn record(file: &'static str, line: u32, name: &'static str, msg: &str, hard: bool) {
    if !hard && is_suppressed(file, line) {
        return;
    }
    write_log(file, line, name, msg);
    let summary = format!("sanity: {name}: {msg}");
    crate::notify::warn(&summary);
    if hard || std::env::var_os("EDIT_SANITY_PANIC").is_some() {
        let path_hint =
            log_path().map(|p| p.display().to_string()).unwrap_or_else(|| "<unavailable>".into());
        panic!("sanity::{name} at {file}:{line}: {msg}\n  log: {path_hint}");
    }
}

/// No-op stub used when the `sanity` feature is off. Kept so the macros
/// compile to a call that the optimiser deletes.
#[cfg(not(feature = "sanity"))]
#[inline(always)]
pub fn record(_file: &'static str, _line: u32, _name: &'static str, _msg: &str, _hard: bool) {}

#[cfg(feature = "sanity")]
const DEDUP_WINDOW: Duration = Duration::from_secs(1);

#[cfg(feature = "sanity")]
static DEDUP: Mutex<Option<HashMap<(&'static str, u32), Instant>>> = Mutex::new(None);

#[cfg(feature = "sanity")]
fn is_suppressed(file: &'static str, line: u32) -> bool {
    let Ok(mut guard) = DEDUP.lock() else {
        return false;
    };
    let map = guard.get_or_insert_with(HashMap::new);
    let now = Instant::now();
    match map.get(&(file, line)) {
        Some(last) if now.duration_since(*last) < DEDUP_WINDOW => true,
        _ => {
            map.insert((file, line), now);
            false
        }
    }
}

#[cfg(feature = "sanity")]
fn write_log(file: &str, line: u32, name: &str, msg: &str) {
    let Some(path) = log_path() else { return };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let Ok(mut f) = OpenOptions::new().create(true).append(true).open(&path) else {
        return;
    };
    let pid = std::process::id();
    let (ts, _, _, _) = now_parts();
    let _ = writeln!(f, "{ts} {pid} {file}:{line} {name}: {msg}");
}

#[cfg(feature = "sanity")]
fn log_path() -> Option<PathBuf> {
    let (_, y, m, d) = now_parts();
    let mut p = std::env::temp_dir();
    p.push("edit");
    p.push("log");
    p.push(format!("sanity-{y:04}{m:02}{d:02}.log"));
    Some(p)
}

#[cfg(feature = "sanity")]
fn now_parts() -> (String, i32, u32, u32) {
    let secs = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs() as i64;
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    unsafe { libc::gmtime_r(&secs, &mut tm) };
    let y = tm.tm_year + 1900;
    let mo = (tm.tm_mon + 1) as u32;
    let d = tm.tm_mday as u32;
    let hh = tm.tm_hour as u32;
    let mm = tm.tm_min as u32;
    let ss = tm.tm_sec as u32;
    let ts = format!("{y:04}-{mo:02}-{d:02}T{hh:02}:{mm:02}:{ss:02}Z");
    (ts, y, mo, d)
}

/// Soft check. On failure logs + notifies, then continues. Suppressed if
/// the same call site fired within the last second.
///
/// `sanity::check!(check_name, cond, "fmt {args...}", args...)`
#[macro_export]
macro_rules! sanity_check {
    ($name:ident, $cond:expr $(,)?) => {{
        if !$cond {
            $crate::sanity::record(file!(), line!(), stringify!($name), "", false);
        }
    }};
    ($name:ident, $cond:expr, $($arg:tt)+) => {{
        if !$cond {
            let msg = format!($($arg)+);
            $crate::sanity::record(file!(), line!(), stringify!($name), &msg, false);
        }
    }};
}

/// Hard check. On failure: with the `sanity` feature on, logs + notifies
/// then panics with the logfile path embedded in the message so the
/// dying terminal points the user at the trail. Without the feature it
/// degrades to a plain `debug_assert!` so debug builds still catch the
/// invariant -- just without the logfile breadcrumb. Not suppressed by
/// dedup.
///
/// `sanity_assert!(check_name, cond, "fmt {args...}", args...)`
#[macro_export]
macro_rules! sanity_assert {
    ($name:ident, $cond:expr $(,)?) => {{
        #[cfg(feature = "sanity")]
        {
            if !$cond {
                $crate::sanity::record(file!(), line!(), stringify!($name), "", true);
            }
        }
        #[cfg(not(feature = "sanity"))]
        {
            debug_assert!($cond, concat!("sanity::", stringify!($name)));
        }
    }};
    ($name:ident, $cond:expr, $($arg:tt)+) => {{
        #[cfg(feature = "sanity")]
        {
            if !$cond {
                let msg = format!($($arg)+);
                $crate::sanity::record(file!(), line!(), stringify!($name), &msg, true);
            }
        }
        #[cfg(not(feature = "sanity"))]
        {
            debug_assert!($cond, $($arg)+);
        }
    }};
}

#[cfg(all(test, feature = "sanity"))]
mod tests {
    #[test]
    fn check_writes_log_and_calls_handler() {
        use std::sync::{Mutex, OnceLock};
        static RECEIVED: OnceLock<Mutex<Vec<String>>> = OnceLock::new();
        let received = RECEIVED.get_or_init(|| Mutex::new(Vec::new()));
        fn record_handler(msg: &str) {
            RECEIVED.get().unwrap().lock().unwrap().push(msg.to_string());
        }
        crate::notify::set_handler(record_handler);

        // Intentional trip with a unique name so dedup does not suppress.
        crate::sanity_check!(test_intentional_trip, false, "expected = {}", 42);

        let msgs = received.lock().unwrap();
        assert!(msgs.iter().any(|m| m.contains("test_intentional_trip")));
        assert!(msgs.iter().any(|m| m.contains("expected = 42")));

        crate::notify::clear_handler();
    }
}
