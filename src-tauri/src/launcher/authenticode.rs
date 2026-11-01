//! Authenticode check of a downloaded installer (Windows), before LRAM ever runs it.
//!
//! Two checks, both required:
//! 1. `WinVerifyTrust` (`WINTRUST_ACTION_GENERIC_VERIFY_V2`, no UI) on the file's own embedded
//!    signature: the file is unmodified and chains to a trusted root. Revocation is checked for
//!    the whole chain except the root (`WTD_REVOKE_WHOLECHAIN` +
//!    `WTD_REVOCATION_CHECK_CHAIN_EXCLUDE_ROOT`), online: the file was just downloaded, so the
//!    network is up; when the revocation servers can't be reached the check fails and the
//!    installer is not run (fail closed). A catalog-only signature has no embedded signer and
//!    fails step 2.
//! 2. The leaf certificate of the first signer *that `WinVerifyTrust` itself verified* (read
//!    from its state: `WTHelperProvDataFromStateData` → `WTHelperGetProvSignerFromChain` →
//!    `WTHelperGetProvCertFromChain(0)`, before the state is closed; never looked up again in the
//!    file's unauthenticated certificate bag) has the simple display name (its CN) exactly
//!    `signer`, via `CertGetNameStringW`.

use std::path::Path;

use windows::Win32::Foundation::{HWND, INVALID_HANDLE_VALUE};
use windows::Win32::Security::Cryptography::{CERT_CONTEXT, CERT_NAME_SIMPLE_DISPLAY_TYPE, CertGetNameStringW};
use windows::Win32::Security::WinTrust::{
    WINTRUST_ACTION_GENERIC_VERIFY_V2, WINTRUST_DATA, WINTRUST_DATA_0, WINTRUST_FILE_INFO, WTD_CHOICE_FILE,
    WTD_REVOCATION_CHECK_CHAIN_EXCLUDE_ROOT, WTD_REVOKE_WHOLECHAIN, WTD_STATEACTION_CLOSE, WTD_STATEACTION_VERIFY,
    WTD_UI_NONE, WTHelperGetProvCertFromChain, WTHelperGetProvSignerFromChain, WTHelperProvDataFromStateData,
    WinVerifyTrust,
};
use windows::core::{HSTRING, PCWSTR};

use crate::error::{AppError, Result};

/// The signer every Roblox installer and client carries.
pub const ROBLOX: &str = "Roblox Corporation";

/// Ok only when `path` has a valid, trusted embedded Authenticode signature whose signer is
/// exactly `signer` (see the module notes).
pub fn verify(path: &Path, signer: &str) -> Result<()> {
    let refused = |why: String| AppError::Invalid(tr!("The downloaded installer was not run: {why}.", why = why));
    let (status, name) = trust(path);
    if status != 0 {
        return Err(refused(tr!("its signature didn't check out ({status})", status = format!("0x{:08X}", status as u32))));
    }
    match name {
        Some(name) if name == signer => Ok(()),
        Some(name) => Err(refused(tr!("it is signed by \"{name}\", not {signer}", name = name, signer = signer))),
        None => Err(refused(tr!("its signer couldn't be read"))),
    }
}

/// `WinVerifyTrust`'s answer for the file's embedded signature (0 = trusted) and, when trusted,
/// the simple display name of the signer certificate it verified.
fn trust(path: &Path) -> (i32, Option<String>) {
    let wide = HSTRING::from(path.as_os_str());
    let mut file = WINTRUST_FILE_INFO {
        cbStruct: size_of::<WINTRUST_FILE_INFO>() as u32,
        pcwszFilePath: PCWSTR(wide.as_ptr()),
        ..Default::default()
    };
    let mut data = WINTRUST_DATA {
        cbStruct: size_of::<WINTRUST_DATA>() as u32,
        dwUIChoice: WTD_UI_NONE,
        fdwRevocationChecks: WTD_REVOKE_WHOLECHAIN,
        dwUnionChoice: WTD_CHOICE_FILE,
        Anonymous: WINTRUST_DATA_0 { pFile: &mut file },
        dwStateAction: WTD_STATEACTION_VERIFY,
        dwProvFlags: WTD_REVOCATION_CHECK_CHAIN_EXCLUDE_ROOT,
        ..Default::default()
    };
    let mut action = WINTRUST_ACTION_GENERIC_VERIFY_V2;
    // No interactive user: never any UI.
    let hwnd = HWND(INVALID_HANDLE_VALUE.0);
    // SAFETY: `data` points at `file`, whose path is the live NUL-terminated `wide`; all three
    // outlive both calls. The provider data, signer and certificate read in between belong to
    // the state the first call left in `data` and are used only before the second call frees it.
    unsafe {
        let status = WinVerifyTrust(hwnd, &mut action, (&raw mut data).cast());
        let name = (status == 0).then(|| verified_signer(&data)).flatten();
        data.dwStateAction = WTD_STATEACTION_CLOSE;
        WinVerifyTrust(hwnd, &mut action, (&raw mut data).cast());
        (status, name)
    }
}

