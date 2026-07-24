//! Unix-specific platform code. Unix is the only target, so this is not
//! behind a trait -- `sys` re-exports it directly.
//!
//! the terminal-i/o pieces (raw mode, sigwinch resize injection, polling
//! stdin reader, write_stdout) live in the `tty` workspace crate and are
//! re-exported below. fs + icu helpers stay here because they're
//! edit-only.

use std::ffi::{c_char, c_int, c_void};
use std::fs::File;
use std::io;
use std::mem::{self, MaybeUninit};
use std::os::fd::AsRawFd as _;
use std::path::Path;
use std::ptr::NonNull;

#[cfg(edit_icu_renaming_auto_detect)]
use stdext::arena::Arena;
#[cfg(edit_icu_renaming_auto_detect)]
use stdext::collections::BString;

pub use tty::{
    Deinit, get_window_size, init, inject_window_size_into_stdin, open_stdin_if_redirected,
    read_stdin, switch_modes, write_stdout,
};

#[derive(Clone, PartialEq, Eq)]
pub struct FileId {
    st_dev: libc::dev_t,
    st_ino: libc::ino_t,
}

/// Checks whether the current process has write permission on `path`.
///
/// Uses `access(2)` with `W_OK`, which follows symlinks and honours the
/// filesystem's ACLs. Returns `false` on any error (path missing, permission
/// denied, etc.) — callers should only invoke this for paths known to exist.
pub fn is_path_writable(path: &Path) -> bool {
    use std::os::unix::ffi::OsStrExt as _;

    let bytes = path.as_os_str().as_bytes();
    let Ok(c_path) = std::ffi::CString::new(bytes) else {
        return false;
    };
    unsafe { libc::access(c_path.as_ptr(), libc::W_OK) == 0 }
}

/// Returns a unique identifier for the given file by handle or path.
pub fn file_id(file: Option<&File>, path: &Path) -> io::Result<FileId> {
    let file = match file {
        Some(f) => f,
        None => &File::open(path)?,
    };

    unsafe {
        let mut stat = MaybeUninit::<libc::stat>::uninit();
        check_int_return(libc::fstat(file.as_raw_fd(), stat.as_mut_ptr()))?;
        let stat = stat.assume_init();
        Ok(FileId { st_dev: stat.st_dev, st_ino: stat.st_ino })
    }
}

unsafe fn load_library(name: *const c_char) -> io::Result<NonNull<c_void>> {
    unsafe {
        NonNull::new(libc::dlopen(name, libc::RTLD_LAZY))
            .ok_or_else(|| from_raw_os_error(libc::ENOENT))
    }
}

/// Loads a function from a dynamic library.
///
/// # Safety
///
/// This function is highly unsafe as it requires you to know the exact type
/// of the function you're loading. No type checks whatsoever are performed.
//
// It'd be nice to constrain T to std::marker::FnPtr, but that's unstable.
pub unsafe fn get_proc_address<T>(handle: NonNull<c_void>, name: *const c_char) -> io::Result<T> {
    unsafe {
        let sym = libc::dlsym(handle.as_ptr(), name);
        if sym.is_null() {
            Err(from_raw_os_error(libc::ENOENT))
        } else {
            Ok(mem::transmute_copy(&sym))
        }
    }
}

pub struct LibIcu {
    pub libicuuc: NonNull<c_void>,
    pub libicui18n: NonNull<c_void>,
}

pub fn load_icu() -> io::Result<LibIcu> {
    const fn const_str_eq(a: &str, b: &str) -> bool {
        let a = a.as_bytes();
        let b = b.as_bytes();
        let mut i = 0;

        loop {
            if i >= a.len() || i >= b.len() {
                return a.len() == b.len();
            }
            if a[i] != b[i] {
                return false;
            }
            i += 1;
        }
    }

    const LIBICUUC: &str = concat!(env!("EDIT_CFG_ICUUC_SONAME"), "\0");
    const LIBICUI18N: &str = concat!(env!("EDIT_CFG_ICUI18N_SONAME"), "\0");

    if const { const_str_eq(LIBICUUC, LIBICUI18N) } {
        let icu = unsafe { load_library(LIBICUUC.as_ptr().cast())? };
        Ok(LibIcu { libicuuc: icu, libicui18n: icu })
    } else {
        let libicuuc = unsafe { load_library(LIBICUUC.as_ptr().cast())? };
        let libicui18n = unsafe { load_library(LIBICUI18N.as_ptr().cast())? };
        Ok(LibIcu { libicuuc, libicui18n })
    }
}

