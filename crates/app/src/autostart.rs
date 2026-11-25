//! Windows `HKCU\Software\Microsoft\Windows\CurrentVersion\Run` autostart toggle via direct `advapi32` FFI.

use std::path::Path;

pub const RUN_SUBKEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
pub const VALUE_NAME: &str = "QuotaGlass";

/// Formats and validates an executable path for a Windows Run registry `REG_SZ` entry.
/// Quotes the path cleanly and rejects relative paths, embedded quotes, control characters,
/// non-UTF-8 paths, and trailing path separators (which escape a closing quote on Windows).
pub fn format_run_value(exe: &Path) -> Result<String, &'static str> {
    if !exe.is_absolute() {
        return Err("Executable path must be absolute.");
    }
    let raw = exe.to_str().ok_or("Executable path is not valid UTF-8.")?;
    if raw.is_empty() {
        return Err("Executable path cannot be empty.");
    }
    if raw.contains('"') || raw.chars().any(char::is_control) {
        return Err("Executable path contains unsafe characters.");
    }
    if raw.ends_with('\\') || raw.ends_with('/') {
        return Err("Executable path cannot end with a path separator.");
    }
    Ok(format!("\"{raw}\""))
}

#[cfg(windows)]
mod imp {
    use super::*;
    use std::ffi::c_void;

    type Hkey = *mut c_void;

    // ((HKEY)(ULONG_PTR)((LONG)0x80000001))
    const HKEY_CURRENT_USER: Hkey = (-2147483647_i32 as isize) as Hkey;
    const KEY_QUERY_VALUE: u32 = 0x0001;
    const KEY_SET_VALUE: u32 = 0x0002;
    const REG_SZ: u32 = 1;
    const ERROR_SUCCESS: i32 = 0;
    const ERROR_FILE_NOT_FOUND: i32 = 2;

    #[link(name = "advapi32")]
    extern "system" {
        fn RegOpenKeyExW(
            hkey: Hkey,
            subkey: *const u16,
            options: u32,
            sam: u32,
            result: *mut Hkey,
        ) -> i32;
        fn RegQueryValueExW(
            hkey: Hkey,
            value_name: *const u16,
            reserved: *const u32,
            reg_type: *mut u32,
            data: *mut u8,
            cb_data: *mut u32,
        ) -> i32;
        fn RegSetValueExW(
            hkey: Hkey,
            value_name: *const u16,
            reserved: u32,
            reg_type: u32,
            data: *const u8,
            cb_data: u32,
        ) -> i32;
        fn RegDeleteValueW(hkey: Hkey, value_name: *const u16) -> i32;
        fn RegCloseKey(hkey: Hkey) -> i32;
    }

    struct RegKey(Hkey);

