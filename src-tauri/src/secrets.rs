//! API keys live in the Windows Credential Manager (DPAPI-protected, per Windows user).
//! They are never written to disk by the app and never sent to the UI.

use keyring::Entry;
use zeroize::Zeroizing;

const SERVICE: &str = "com.usagecheck.app";

fn entry(account_id: &str) -> Result<Entry, String> {
    Entry::new(SERVICE, account_id).map_err(|e| format!("Credential store unavailable: {e}"))
}

pub fn set(account_id: &str, secret: &str) -> Result<(), String> {
    entry(account_id)?
        .set_password(secret)
        .map_err(|e| format!("Could not save the key to Windows Credential Manager: {e}"))
}

pub fn get(account_id: &str) -> Result<Option<Zeroizing<String>>, String> {
    match entry(account_id)?.get_password() {
        Ok(s) => Ok(Some(Zeroizing::new(s))),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(format!("Could not read the key from Windows Credential Manager: {e}")),
    }
}

pub fn exists(account_id: &str) -> bool {
    matches!(get(account_id), Ok(Some(_)))
}

pub fn delete(account_id: &str) -> Result<(), String> {
    match entry(account_id)?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(format!("Could not remove the key from Windows Credential Manager: {e}")),
    }
}

/// Basic hygiene on pasted keys: trimmed, bounded, printable ASCII only.
pub fn validate(secret: &str) -> Result<&str, String> {
    let s = secret.trim();
    if s.is_empty() {
        return Err("The key is empty.".into());
    }
    if s.len() > 1024 {
        return Err("That key is too long to be valid.".into());
    }
    if !s.chars().all(|c| c.is_ascii_graphic()) {
        return Err("The key contains spaces or unexpected characters. Paste just the key.".into());
    }
    Ok(s)
}
