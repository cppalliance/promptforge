//! The installer's STT choice and the tray's Launch at Login entry, both
//! recorded in the registry. Raw Win32 FFI has no safe wrapper in the tree.

use windows_sys::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS};
use windows_sys::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, KEY_READ, KEY_SET_VALUE, REG_DWORD, REG_SZ, RegCloseKey,
    RegDeleteValueW, RegOpenKeyExW, RegQueryValueExW, RegSetValueExW,
};

/// The autostart entry the tray's Launch at Login check item manages:
/// `HKCU\...\CurrentVersion\Run\PromptForgeGateway`.
const RUN_SUBKEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";
/// The Run-key value name holding the gateway's login command line.
const RUN_VALUE: &str = "PromptForgeGateway";

/// Opens one of this module's subkeys, returning the live handle. The
/// caller closes it with [`close_key`] on every path.
///
/// # Errors
/// Returns the registry status code when the key cannot be opened.
fn open_key(subkey: &str, access: u32) -> Result<HKEY, u32> {
    let subkey: Vec<u16> = subkey.encode_utf16().chain(Some(0)).collect();
    let mut key: HKEY = std::ptr::null_mut();
    // SAFETY: `subkey` is a valid null-terminated UTF-16 string and
    // `key` is valid for one HKEY write; on success `key` holds a live
    // handle that `close_key` releases exactly once.
    let status =
        unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, subkey.as_ptr(), 0, access, &raw mut key) };
    if status != ERROR_SUCCESS {
        return Err(status);
    }
    Ok(key)
}

/// Closes a handle from [`open_key`].
///
/// # Safety
/// `key` must be a live handle from a successful [`open_key`] that has
/// not been closed yet; it is closed exactly once here.
fn close_key(key: HKEY) {
    // SAFETY: per the contract above, `key` is live and closed once.
    unsafe {
        RegCloseKey(key);
    }
}

/// The registry error code as an `io::Error`, for the write shims whose
/// callers report failures.
fn status_error(status: u32) -> std::io::Error {
    std::io::Error::from_raw_os_error(status.cast_signed())
}

/// Reads `HKCU\Software\PromptForge\PromptForge\InstallSTT` as a DWORD,
/// or `None` when the key or value is absent, has another type, or
/// cannot be read.
pub(super) fn install_stt_dword() -> Option<u32> {
    let key = open_key("Software\\PromptForge\\PromptForge", KEY_READ).ok()?;
    let value: Vec<u16> = "InstallSTT".encode_utf16().chain(Some(0)).collect();
    let mut data = 0u32;
    let mut kind = 0u32;
    // A REG_DWORD is 4 bytes; the query fails rather than truncates when
    // the buffer is too small.
    let mut len = 4u32;
    // SAFETY: `key` is the live handle opened above; `value` is a valid
    // null-terminated UTF-16 string; `data` is valid for `len` bytes of
    // writes and `kind` and `len` are valid for one u32 write each.
    let status = unsafe {
        RegQueryValueExW(
            key,
            value.as_ptr(),
            std::ptr::null(),
            &raw mut kind,
            std::ptr::from_mut(&mut data).cast::<u8>(),
            &raw mut len,
        )
    };
    close_key(key);
    if status != ERROR_SUCCESS || kind != REG_DWORD || len != 4 {
        return None;
    }
    Some(data)
}

/// Reads the Launch at Login command line from the Run key, or `None`
/// when the value is absent, is not a string, or cannot be read. The
/// tray's check item derives its state from this read alone - never
/// from local config - because the user can revoke the entry
/// externally.
pub(crate) fn read_run_value() -> Option<String> {
    let key = open_key(RUN_SUBKEY, KEY_READ).ok()?;
    let value: Vec<u16> = RUN_VALUE.encode_utf16().chain(Some(0)).collect();
    let mut kind = 0u32;
    let mut len = 0u32;
    // SAFETY: `key` is live; `value` is a valid null-terminated UTF-16
    // string; the null buffer queries the required size into `len`.
    let status = unsafe {
        RegQueryValueExW(
            key,
            value.as_ptr(),
            std::ptr::null(),
            &raw mut kind,
            std::ptr::null_mut(),
            &raw mut len,
        )
    };
    if status != ERROR_SUCCESS || kind != REG_SZ || len == 0 {
        close_key(key);
        return None;
    }
    let mut buffer = vec![0u16; (len as usize).div_ceil(2) + 1];
    // SAFETY: `key` is live; `buffer` is valid for `len` bytes of
    // writes plus a spare terminator word, and `kind`/`len` are valid
    // for one u32 write each.
    let status = unsafe {
        RegQueryValueExW(
            key,
            value.as_ptr(),
            std::ptr::null(),
            &raw mut kind,
            buffer.as_mut_ptr().cast::<u8>(),
            &raw mut len,
        )
    };
    close_key(key);
    if status != ERROR_SUCCESS {
        return None;
    }
    let words = (len as usize) / 2;
    String::from_utf16(&buffer[..words])
        .ok()
        .map(|text| text.trim_end_matches('\0').to_owned())
}

/// Writes the Launch at Login command line to the Run key, creating the
/// value when absent.
///
/// # Errors
/// Returns the registry error when the key cannot be opened or the
/// value cannot be written.
pub(crate) fn write_run_value(command: &str) -> std::io::Result<()> {
    let key = open_key(RUN_SUBKEY, KEY_SET_VALUE).map_err(status_error)?;
    let value: Vec<u16> = RUN_VALUE.encode_utf16().chain(Some(0)).collect();
    let wide: Vec<u16> = command.encode_utf16().chain(Some(0)).collect();
    let len = u32::try_from(wide.len() * 2).unwrap_or(u32::MAX);
    // SAFETY: `key` is live; `value` and `wide` are valid
    // null-terminated UTF-16 strings; `wide` is readable for `len`
    // bytes.
    let status = unsafe {
        RegSetValueExW(
            key,
            value.as_ptr(),
            0,
            REG_SZ,
            wide.as_ptr().cast::<u8>(),
            len,
        )
    };
    close_key(key);
    if status != ERROR_SUCCESS {
        return Err(status_error(status));
    }
    Ok(())
}

/// Deletes the Launch at Login value from the Run key. An absent value
/// is a success: delete is idempotent.
///
/// # Errors
/// Returns the registry error when the key cannot be opened or the
/// value cannot be deleted.
pub(crate) fn delete_run_value() -> std::io::Result<()> {
    let key = open_key(RUN_SUBKEY, KEY_SET_VALUE).map_err(status_error)?;
    let value: Vec<u16> = RUN_VALUE.encode_utf16().chain(Some(0)).collect();
    // SAFETY: `key` is live and `value` is a valid null-terminated
    // UTF-16 string.
    let status = unsafe { RegDeleteValueW(key, value.as_ptr()) };
    close_key(key);
    if status != ERROR_SUCCESS && status != ERROR_FILE_NOT_FOUND {
        return Err(status_error(status));
    }
    Ok(())
}
