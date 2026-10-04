//! Descriptor calls in the C runtime that owns the wake-up descriptor.
//!
//! `open_channel` returns a descriptor of Emacs's C runtime. On Unix there is
//! one C library per process. On Windows each C runtime has its own
//! descriptor table: official Emacs builds use `msvcrt.dll`, while Rust
//! modules link the universal C runtime (UCRT). A UCRT call on an `msvcrt.dll`
//! descriptor ends the process with `0xC0000409`. The module therefore finds
//! the runtime that `emacs.exe` imports and calls `_write` and `_close` of
//! that DLL, whatever C runtime the module itself links. See ADR 0001.

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
/// offsets are replaced by relative virtual addresses. `byte(at)` returns the
/// byte at offset `at`, or `None` outside the image. `msvcrt.dll` wins over
/// UCRT when both appear; an unreadable import name is skipped. Returns `None`
/// for a malformed image or one that imports neither.
#[cfg(any(windows, test))]
fn imported_crt(byte: impl Fn(usize) -> Option<u8>) -> Option<Crt> {
    let at = |base: usize, off: usize| base.checked_add(off);
    let bytes = |pos: usize, n: usize| -> Option<u32> {
        (0..n).rev().try_fold(0u32, |acc, k| Some(acc << 8 | byte(pos.checked_add(k)?)? as u32))
    };
    let u16_at = |pos: usize| bytes(pos, 2);
    let u32_at = |pos: usize| bytes(pos, 4).map(|v| v as usize);

    if u16_at(0)? != u32::from_le_bytes([b'M', b'Z', 0, 0]) {
        return None;
    }
    let nt = u32_at(0x3c)?;
    if u32_at(nt)? != u32::from_le_bytes(*b"PE\0\0") as usize {
        return None;
    }
    let optional = at(nt, 24)?;
    let (count_at, dirs_at) = match u16_at(optional)? {
        0x20b => (at(optional, 108)?, at(optional, 112)?),
        0x10b => (at(optional, 92)?, at(optional, 96)?),
        _ => return None,
    };
    if u32_at(count_at)? < 2 {
        return None;
    }
    let imports = u32_at(at(dirs_at, 8)?)?;
    if imports == 0 {
        return None;
    }

    let mut found = None;
    let mut descriptor = imports;
    loop {
        let name_at = u32_at(at(descriptor, 12)?)?;
        if name_at == 0 {
            break;
        }
        let mut name = Vec::new();
        let mut pos = Some(name_at);
        while let Some(b) = pos.and_then(&byte) {
            if b == 0 {
                break;
            }
            name.push(b.to_ascii_lowercase());
            pos = pos.and_then(|p| p.checked_add(1));
        }
        if name == b"msvcrt.dll" {
            return Some(Crt::Msvcrt);
        }
        if name == b"ucrtbase.dll" || name.starts_with(b"api-ms-win-crt-") {
            found = Some(Crt::Ucrt);
        }
        descriptor = at(descriptor, 20)?;
    }
    found
}

#[cfg(windows)]
mod windows {
    use super::{Crt, imported_crt};
    use std::ffi::CStr;
    use std::os::raw::{c_char, c_int, c_uint, c_void};
    use std::sync::OnceLock;

