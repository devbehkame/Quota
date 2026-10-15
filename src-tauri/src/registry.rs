//! Small helpers for values under HKEY_CURRENT_USER.

use std::ffi::c_void;
use std::ptr::null_mut;

use windows_sys::Win32::Foundation::ERROR_SUCCESS;
use windows_sys::Win32::System::Registry::{
    RegDeleteKeyValueW, RegGetValueW, RegSetKeyValueW, HKEY_CURRENT_USER, REG_SZ, RRF_RT_REG_SZ,
};

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

/// A REG_SZ value under HKEY_CURRENT_USER.
pub fn get_string(key: &str, name: &str) -> Option<String> {
    let (key, name) = (wide(key), wide(name));
    let mut buf = vec![0u16; 1024];
    let mut size = (buf.len() * 2) as u32;
    // SAFETY: `buf` is writable for `size` bytes; key and name are NUL-terminated.
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            key.as_ptr(),
            name.as_ptr(),
            RRF_RT_REG_SZ,
            null_mut(),
            buf.as_mut_ptr() as *mut c_void,
            &mut size,
        )
    };
    if status != ERROR_SUCCESS {
        return None;
    }
    let len = (size as usize / 2).min(buf.len());
    let s = String::from_utf16_lossy(&buf[..len]).trim_end_matches('\0').to_string();
    (!s.is_empty()).then_some(s)
}

/// Writes a REG_SZ value under HKEY_CURRENT_USER; the key must already exist.
pub fn set_string(key: &str, name: &str, value: &str) -> bool {
    let (key, name, value) = (wide(key), wide(name), wide(value));
    // SAFETY: all three strings are NUL-terminated; the size includes the NUL.
    let status = unsafe {
        RegSetKeyValueW(
            HKEY_CURRENT_USER,
            key.as_ptr(),
            name.as_ptr(),
            REG_SZ,
            value.as_ptr() as *const c_void,
            (value.len() * 2) as u32,
        )
    };
    status == ERROR_SUCCESS
}

pub fn delete_value(key: &str, name: &str) {
    let (key, name) = (wide(key), wide(name));
    // SAFETY: both strings are NUL-terminated; only this one value is removed.
    unsafe {
        RegDeleteKeyValueW(HKEY_CURRENT_USER, key.as_ptr(), name.as_ptr());
    }
}
