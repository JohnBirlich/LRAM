//! Read-only machine inventory. No shell, serial numbers, identifiers or network requests.
use serde::Serialize;
use ts_rs::TS;
use windows::Win32::Graphics::Dxgi::{CreateDXGIFactory1, IDXGIFactory1, DXGI_ADAPTER_FLAG_SOFTWARE};
use windows::Win32::System::Registry::HKEY_LOCAL_MACHINE;

#[derive(Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct GraphicsAdapter {
    pub name: String,
    pub dedicated_mb: f64,
}

#[derive(Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct HardwareInfo {
    pub board: Option<String>,
    pub cpu: Option<String>,
    pub logical_processors: Option<u32>,
    pub memory_rates: Vec<u32>,
    pub gpus: Vec<GraphicsAdapter>,
}

pub fn read() -> HardwareInfo {
    let reg = |key: &str, name: &str| crate::launcher::client::reg_string(HKEY_LOCAL_MACHINE, key, Some(name));
    let bios = r"HARDWARE\DESCRIPTION\System\BIOS";
    let board: Vec<String> = [reg(bios, "BaseBoardManufacturer"), reg(bios, "BaseBoardProduct")].into_iter().flatten().collect();
    HardwareInfo {
        board: (!board.is_empty()).then(|| board.join(" ")),
        cpu: reg(r"HARDWARE\DESCRIPTION\System\CentralProcessor\0", "ProcessorNameString").map(|s| s.trim().to_owned()),
        logical_processors: std::thread::available_parallelism().ok().map(|n| n.get() as u32),
        memory_rates: firmware().map(|b| memory_rates(&b)).unwrap_or_default(),
        gpus: adapters(),
    }
}

fn adapters() -> Vec<GraphicsAdapter> {
    let mut out = Vec::new();
    // SAFETY: DXGI creates owned COM interfaces; descriptors are copied before releasing them.
    unsafe {
        let Ok(factory) = CreateDXGIFactory1::<IDXGIFactory1>() else { return out };
        for i in 0..32 {
            let Ok(adapter) = factory.EnumAdapters1(i) else { break };
            let Ok(desc) = adapter.GetDesc1() else { continue };
            if desc.Flags & DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32 != 0 { continue; }
            let end = desc.Description.iter().position(|&c| c == 0).unwrap_or(desc.Description.len());
            out.push(GraphicsAdapter {
                name: String::from_utf16_lossy(&desc.Description[..end]),
                dedicated_mb: desc.DedicatedVideoMemory as f64 / (1024.0 * 1024.0),
            });
        }
    }
    out
}

fn firmware() -> Option<Vec<u8>> {
    // The documented RSMB provider returns RawSMBIOSData, not an address to firmware memory.
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetSystemFirmwareTable(provider: u32, table: u32, buffer: *mut std::ffi::c_void, size: u32) -> u32;
    }
    let provider = u32::from_be_bytes(*b"RSMB");
    // SAFETY: first ask for the size with a null buffer, then pass a bounded allocation.
    let size = unsafe { GetSystemFirmwareTable(provider, 0, std::ptr::null_mut(), 0) };
    if !(8..=4_194_304).contains(&size) { return None; }
    let mut data = vec![0u8; size as usize];
    // SAFETY: the allocation is exactly `size` bytes and remains alive during the call.
    let got = unsafe { GetSystemFirmwareTable(provider, 0, data.as_mut_ptr().cast(), size) };
    (got == size).then_some(data)
}

// SMBIOS type 17: report configured transfer rate (MT/s), with rated speed as fallback.
// Unknown/extended encodings are left unavailable rather than inventing a frequency.
fn memory_rates(raw: &[u8]) -> Vec<u32> {
    let mut out = Vec::new();
    let Some(len) = raw.get(4..8).and_then(|v| <[u8; 4]>::try_from(v).ok()).map(u32::from_le_bytes) else { return out };
    let Some(table) = raw.get(8..8usize.saturating_add(len as usize)) else { return out };
    let mut at = 0;
    while let Some(header) = table.get(at..at + 4) {
        let (kind, size) = (header[0], header[1] as usize);
        if size < 4 { break; }
        let Some(record) = table.get(at..at + size) else { break };
        if kind == 17 {
            let word = |offset| record.get(offset..offset + 2).map(|v| u16::from_le_bytes([v[0], v[1]]));
            // Size 0 means an empty DIMM slot.
            if word(12).is_some_and(|v| v != 0) {
                let valid = |v: &u16| *v > 0 && *v < u16::MAX;
                if let Some(rate) = word(32).filter(valid).or_else(|| word(21).filter(valid)) {
                    out.push(rate as u32);
                }
            }
        }
        let Some(end) = table[at + size..].windows(2).position(|s| s == [0, 0]) else { break };
        at += size + end + 2;
        if kind == 127 { break; }
    }
    out.sort_unstable();
    out.dedup();
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn malformed_firmware_is_safe() {
        for n in 0..80 { assert!(memory_rates(&vec![0; n]).is_empty()); }
        assert!(memory_rates(&[0xff; 80]).is_empty());
    }
    #[test]
    fn configured_memory_rate_and_empty_slots() {
        let mut record = vec![0u8; 34];
        record[0] = 17; record[1] = 34;
        record[12..14].copy_from_slice(&8192u16.to_le_bytes());
        record[21..23].copy_from_slice(&3200u16.to_le_bytes());
        record[32..34].copy_from_slice(&2666u16.to_le_bytes());
        record.extend([0, 0]);
        let mut raw = vec![0, 3, 0, 0];
        raw.extend((record.len() as u32).to_le_bytes()); raw.extend(&record);
        assert_eq!(memory_rates(&raw), [2666]);
        raw[20..22].fill(0);
        assert!(memory_rates(&raw).is_empty());
    }
}

#[derive(Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PublicLocation {
    pub ip: String,
    pub country: String,
}

/// Explicit user action only. No cookies, machine inventory or identifiers leave the PC.
pub async fn public_location() -> crate::error::Result<PublicLocation> {
    use crate::error::AppError;
    let unavailable = || AppError::Network("IP/country lookup unavailable. Hardware and capacity estimates still work offline.".into());
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(8))
        .redirect(reqwest::redirect::Policy::none())
        .build().map_err(|_| unavailable())?;
    let mut response = client.get("https://ipwho.is/?fields=success,ip,country")
        .send().await.map_err(|_| unavailable())?;
    if !response.status().is_success() { return Err(unavailable()); }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| unavailable())? {
        if bytes.len() + chunk.len() > 4096 { return Err(unavailable()); }
        bytes.extend_from_slice(&chunk);
    }
    #[derive(serde::Deserialize)]
    struct Reply { success: bool, ip: String, country: String }
    let reply: Reply = serde_json::from_slice(&bytes).map_err(|_| unavailable())?;
    if !reply.success || reply.ip.parse::<std::net::IpAddr>().is_err()
        || reply.country.is_empty() || reply.country.len() > 100 || reply.country.chars().any(char::is_control) {
        return Err(unavailable());
    }
    Ok(PublicLocation { ip: reply.ip, country: reply.country })
}
