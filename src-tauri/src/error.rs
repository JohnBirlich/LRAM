use serde::{Serialize, Serializer};
use ts_rs::TS;

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("The vault is locked.")]
    Locked,
    #[error("Wrong password.")]
    WrongPassword,
    #[error("This cookie is not valid or has expired. Sign in again.")]
    CookieInvalid,
    #[error("Roblox wants a human check for this account. Sign in to it again with Add account > Sign in with Roblox.")]
    Challenge,
    #[error("Roblox is limiting requests. Try again in {0} s.")]
    RateLimited(u64),
    #[error("Too many wrong passwords. Try again in {0} s.")]
    TooManyAttempts(u64),
    #[error("Roblox now requires a bound session for this account.")]
    BoundSessionRequired,
    #[error("Roblox answered unexpectedly ({0}).")]
    Roblox(String),
    #[error("Network error: {0}")]
    Network(String),
    #[error("Not found.")]
    NotFound,
    #[error("Cancelled.")]
    Cancelled,
    #[error("{0}")]
    Invalid(String),
    #[error("{0}")]
    Wait(String),
    #[error("Storage error: {0}")]
    Storage(String),
    #[error("Encryption failed.")]
    Crypto,
    #[error("{0}")]
    Internal(String),
}

#[cfg(windows)]
// -- Main func
pub fn windows_error(e: &windows::core::Error) -> String {
    let code = e.code().0 as u32;
    match known_win32(e.code()) {
        Some(what) => format!("{what} (0x{code:08X})"),
        None => format!("Windows error 0x{code:08X}"),
    }
}

#[cfg(windows)]
fn known_win32(code: windows::core::HRESULT) -> Option<&'static str> {
    use windows::Win32::Foundation::{
        ERROR_ACCESS_DENIED, ERROR_INVALID_HANDLE, ERROR_INVALID_PARAMETER, ERROR_NOT_ENOUGH_MEMORY, ERROR_PRIVILEGE_NOT_HELD,
        ERROR_SHARING_VIOLATION,
    };
    let known = [
        (ERROR_ACCESS_DENIED, "access denied"),
        (ERROR_INVALID_HANDLE, "invalid handle"),
        (ERROR_NOT_ENOUGH_MEMORY, "not enough memory"),
        (ERROR_SHARING_VIOLATION, "the file is in use"),
        (ERROR_INVALID_PARAMETER, "the parameter is incorrect"),
        (ERROR_PRIVILEGE_NOT_HELD, "the privilege is not held"),
    ];
    known.iter().find(|(c, _)| code == c.to_hresult()).map(|(_, what)| *what)
}

#[cfg(all(test, windows))]
mod windows_error_tests {
    use windows::Win32::Foundation::{E_FAIL, ERROR_ACCESS_DENIED, ERROR_INVALID_PARAMETER, ERROR_NOT_ENOUGH_MEMORY};

    use super::*;

    #[test]
    fn windows_errors_are_english() {
        assert_eq!(known_win32(ERROR_ACCESS_DENIED.to_hresult()), Some("access denied"));
        assert_eq!(known_win32(ERROR_INVALID_PARAMETER.to_hresult()), Some("the parameter is incorrect"));
        assert_eq!(known_win32(ERROR_NOT_ENOUGH_MEMORY.to_hresult()), Some("not enough memory"));
        assert_eq!(known_win32(E_FAIL), None);
    }
}

impl AppError {
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Locked => "locked",
            Self::WrongPassword => "wrongPassword",
            Self::CookieInvalid => "cookieInvalid",
            Self::Challenge => "challenge",
            Self::RateLimited(_) => "rateLimited",
            Self::TooManyAttempts(_) => "tooManyAttempts",
            Self::BoundSessionRequired => "boundSessionRequired",
            Self::Roblox(_) => "roblox",
            Self::Network(_) => "network",
            Self::NotFound => "notFound",
            Self::Cancelled => "cancelled",
            Self::Invalid(_) | Self::Wait(_) => "invalid",
            Self::Storage(_) => "storage",
            Self::Crypto => "crypto",
            Self::Internal(_) => "internal",
        }
    }

    pub fn temporary(&self) -> bool {
        match self {
            Self::Locked | Self::RateLimited(_) | Self::TooManyAttempts(_) | Self::Network(_) | Self::Wait(_) => true,
            Self::Roblox(why) => !why.starts_with("HTTP 4"),
            _ => false,
        }
    }

    pub fn is_forbidden(&self) -> bool {
        matches!(self, Self::Roblox(why) if why == "HTTP 403")
    }

    pub fn log(&self) {
        let level = match self {
            Self::Cancelled | Self::Invalid(_) | Self::Wait(_) | Self::Locked | Self::WrongPassword | Self::NotFound => log::Level::Debug,
            Self::Internal(_) | Self::Storage(_) | Self::Crypto => log::Level::Error,
            _ => log::Level::Warn,
        };
        log::log!(level, "{} error: {self}", self.kind());
    }

    pub fn message(&self) -> String {
        match self {
            Self::Locked => tr!("The vault is locked."),
            Self::WrongPassword => tr!("Wrong password."),
            Self::CookieInvalid => tr!("This cookie is not valid or has expired. Sign in again."),
            Self::Challenge => {
                tr!("Roblox wants a human check for this account. Sign in to it again with Add account > Sign in with Roblox.")
            }
            Self::RateLimited(s) => tr!("Roblox is limiting requests. Try again in {seconds} s.", seconds = s),
            Self::TooManyAttempts(s) => tr!("Too many wrong passwords. Try again in {seconds} s.", seconds = s),
            Self::BoundSessionRequired => tr!("Roblox now requires a bound session for this account."),
            Self::Roblox(why) => tr!("Roblox answered unexpectedly ({why}).", why = why),
            Self::Network(why) => tr!("Network error: {why}", why = why),
            Self::NotFound => tr!("Not found."),
            Self::Cancelled => tr!("Cancelled."),
            Self::Invalid(m) | Self::Wait(m) | Self::Internal(m) => m.clone(),
            Self::Storage(why) => tr!("Storage error: {why}", why = why),
            Self::Crypto => tr!("Encryption failed."),
        }
    }
}

#[derive(Serialize, TS)]
#[ts(export)]
pub struct ErrorPayload {
    pub kind: String,
    pub message: String,
}

impl Serialize for AppError {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        self.log();
        ErrorPayload { kind: self.kind().into(), message: self.message() }.serialize(s)
    }
}

impl From<rusqlite::Error> for AppError {
    fn from(e: rusqlite::Error) -> Self {
        match e {
            rusqlite::Error::QueryReturnedNoRows => Self::NotFound,
            e => Self::Storage(e.to_string()),
        }
    }
}

impl From<std::io::Error> for AppError {
    fn from(e: std::io::Error) -> Self {
        Self::Internal(e.to_string())
    }
}

impl From<tauri::Error> for AppError {
    fn from(e: tauri::Error) -> Self {
        Self::Internal(e.to_string())
    }
}

pub type Result<T, E = AppError> = std::result::Result<T, E>;