    type WriteFn = unsafe extern "C" fn(c_int, *const c_void, c_uint) -> c_int;
    type CloseFn = unsafe extern "C" fn(c_int) -> c_int;
    type FarProc = unsafe extern "system" fn() -> isize;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetModuleHandleA(name: *const c_char) -> *mut c_void;
        fn GetProcAddress(module: *mut c_void, name: *const c_char) -> Option<FarProc>;
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
            // The header page of a mapped image is always readable, and
            // SizeOfImage sits at the same offset for PE32 and PE32+.
            let header = |at: usize| (at < 4096).then(|| base.add(at).read());
            let nt = u32::from_le_bytes([header(0x3c)?, header(0x3d)?, header(0x3e)?, header(0x3f)?]);
            let size_at = (nt as usize).checked_add(24 + 56)?;
            let size = u32::from_le_bytes([
                header(size_at)?,
                header(size_at + 1)?,
                header(size_at + 2)?,
                header(size_at + 3)?,
            ]) as usize;
            // Read single bytes through the raw pointer: the parser touches
            // only headers, import descriptors and DLL names, which nothing
            // writes after loading. No reference to the whole image is made.
            imported_crt(|at| (at < size).then(|| base.add(at).read()))
        }
    }

    unsafe fn resolve(dll: &CStr) -> Option<Fns> {
        unsafe {
            let module = GetModuleHandleA(dll.as_ptr());
            if module.is_null() {
                return None;
            }
            let write = GetProcAddress(module, c"_write".as_ptr())?;
            let close = GetProcAddress(module, c"_close".as_ptr())?;
            Some(Fns {
                write: std::mem::transmute::<FarProc, WriteFn>(write),
                close: std::mem::transmute::<FarProc, CloseFn>(close),
            })
        }
    }

    /// `_write` and `_close` of the C runtime DLL Emacs imports. `None` when
    /// detection fails; the module's own `libc` is used then.
    fn emacs_fns() -> Option<&'static Fns> {
        static FNS: OnceLock<Option<Fns>> = OnceLock::new();
        FNS.get_or_init(|| unsafe {
            match emacs_crt()? {
                Crt::Msvcrt => resolve(c"msvcrt.dll"),
                Crt::Ucrt => resolve(c"ucrtbase.dll"),
            }
        })
        .as_ref()
    }

    pub(crate) unsafe fn write(fd: c_int, buf: *const c_void, len: c_uint) -> c_int {
        match emacs_fns() {
            Some(f) => unsafe { (f.write)(fd, buf, len) },
            None => unsafe { libc::write(fd, buf, len) },
        }
    }

    pub(crate) unsafe fn close(fd: c_int) -> c_int {
        match emacs_fns() {
            Some(f) => unsafe { (f.close)(fd) },
            None => unsafe { libc::close(fd) },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Crt, imported_crt};

    const NT: usize = 0x80;

    /// A minimal mapped PE image importing `dlls`; `pe32` selects the 32-bit
    /// optional header layout.
    fn image(dlls: &[&str], pe32: bool) -> Vec<u8> {
        let mut img = vec![0u8; 0x1000];
        img[0..2].copy_from_slice(b"MZ");
        img[0x3c..0x40].copy_from_slice(&(NT as u32).to_le_bytes());
        img[NT..NT + 4].copy_from_slice(b"PE\0\0");
        let opt = NT + 24;
        let (magic, count, dirs) = if pe32 { (0x10bu16, opt + 92, opt + 96) } else { (0x20b, opt + 108, opt + 112) };
        img[opt..opt + 2].copy_from_slice(&magic.to_le_bytes());
        img[count..count + 4].copy_from_slice(&16u32.to_le_bytes());
        let imports = 0x400;
        img[dirs + 8..dirs + 12].copy_from_slice(&(imports as u32).to_le_bytes());
        let mut name_at = 0x800;
        for (i, dll) in dlls.iter().enumerate() {
            let d = imports + 20 * i;
            img[d + 12..d + 16].copy_from_slice(&(name_at as u32).to_le_bytes());
            img[name_at..name_at + dll.len()].copy_from_slice(dll.as_bytes());
            name_at += dll.len() + 1;
        }
        img
    }

    fn crt(img: &[u8]) -> Option<Crt> {
        imported_crt(|at| img.get(at).copied())
    }

    #[test]
    fn msvcrt_is_found() {
        assert_eq!(crt(&image(&["KERNEL32.dll", "msvcrt.dll", "USER32.dll"], false)), Some(Crt::Msvcrt));
        assert_eq!(crt(&image(&["KERNEL32.dll", "msvcrt.dll"], true)), Some(Crt::Msvcrt));
    }

    #[test]
    fn ucrt_is_found() {
        let img = image(&["KERNEL32.dll", "api-ms-win-crt-stdio-l1-1-0.dll"], false);
        assert_eq!(crt(&img), Some(Crt::Ucrt));
        assert_eq!(crt(&image(&["ucrtbase.dll"], false)), Some(Crt::Ucrt));
    }

    #[test]
    fn msvcrt_wins_over_ucrt() {
        let img = image(&["api-ms-win-crt-heap-l1-1-0.dll", "MSVCRT.DLL"], false);
        assert_eq!(crt(&img), Some(Crt::Msvcrt));
    }

    #[test]
    fn unreadable_name_is_skipped() {
        let mut img = image(&["KERNEL32.dll", "msvcrt.dll"], false);
        img[0x400 + 12..0x400 + 16].copy_from_slice(&0xffff_fff0u32.to_le_bytes());
        assert_eq!(crt(&img), Some(Crt::Msvcrt));
    }

    #[test]
    fn no_crt_or_bad_image_is_none() {
        assert_eq!(crt(&image(&["KERNEL32.dll"], false)), None);
        assert_eq!(crt(&image(&[], false)), None);
        assert_eq!(crt(b"MZ"), None);
        let mut img = image(&["msvcrt.dll"], false);
        img[NT] = b'X';
        assert_eq!(crt(&img), None);
        // Too few data directories for an import table.
        let mut img = image(&["msvcrt.dll"], false);
        img[NT + 24 + 108..NT + 24 + 112].copy_from_slice(&1u32.to_le_bytes());
        assert_eq!(crt(&img), None);
        // A name that runs off the end of the image.
        let mut img = image(&["msvcrt.dll"], false);
        img.truncate(0x805);
        assert_eq!(crt(&img), None);
        // An import table that runs off the image before its zero descriptor.
        let mut img = image(&["ucrtbase.dll"], false);
        img.truncate(0x400 + 20 + 12);
        img[0x200..0x20d].copy_from_slice(b"ucrtbase.dll\0");
        img[0x400 + 12..0x400 + 16].copy_from_slice(&0x200u32.to_le_bytes());
        assert_eq!(crt(&img), None);
    }
}