    impl Drop for RegKey {
        fn drop(&mut self) {
            if !self.0.is_null() {
                // SAFETY: `self.0` is an open registry key obtained from `RegOpenKeyExW`.
                unsafe {
                    RegCloseKey(self.0);
                }
            }
        }
    }

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn open_run_key(sam: u32) -> Result<Option<RegKey>, &'static str> {
        let subkey = wide(RUN_SUBKEY);
        let mut hkey: Hkey = std::ptr::null_mut();
        // SAFETY: `subkey` is null-terminated UTF-16 and `hkey` is a valid out-pointer.
        let err = unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, subkey.as_ptr(), 0, sam, &mut hkey) };
        if err == ERROR_FILE_NOT_FOUND {
            return Ok(None);
        }
        if err != ERROR_SUCCESS || hkey.is_null() {
            return Err("Could not open Windows startup registry key.");
        }
        Ok(Some(RegKey(hkey)))
    }

    pub fn is_enabled() -> Result<bool, &'static str> {
        let Some(key) = open_run_key(KEY_QUERY_VALUE)? else {
            return Ok(false);
        };
        let name = wide(VALUE_NAME);
        let mut reg_type: u32 = 0;
        let mut cb: u32 = 0;
        // SAFETY: querying size with `data = null` is documented by `RegQueryValueExW`.
        let err = unsafe {
            RegQueryValueExW(
                key.0,
                name.as_ptr(),
                std::ptr::null(),
                &mut reg_type,
                std::ptr::null_mut(),
                &mut cb,
            )
        };
        if err == ERROR_FILE_NOT_FOUND {
            return Ok(false);
        }
        if err != ERROR_SUCCESS {
            return Err("Could not read Windows startup setting.");
        }
        Ok(reg_type == REG_SZ && cb > 2)
    }

    pub fn set_enabled_for(enabled: bool, exe: &Path) -> Result<(), &'static str> {
        let Some(key) = open_run_key(KEY_SET_VALUE)? else {
            if !enabled {
                return Ok(());
            }
            return Err("Windows startup registry key is missing.");
        };
        let name = wide(VALUE_NAME);
        if enabled {
            let formatted = format_run_value(exe)?;
            let data = wide(&formatted);
            let byte_len = u32::try_from(std::mem::size_of_val(data.as_slice()))
                .map_err(|_| "Path too long.")?;
            // SAFETY: `key.0` is opened with `KEY_SET_VALUE`, `name` and `data` are null-terminated UTF-16,
            // and `byte_len` equals the exact byte size of `data`.
            let err = unsafe {
                RegSetValueExW(
                    key.0,
                    name.as_ptr(),
                    0,
                    REG_SZ,
                    data.as_ptr().cast::<u8>(),
                    byte_len,
                )
            };
            if err != ERROR_SUCCESS {
                return Err("Could not save start with Windows.");
            }
        } else {
            // SAFETY: `key.0` is opened with `KEY_SET_VALUE` and `name` is null-terminated UTF-16.
            let err = unsafe { RegDeleteValueW(key.0, name.as_ptr()) };
            if err != ERROR_SUCCESS && err != ERROR_FILE_NOT_FOUND {
                return Err("Could not disable start with Windows.");
            }
        }
        Ok(())
    }

    pub fn set_enabled(enabled: bool) -> Result<(), &'static str> {
        if enabled {
            let exe = std::env::current_exe().map_err(|_| "Could not locate app executable.")?;
            set_enabled_for(true, &exe)
        } else {
            set_enabled_for(false, Path::new(r"C:\unused"))
        }
    }
}

#[cfg(not(windows))]
mod imp {
    use super::*;

    pub fn is_enabled() -> Result<bool, &'static str> {
        Ok(false)
    }

    pub fn set_enabled_for(_enabled: bool, _exe: &Path) -> Result<(), &'static str> {
        Err("Autostart is only supported on Windows.")
    }

    pub fn set_enabled(_enabled: bool) -> Result<(), &'static str> {
        Err("Autostart is only supported on Windows.")
    }
}

pub use imp::{is_enabled, set_enabled, set_enabled_for};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_run_value_quotes_valid_absolute_paths() {
        let p = Path::new(r"C:\Program Files\QuotaGlass\quotaglass.exe");
        assert_eq!(
            format_run_value(p),
            Ok(r#""C:\Program Files\QuotaGlass\quotaglass.exe""#.to_string())
        );
    }

    #[test]
    fn format_run_value_rejects_relative_quotes_control_and_trailing_sep() {
        assert!(format_run_value(Path::new("quotaglass.exe")).is_err());
        assert!(format_run_value(Path::new(r"sub\quotaglass.exe")).is_err());
        assert!(format_run_value(Path::new("")).is_err());
        assert!(format_run_value(Path::new(r#"C:\a"b\quotaglass.exe"#)).is_err());
        assert!(format_run_value(Path::new("C:\\a\0b\\app.exe")).is_err());
        assert!(format_run_value(Path::new("C:\\a\nb\\app.exe")).is_err());
        assert!(format_run_value(Path::new("C:\\a\rb\\app.exe")).is_err());
        assert!(format_run_value(Path::new("C:\\a\tb\\app.exe")).is_err());
        assert!(format_run_value(Path::new(r"C:\Program Files\QuotaGlass\")).is_err());
    }
}
