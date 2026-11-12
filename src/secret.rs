//! Encrypts secrets at rest. On Windows this uses DPAPI bound to the current user,
//! so a copied settings.json is useless on another account or machine.

use anyhow::Result;
use base64::{Engine as _, engine::general_purpose::STANDARD};

#[cfg(windows)]
pub fn protect(plain: &str) -> Result<String> {
    use windows::Win32::{
        Foundation::{HLOCAL, LocalFree},
        Security::Cryptography::{CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptProtectData},
    };
    let bytes = plain.as_bytes();
    let input = CRYPT_INTEGER_BLOB {
        cbData: bytes.len() as u32,
        pbData: bytes.as_ptr() as *mut u8,
    };
    let mut output = CRYPT_INTEGER_BLOB::default();
    unsafe {
        CryptProtectData(
            &input,
            None,
            None,
            None,
            None,
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut output,
        )?;
        let sealed = std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec();
        let _ = LocalFree(Some(HLOCAL(output.pbData.cast())));
        Ok(format!("dpapi:{}", STANDARD.encode(sealed)))
    }
}

#[cfg(windows)]
pub fn unprotect(sealed: &str) -> Result<String> {
    use windows::Win32::{
        Foundation::{HLOCAL, LocalFree},
        Security::Cryptography::{
            CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptUnprotectData,
        },
    };
    let Some(encoded) = sealed.strip_prefix("dpapi:") else {
        return unprotect_plain(sealed);
    };
    let mut bytes = STANDARD.decode(encoded)?;
    let input = CRYPT_INTEGER_BLOB {
        cbData: bytes.len() as u32,
        pbData: bytes.as_mut_ptr(),
    };
    let mut output = CRYPT_INTEGER_BLOB::default();
    unsafe {
        CryptUnprotectData(
            &input,
            None,
            None,
            None,
            None,
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut output,
        )?;
        let plain = std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec();
        let _ = LocalFree(Some(HLOCAL(output.pbData.cast())));
        Ok(String::from_utf8(plain)?)
    }
}

/// Non-Windows builds (tests, CI) have no DPAPI; obfuscate only.
#[cfg(not(windows))]
pub fn protect(plain: &str) -> Result<String> {
    Ok(format!("b64:{}", STANDARD.encode(plain)))
}

#[cfg(not(windows))]
pub fn unprotect(sealed: &str) -> Result<String> {
    unprotect_plain(sealed)
}

fn unprotect_plain(sealed: &str) -> Result<String> {
    let encoded = sealed
        .strip_prefix("b64:")
        .ok_or_else(|| anyhow::anyhow!("未知的密钥格式"))?;
    Ok(String::from_utf8(STANDARD.decode(encoded)?)?)
}

#[cfg(test)]
mod tests {
    #[test]
    fn roundtrip() {
        let sealed = super::protect("sk-test-123").unwrap();
        assert!(!sealed.contains("sk-test-123"));
        assert_eq!(super::unprotect(&sealed).unwrap(), "sk-test-123");
    }
}