/// ICU, by default, adds the major version as a suffix to each exported symbol.
/// They also recommend to disable this for system-level installations (`runConfigureICU Linux --disable-renaming`),
/// but I found that many (most?) Linux distributions don't do this for some reason.
/// This function returns the suffix, if any.
#[cfg(edit_icu_renaming_auto_detect)]
pub fn icu_detect_renaming_suffix(arena: &Arena, handle: NonNull<c_void>) -> BString<'_> {
    unsafe {
        type T = *const c_void;

        let mut res = BString::empty();

        // Check if the ICU library is using unversioned symbols.
        // Return an empty suffix in that case.
        if get_proc_address::<T>(handle, c"u_errorName".as_ptr()).is_ok() {
            return res;
        }

        // In the versions (63-76) and distributions (Arch/Debian) I tested,
        // this symbol seems to be always present. This allows us to call `dladdr`.
        // It's the `UCaseMap::~UCaseMap()` destructor which for some reason isn't
        // in a namespace. Thank you ICU maintainers for this oversight.
        let proc = match get_proc_address::<T>(handle, c"_ZN8UCaseMapD1Ev".as_ptr()) {
            Ok(proc) => proc,
            Err(_) => return res,
        };

        // `dladdr` is specific to GNU's libc unfortunately.
        let mut info: libc::Dl_info = mem::zeroed();
        let ret = libc::dladdr(proc, &mut info);
        if ret == 0 {
            return res;
        }

        // The library path is in `info.dli_fname`.
        let path = match std::ffi::CStr::from_ptr(info.dli_fname).to_str() {
            Ok(name) => name,
            Err(_) => return res,
        };

        let path = match std::fs::read_link(path) {
            Ok(path) => path,
            Err(_) => path.into(),
        };

        // I'm going to assume it's something like "libicuuc.so.76.1".
        let path = path.into_os_string();
        let path = path.to_string_lossy();
        let suffix_start = match path.rfind(".so.") {
            Some(pos) => pos + 4,
            None => return res,
        };
        let version = &path[suffix_start..];
        let version_end = version.find('.').unwrap_or(version.len());
        let version = &version[..version_end];

        res.push(arena, '_');
        res.push_str(arena, version);
        res
    }
}

#[cfg(edit_icu_renaming_auto_detect)]
#[allow(clippy::not_unsafe_ptr_arg_deref)]
pub fn icu_add_renaming_suffix<'a, 'b, 'r>(
    arena: &'a Arena,
    name: *const c_char,
    suffix: &str,
) -> *const c_char
where
    'a: 'r,
    'b: 'r,
{
    if suffix.is_empty() {
        name
    } else {
        // SAFETY: In this particular case we know that the string
        // is valid UTF-8, because it comes from icu.rs.
        let name = unsafe { std::ffi::CStr::from_ptr(name) };
        let name = unsafe { name.to_str().unwrap_unchecked() };

        let mut res = BString::empty();
        res.reserve(arena, name.len() + suffix.len() + 1);
        res.push_str(arena, name);
        res.push_str(arena, suffix);
        res.push(arena, '\0');
        res.as_ptr() as *const c_char
    }
}

#[inline]
#[cold]
fn last_os_error() -> io::Error {
    io::Error::last_os_error()
}

#[inline]
#[cold]
fn from_raw_os_error(code: c_int) -> io::Error {
    io::Error::from_raw_os_error(code)
}

fn check_int_return(ret: libc::c_int) -> io::Result<libc::c_int> {
    if ret < 0 { Err(last_os_error()) } else { Ok(ret) }
}
