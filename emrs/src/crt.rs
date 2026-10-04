//! Descriptor calls in the C runtime that owns the wake-up descriptor.
//!
//! `open_channel` returns a descriptor of Emacs's C runtime. On Unix there is
//! one C library per process. On Windows each C runtime has its own
//! descriptor table: official Emacs builds use `msvcrt.dll`, while Rust
//! modules link the universal C runtime (UCRT). A UCRT call on an `msvcrt.dll`
//! descriptor ends the process with `0xC0000409`. The module therefore finds
//! the runtime that `emacs.exe` imports and calls `_write` and `_close` there.
//! See ADR 0001.

#[cfg(not(windows))]
pub(crate) use libc::{close, write};

#[cfg(windows)]
pub(crate) use windows::{close, write};

/// A C runtime that an executable imports.
#[cfg_attr(not(any(windows, test)), allow(dead_code))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Crt {
    Msvcrt,
    Ucrt,
}

/// Read the C runtime imported by a PE image as mapped in memory, where file
/// offsets are replaced by relative virtual addresses. `msvcrt.dll` wins over
/// UCRT when both appear. Returns `None` for a malformed image or one that
/// imports neither.
#[cfg(any(windows, test))]
fn imported_crt(image: &[u8]) -> Option<Crt> {
    let u16_at = |at: usize| Some(u16::from_le_bytes(image.get(at..at + 2)?.try_into().ok()?));
    let u32_at = |at: usize| Some(u32::from_le_bytes(image.get(at..at + 4)?.try_into().ok()?));

    if image.get(0..2)? != b"MZ" {
        return None;
    }
    let nt = u32_at(0x3c)? as usize;
    if image.get(nt..nt + 4)? != b"PE\0\0" {
        return None;
    }
    let optional = nt + 24;
    let (count_at, dirs_at) = match u16_at(optional)? {
        0x20b => (optional + 108, optional + 112),
        0x10b => (optional + 92, optional + 96),
        _ => return None,
    };
    if u32_at(count_at)? < 2 {
        return None;
    }
    let imports = u32_at(dirs_at + 8)? as usize;
    if imports == 0 {
        return None;
    }

    let mut found = None;
    for descriptor in (imports..).step_by(20) {
        let name = u32_at(descriptor + 12)? as usize;
        if name == 0 {
            break;
        }
        let bytes = image.get(name..)?;
        let name = &bytes[..bytes.iter().position(|&b| b == 0)?];
        let name = String::from_utf8_lossy(name).to_ascii_lowercase();
        if name == "msvcrt.dll" {
            return Some(Crt::Msvcrt);
        }
        if name == "ucrtbase.dll" || name.starts_with("api-ms-win-crt-") {
            found = Some(Crt::Ucrt);
        }
    }
    found
}

#[cfg(windows)]
mod windows {
    use super::{Crt, imported_crt};
    use std::os::raw::{c_char, c_int, c_uint, c_void};
    use std::sync::OnceLock;

    type WriteFn = unsafe extern "C" fn(c_int, *const c_void, c_uint) -> c_int;
    type CloseFn = unsafe extern "C" fn(c_int) -> c_int;

    unsafe extern "system" {
        fn GetModuleHandleA(name: *const c_char) -> *mut c_void;
        fn GetProcAddress(module: *mut c_void, name: *const c_char) -> *mut c_void;
    }

    struct Fns {
        write: WriteFn,
        close: CloseFn,
    }

    /// The C runtime of the executable that loaded this module.
    fn emacs_crt() -> Option<Crt> {
        unsafe {
            let base = GetModuleHandleA(std::ptr::null()) as *const u8;
            if base.is_null() {
                return None;
            }
            // The headers fill at least the first page of the mapped image.
            let headers = std::slice::from_raw_parts(base, 4096);
            let nt = u32::from_le_bytes(headers.get(0x3c..0x40)?.try_into().ok()?) as usize;
            let at = nt + 24 + 56;
            let size = u32::from_le_bytes(headers.get(at..at + 4)?.try_into().ok()?) as usize;
            imported_crt(std::slice::from_raw_parts(base, size))
        }
    }

