//! The bandwidth limit: a Windows QoS policy throttling the outbound traffic of every
//! `RobloxPlayerBeta.exe` together. Setting it needs admin rights, so `apply` runs a fixed,
//! built-in PowerShell script (only the validated rate is put in) elevated through the UAC
//! prompt, and only when the user clicks Apply. The policy lives in the ActiveStore: Windows
//! drops it on restart, and nothing about the system stays changed. Reading it back needs admin
//! rights too, so whether it is in force is judged by when it was applied (`applied_this_boot`).

use std::os::windows::ffi::OsStrExt;
use std::time::Duration;

use windows::Win32::Foundation::{ERROR_CANCELLED, WAIT_OBJECT_0};
use windows::Win32::System::SystemInformation::GetTickCount64;
use windows::Win32::System::Threading::{GetExitCodeProcess, WaitForSingleObject};
use windows::Win32::UI::Shell::{SEE_MASK_NOASYNC, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW, ShellExecuteExW};
use windows::Win32::UI::WindowsAndMessaging::SW_HIDE;
use windows::core::{Owned, PCWSTR, w};

use crate::error::{AppError, Result};

const POLICY: &str = "LRAM-Roblox";
pub const MIN_KBPS: u32 = 64;
pub const MAX_KBPS: u32 = 10_000_000;
/// How long the elevated script may take once the prompt is answered.
const TIMEOUT: Duration = Duration::from_secs(60);

/// The script `apply` runs elevated (pure): removes LRAM's policy, then for a rate (kbps,
/// `MIN_KBPS..=MAX_KBPS`) adds it again; 0 only removes it. Nothing but the checked integer is
/// put into it.
pub fn script(kbps: u32) -> Result<String> {
    let remove = format!("Remove-NetQosPolicy -Name '{POLICY}' -PolicyStore ActiveStore -Confirm:$false -ErrorAction SilentlyContinue");
    if kbps == 0 {
        // Nothing to remove is fine, yet leaves PowerShell's exit code at 1.
        return Ok(format!("{remove}; exit 0"));
    }
    if !(MIN_KBPS..=MAX_KBPS).contains(&kbps) {
        return Err(AppError::Invalid(tr!("The bandwidth limit must be 0 (none) or {min} to {max} kbit/s.", min = MIN_KBPS, max = MAX_KBPS)));
    }
    let bits = u64::from(kbps) * 1000;
    Ok(format!(
        "$ErrorActionPreference = 'Stop'; {remove}; New-NetQosPolicy -Name '{POLICY}' -AppPathNameMatchCondition \
         'RobloxPlayerBeta.exe' -ThrottleRateActionBitsPerSecond {bits} -PolicyStore ActiveStore | Out-Null"
    ))
}

/// Sets (or with 0 removes) the limit: Windows shows its admin prompt, then the script runs
/// hidden; waits for it. Declining the prompt or a failing script is an error.
pub fn apply(kbps: u32) -> Result<()> {
    let script = script(kbps)?;
    let exe = std::env::var_os("SystemRoot").unwrap_or_else(|| r"C:\Windows".into());
    let exe = std::path::Path::new(&exe).join(r"System32\WindowsPowerShell\v1.0\powershell.exe");
    let wide = |s: &std::ffi::OsStr| s.encode_wide().chain([0]).collect::<Vec<u16>>();
    let file = wide(exe.as_os_str());
    let params = wide(format!("-NoProfile -NonInteractive -ExecutionPolicy Bypass -WindowStyle Hidden -Command \"{script}\"").as_ref());
    let mut info = SHELLEXECUTEINFOW {
        cbSize: size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC,
        lpVerb: w!("runas"),
        lpFile: PCWSTR(file.as_ptr()),
        lpParameters: PCWSTR(params.as_ptr()),
        nShow: SW_HIDE.0,
        ..Default::default()
    };
    // SAFETY: `info` is a local with its size set; the strings it points at outlive the call.
    if let Err(e) = unsafe { ShellExecuteExW(&mut info) } {
        return Err(if e.code() == ERROR_CANCELLED.to_hresult() {
            AppError::Invalid(tr!("The bandwidth limit needs admin rights; the prompt was declined."))
        } else {
            AppError::Internal(tr!("Could not start PowerShell: {why}", why = e.message()))
        });
    }
    if info.hProcess.is_invalid() {
        return Err(AppError::Internal(tr!("PowerShell didn't start.")));
    }
    // SAFETY: SEE_MASK_NOCLOSEPROCESS hands us the process handle; Owned closes it.
    let h = unsafe { Owned::new(info.hProcess) };
    let mut code = 1u32;
    // SAFETY: `h` is a live process handle; `code` is a local.
    unsafe {
        if WaitForSingleObject(*h, TIMEOUT.as_millis() as u32) != WAIT_OBJECT_0 {
            return Err(AppError::Internal(
                tr!("Windows didn't answer in time; the limit may or may not be set. Apply again to be sure."),
            ));
        }
        GetExitCodeProcess(*h, &mut code).map_err(|e| AppError::Internal(e.message()))?;
    }
    if code != 0 {
        return Err(AppError::Internal(tr!("Windows refused the bandwidth limit (PowerShell exit code {code}).", code = code)));
    }
    log::info!("bandwidth limit set to {kbps} kbit/s");
    Ok(())
}

/// When Windows last started, Unix ms (to the second or so).
pub fn boot_ms(now_ms: i64) -> i64 {
    // SAFETY: no arguments; reads the tick count.
    now_ms - unsafe { GetTickCount64() } as i64
}

/// A policy applied when Windows had started at `applied_boot_ms` is still in force now that it
/// started at `boot_ms` (pure; the two readings of the boot time differ by a few ms).
pub fn applied_this_boot(applied_boot_ms: i64, boot_ms: i64) -> bool {
    (applied_boot_ms - boot_ms).abs() < 60_000
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_script_has_only_a_checked_rate_in_it() {
        let s = script(1_000).unwrap();
        assert!(s.contains("-ThrottleRateActionBitsPerSecond 1000000 -PolicyStore ActiveStore"), "{s}");
        assert!(s.starts_with("$ErrorActionPreference = 'Stop'; Remove-NetQosPolicy -Name 'LRAM-Roblox'"), "{s}");
        assert!(!s.contains('"'), "it goes inside -Command \"...\"");
        let off = script(0).unwrap();
        assert!(off.starts_with("Remove-NetQosPolicy") && !off.contains("New-NetQosPolicy"), "{off}");
        for bad in [1, MIN_KBPS - 1, MAX_KBPS + 1, u32::MAX] {
            assert!(matches!(script(bad), Err(AppError::Invalid(_))), "{bad}");
        }
        assert!(script(MIN_KBPS).is_ok() && script(MAX_KBPS).is_ok());
        assert!(script(MAX_KBPS).unwrap().contains(&format!("{}", u64::from(MAX_KBPS) * 1000)));
    }

    #[test]
    fn a_policy_lasts_until_windows_restarts() {
        let now = 1_800_000_000_000;
        let boot = boot_ms(now);
        assert!(boot <= now);
        assert!(applied_this_boot(boot + 30, boot));
        assert!(!applied_this_boot(boot - 3_600_000, boot), "applied before a restart");
        assert!(!applied_this_boot(0, boot), "never applied");
    }
}
