use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Nonce};
use argon2::{Algorithm, Argon2, Params, Version};
use zeroize::Zeroizing;

use super::{Kdf, Secret};
use crate::error::{AppError, Result};

pub const DEFAULT_M_KIB: u32 = 64 * 1024;
pub const DEFAULT_T: u32 = 3;
pub const DEFAULT_P: u32 = 4;

const NONCE_LEN: usize = 12;

pub fn random_bytes<const N: usize>() -> [u8; N] {
    let mut b = [0u8; N];
    getrandom::fill(&mut b).expect("OS random number generator failed");
    b
}

pub fn new_kdf() -> Kdf {
    let (m_kib, t, p) = COST;
    Kdf { salt: random_bytes::<16>().to_vec(), m_kib, t, p }
}

#[cfg(not(test))]
const COST: (u32, u32, u32) = (DEFAULT_M_KIB, DEFAULT_T, DEFAULT_P);
#[cfg(test)]
const COST: (u32, u32, u32) = (1024, 1, 1);

pub fn new_recovery_kdf() -> Kdf {
    let (m_kib, t, p) = RECOVERY_COST;
    Kdf { salt: random_bytes::<16>().to_vec(), m_kib, t, p }
}

#[cfg(not(test))]
const RECOVERY_COST: (u32, u32, u32) = (256 * 1024, 3, 4);
#[cfg(test)]
const RECOVERY_COST: (u32, u32, u32) = COST;

const CODE_ALPHABET: &[u8; 31] = b"23456789ABCDEFGHJKMNPQRSTUVWXYZ";
const CODE_LEN: usize = 9;
const CODE_BYTE_LIMIT: u8 = 248;

fn dashed(code: &str) -> Secret {
    Secret::new(format!("{}-{}-{}", &code[..3], &code[3..6], &code[6..]))
}

pub fn recovery_code() -> Secret {
    let mut code = Zeroizing::new(String::with_capacity(CODE_LEN));
    while code.len() < CODE_LEN {
        let [b] = random_bytes::<1>();
        if b < CODE_BYTE_LIMIT {
            code.push(CODE_ALPHABET[b as usize % CODE_ALPHABET.len()] as char);
        }
    }
    dashed(&code)
}

pub fn normalize_code(raw: &str) -> Option<Secret> {
    let code: Zeroizing<String> = Zeroizing::new(
        raw.chars().filter(|c| *c != '-' && !c.is_whitespace()).map(|c| c.to_ascii_uppercase()).collect(),
    );
    (code.len() == CODE_LEN && code.bytes().all(|b| CODE_ALPHABET.contains(&b))).then(|| dashed(&code))
}

pub fn derive_key(password: &str, kdf: &Kdf) -> Result<Zeroizing<[u8; 32]>> {
    let params = Params::new(kdf.m_kib, kdf.t, kdf.p, Some(32)).map_err(|e| {
        log::warn!("bad argon2 parameters (m {} KiB, t {}, p {}): {e}", kdf.m_kib, kdf.t, kdf.p);
        AppError::Crypto
    })?;
    let mut key = Zeroizing::new([0u8; 32]);
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
        .hash_password_into(password.as_bytes(), &kdf.salt, key.as_mut())
        .map_err(|e| {
            log::warn!("argon2 hashing failed: {e}");
            AppError::Crypto
        })?;
    Ok(key)
}

pub fn seal(key: &[u8; 32], aad: &[u8], plaintext: &[u8]) -> Result<Vec<u8>> {
    let nonce = random_bytes::<NONCE_LEN>();
    let ct = Aes256Gcm::new(key.into())
        .encrypt(&Nonce::from(nonce), Payload { msg: plaintext, aad })
        .map_err(|_| AppError::Crypto)?;
    let mut out = Vec::with_capacity(NONCE_LEN + ct.len());
    out.extend_from_slice(&nonce);
    out.extend_from_slice(&ct);
    Ok(out)
}

// -- Main func
pub fn open(key: &[u8; 32], aad: &[u8], blob: &[u8]) -> Result<Zeroizing<Vec<u8>>> {
    let (nonce, ct) = blob.split_first_chunk::<NONCE_LEN>().ok_or(AppError::Crypto)?;
    Aes256Gcm::new(key.into())
        .decrypt(&Nonce::from(*nonce), Payload { msg: ct, aad })
        .map(Zeroizing::new)
        .map_err(|_| AppError::Crypto)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seal_open_round_trip() {
        let key = random_bytes::<32>();
        let blob = seal(&key, b"aad", b"secret").unwrap();
        assert_eq!(open(&key, b"aad", &blob).unwrap().as_slice(), b"secret");
        assert_ne!(seal(&key, b"aad", b"secret").unwrap(), blob, "nonce must be fresh");
    }

    #[test]
    fn open_rejects_wrong_key_aad_or_blob() {
        let key = random_bytes::<32>();
        let blob = seal(&key, b"aad", b"secret").unwrap();
        assert!(matches!(open(&random_bytes::<32>(), b"aad", &blob), Err(AppError::Crypto)));
        assert!(matches!(open(&key, b"other", &blob), Err(AppError::Crypto)));
        let mut tampered = blob.clone();
        *tampered.last_mut().unwrap() ^= 1;
        assert!(matches!(open(&key, b"aad", &tampered), Err(AppError::Crypto)));
        assert!(matches!(open(&key, b"aad", &blob[..5]), Err(AppError::Crypto)));
    }

    #[test]
    fn derive_key_is_deterministic_per_password() {
        let kdf = new_kdf();
        let a = derive_key("hunter22", &kdf).unwrap();
        assert_eq!(*a, *derive_key("hunter22", &kdf).unwrap());
        assert_ne!(*a, *derive_key("hunter23", &kdf).unwrap());
        assert!(Params::new(DEFAULT_M_KIB, DEFAULT_T, DEFAULT_P, Some(32)).is_ok());
    }

    #[test]
    fn recovery_codes_have_the_format_and_alphabet() {
        assert_eq!(CODE_ALPHABET.len(), 31);
        assert!(!CODE_ALPHABET.iter().any(|b| b"0O1IL".contains(b)));
        assert_eq!(CODE_BYTE_LIMIT as usize % CODE_ALPHABET.len(), 0);
        assert!(Params::new(256 * 1024, 3, 4, Some(32)).is_ok());
        let mut seen = std::collections::HashSet::new();
        for _ in 0..200 {
            let code = recovery_code();
            let c = code.expose();
            assert_eq!(c.len(), 11);
            for (i, b) in c.bytes().enumerate() {
                assert!(if i == 3 || i == 7 { b == b'-' } else { CODE_ALPHABET.contains(&b) }, "{c}");
            }
            assert_eq!(normalize_code(c).unwrap().expose(), c);
            seen.insert(c.to_owned());
        }
        assert!(seen.len() > 190, "codes must differ");
    }

    #[test]
    fn normalize_ignores_case_dashes_and_spaces() {
        for raw in ["abc-def-234", "ABCDEF234", " abc def 234\r\n", "a-b-c-d-e-f-2-3-4"] {
            assert_eq!(normalize_code(raw).unwrap().expose(), "ABC-DEF-234", "{raw:?}");
        }
        for raw in ["", "ABC-DEF-23", "ABC-DEF-2345", "ABC-DEF-230", "OBC-DEF-234", "ILC-DEF-234", "ÄBC-DEF-234", "ABC_DEF_234", "Hello world, this is text"] {
            assert!(normalize_code(raw).is_none(), "{raw:?}");
        }
    }
}