    /// `_write` and `_close` of `msvcrt.dll` when Emacs uses it. `None` means
    /// the module's own UCRT is the right runtime: `ucrtbase.dll` is one DLL
    /// per process, so its descriptor table is shared with Emacs.
    fn msvcrt() -> Option<&'static Fns> {
        static FNS: OnceLock<Option<Fns>> = OnceLock::new();
        FNS.get_or_init(|| unsafe {
            if emacs_crt() != Some(Crt::Msvcrt) {
                return None;
            }
            let module = GetModuleHandleA(c"msvcrt.dll".as_ptr());
            if module.is_null() {
                return None;
            }
            let write = GetProcAddress(module, c"_write".as_ptr());
            let close = GetProcAddress(module, c"_close".as_ptr());
            if write.is_null() || close.is_null() {
                return None;
            }
            Some(Fns {
                write: std::mem::transmute::<*mut c_void, WriteFn>(write),
                close: std::mem::transmute::<*mut c_void, CloseFn>(close),
            })
        })
        .as_ref()
    }

    pub(crate) unsafe fn write(fd: c_int, buf: *const c_void, len: c_uint) -> c_int {
        match msvcrt() {
            Some(f) => unsafe { (f.write)(fd, buf, len) },
            None => unsafe { libc::write(fd, buf, len) },
        }
    }

    pub(crate) unsafe fn close(fd: c_int) -> c_int {
        match msvcrt() {
            Some(f) => unsafe { (f.close)(fd) },
            None => unsafe { libc::close(fd) },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Crt, imported_crt};

    /// A minimal mapped PE32+ image importing `dlls`.
    fn image(dlls: &[&str]) -> Vec<u8> {
        let mut img = vec![0u8; 0x1000];
        img[0..2].copy_from_slice(b"MZ");
        let nt = 0x80;
        img[0x3c..0x40].copy_from_slice(&(nt as u32).to_le_bytes());
        img[nt..nt + 4].copy_from_slice(b"PE\0\0");
        let opt = nt + 24;
        img[opt..opt + 2].copy_from_slice(&0x20bu16.to_le_bytes());
        img[opt + 108..opt + 112].copy_from_slice(&16u32.to_le_bytes());
        let imports = 0x400;
        img[opt + 120..opt + 124].copy_from_slice(&(imports as u32).to_le_bytes());
        let mut name_at = 0x800;
        for (i, dll) in dlls.iter().enumerate() {
            let d = imports + 20 * i;
            img[d + 12..d + 16].copy_from_slice(&(name_at as u32).to_le_bytes());
            img[name_at..name_at + dll.len()].copy_from_slice(dll.as_bytes());
            name_at += dll.len() + 1;
        }
        img
    }

    #[test]
    fn msvcrt_is_found() {
        let img = image(&["KERNEL32.dll", "msvcrt.dll", "USER32.dll"]);
        assert_eq!(imported_crt(&img), Some(Crt::Msvcrt));
    }

    #[test]
    fn ucrt_is_found() {
        let img = image(&["KERNEL32.dll", "api-ms-win-crt-stdio-l1-1-0.dll"]);
        assert_eq!(imported_crt(&img), Some(Crt::Ucrt));
        assert_eq!(imported_crt(&image(&["ucrtbase.dll"])), Some(Crt::Ucrt));
    }

    #[test]
    fn msvcrt_wins_over_ucrt() {
        let img = image(&["api-ms-win-crt-heap-l1-1-0.dll", "MSVCRT.DLL"]);
        assert_eq!(imported_crt(&img), Some(Crt::Msvcrt));
    }

    #[test]
    fn no_crt_or_bad_image_is_none() {
        assert_eq!(imported_crt(&image(&["KERNEL32.dll"])), None);
        assert_eq!(imported_crt(&image(&[])), None);
        assert_eq!(imported_crt(b"MZ"), None);
        let mut img = image(&["msvcrt.dll"]);
        img[0x80] = b'X';
        assert_eq!(imported_crt(&img), None);
        // A name that runs off the end of the image.
        let mut img = image(&["msvcrt.dll"]);
        img.truncate(0x805);
        assert_eq!(imported_crt(&img), None);
    }
}
