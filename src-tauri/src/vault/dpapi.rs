//! Windows DPAPI (current user) with an app-specific entropy.

use windows::Win32::Foundation::{HLOCAL, LocalFree};
use windows::Win32::Security::Cryptography::{
    CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptProtectData, CryptUnprotectData,
};
use windows::core::PCWSTR;
use zeroize::{Zeroize, Zeroizing};

use crate::error::{AppError, Result};

/// DPAPI entropy of every vault wrap. Frozen at the original product name, like the database's:
/// changing it would strand the data key of every existing vault.
pub const ENTROPY: &[u8] = b"RoALT/vault/v1";

pub fn protect(data: &[u8]) -> Result<Vec<u8>> {
    protect_with(data, ENTROPY)
}

/// `protect` with another entropy, so a blob made for one purpose can't stand in for another.
pub fn protect_with(data: &[u8], entropy: &[u8]) -> Result<Vec<u8>> {
    let (input, entropy) = (blob(data)?, blob(entropy)?);
    let mut out = CRYPT_INTEGER_BLOB::default();
    // SAFETY: the input blobs point at live slices DPAPI only reads; DPAPI fills `out`.
    unsafe {
        CryptProtectData(&input, PCWSTR::null(), Some(&entropy), None, None, CRYPTPROTECT_UI_FORBIDDEN, &mut out)
    }
    .map_err(|e| {
        log::error!("DPAPI protect failed: {e}");
        AppError::Crypto
    })?;
    Ok(take(out).to_vec())
}

pub fn unprotect(blob_in: &[u8]) -> Result<Zeroizing<Vec<u8>>> {
    unprotect_with(blob_in, ENTROPY)
}

/// `unprotect` with another app's entropy (an import from it).
pub fn unprotect_with(blob_in: &[u8], entropy: &[u8]) -> Result<Zeroizing<Vec<u8>>> {
    let (input, entropy) = (blob(blob_in)?, blob(entropy)?);
    let mut out = CRYPT_INTEGER_BLOB::default();
    // SAFETY: as in `protect`.
    unsafe { CryptUnprotectData(&input, None, Some(&entropy), None, None, CRYPTPROTECT_UI_FORBIDDEN, &mut out) }
        .map_err(|e| {
            log::error!("DPAPI unprotect failed: {e}");
            AppError::Crypto
        })?;
    Ok(take(out))
}

fn blob(data: &[u8]) -> Result<CRYPT_INTEGER_BLOB> {
    let len = u32::try_from(data.len()).map_err(|_| AppError::Crypto)?;
    Ok(CRYPT_INTEGER_BLOB { cbData: len, pbData: data.as_ptr().cast_mut() })
}

/// Copies DPAPI's output, wipes the original and frees it.
fn take(out: CRYPT_INTEGER_BLOB) -> Zeroizing<Vec<u8>> {
    if out.pbData.is_null() {
        return Zeroizing::new(Vec::new());
    }
    // SAFETY: on success DPAPI returns `cbData` bytes at `pbData`, allocated with LocalAlloc.
    unsafe {
        let bytes = std::slice::from_raw_parts_mut(out.pbData, out.cbData as usize);
        let copy = Zeroizing::new(bytes.to_vec());
        bytes.zeroize();
        LocalFree(Some(HLOCAL(out.pbData.cast())));
        copy
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dpapi_round_trip() {
        let wrapped = protect(b"data key").unwrap();
        assert_ne!(wrapped.as_slice(), b"data key");
        assert_eq!(unprotect(&wrapped).unwrap().as_slice(), b"data key");
        assert!(matches!(unprotect(b"not a dpapi blob"), Err(AppError::Crypto)));
    }
}