/// The first signer's leaf certificate name from a successful `WTD_STATEACTION_VERIFY` state.
///
/// # Safety
/// `data.hWVTStateData` is the live state of a verify call, not yet closed.
unsafe fn verified_signer(data: &WINTRUST_DATA) -> Option<String> {
    // SAFETY: see the function's contract; each pointer is checked before it is read.
    unsafe {
        let prov = WTHelperProvDataFromStateData(data.hWVTStateData);
        if prov.is_null() {
            log::debug!("signer unreadable: no provider data");
            return None;
        }
        let sgnr = WTHelperGetProvSignerFromChain(prov, 0, false, 0);
        if sgnr.is_null() {
            log::debug!("signer unreadable: no signer in the chain");
            return None;
        }
        let cert = WTHelperGetProvCertFromChain(sgnr, 0);
        if cert.is_null() || (*cert).pCert.is_null() {
            log::debug!("signer unreadable: no certificate");
            return None;
        }
        name_of((*cert).pCert)
    }
}

/// # Safety
/// `cert` is a live certificate context.
unsafe fn name_of(cert: *const CERT_CONTEXT) -> Option<String> {
    // SAFETY: the first call asks for the length (with the NUL), the second writes at most
    // `name.len()` characters.
    let name = unsafe {
        let n = CertGetNameStringW(cert, CERT_NAME_SIMPLE_DISPLAY_TYPE, 0, None, None);
        let mut name = vec![0u16; n.max(1) as usize];
        let written = CertGetNameStringW(cert, CERT_NAME_SIMPLE_DISPLAY_TYPE, 0, None, Some(&mut name));
        name.truncate(written.saturating_sub(1) as usize);
        name
    };
    let name = Some(String::from_utf16_lossy(&name)).filter(|n| !n.is_empty());
    if name.is_none() {
        log::debug!("signer unreadable: certificate has no name");
    }
    name
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn existing(paths: &[PathBuf]) -> Option<PathBuf> {
        paths.iter().find(|p| p.is_file()).cloned()
    }

    #[test]
    fn unsigned_file_is_refused() {
        let dir = std::env::temp_dir().join(format!("roalt-sig-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&dir).unwrap();
        let exe = dir.join("unsigned.exe");
        std::fs::write(&exe, b"MZ not really a program").unwrap();
        assert_ne!(trust(&exe).0, 0);
        assert_eq!(trust(&exe).1, None);
        assert!(verify(&exe, ROBLOX).is_err());
        assert!(verify(&dir.join("missing.exe"), ROBLOX).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// explorer.exe: a trusted signature, but Microsoft's, so not run.
    #[test]
    fn system_exe_is_refused() {
        let explorer = PathBuf::from(std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into())).join("explorer.exe");
        assert!(explorer.is_file());
        assert_eq!(trust(&explorer), (0, Some("Microsoft Windows".into())));
        let e = verify(&explorer, ROBLOX).unwrap_err().to_string();
        assert!(e.contains("\"Microsoft Windows\", not Roblox Corporation"), "{e}");
    }

    /// An embedded-signed Microsoft program (the WebView2 runtime LRAM needs anyway) is
    /// trusted, but isn't Roblox's.
    #[test]
    fn signed_by_someone_else_is_refused() {
        let pf = std::env::var("ProgramFiles(x86)").unwrap_or_else(|_| r"C:\Program Files (x86)".into());
        let mut candidates = vec![PathBuf::from(&pf).join(r"Microsoft\Edge\Application\msedge.exe")];
        if let Ok(dirs) = std::fs::read_dir(PathBuf::from(&pf).join(r"Microsoft\EdgeWebView\Application")) {
            candidates.extend(dirs.flatten().map(|d| d.path().join("msedgewebview2.exe")));
        }
        let Some(exe) = existing(&candidates) else {
            eprintln!("no embedded-signed Microsoft program found; skipped");
            return;
        };
        assert_eq!(trust(&exe), (0, Some("Microsoft Corporation".into())), "{}", exe.display());
        let e = verify(&exe, ROBLOX).unwrap_err().to_string();
        assert!(e.contains("Microsoft Corporation"), "{e}");
    }

    /// Roblox's own installed client (read only, never run), when this PC has one.
    #[test]
    fn installed_roblox_client_verifies() {
        let local = std::env::var("LOCALAPPDATA").unwrap_or_default();
        let versions = PathBuf::from(local).join(r"Roblox\Versions");
        let found = std::fs::read_dir(versions)
            .into_iter()
            .flatten()
            .flatten()
            .map(|d| d.path().join("RobloxPlayerBeta.exe"))
            .find(|p| p.is_file());
        let Some(exe) = found else {
            eprintln!("Roblox isn't installed here; skipped");
            return;
        };
        verify(&exe, ROBLOX).unwrap();
    }
}
