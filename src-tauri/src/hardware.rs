//! Read-only Windows inventory. Device identities are local correlation keys only:
//! never serialized, logged, or sent over the network.
use serde::Serialize;
use ts_rs::TS;
use windows::core::{GUID, PCWSTR, w};
use windows::Win32::Graphics::Dxgi::{CreateDXGIFactory1, IDXGIFactory1, DXGI_ADAPTER_FLAG_SOFTWARE};
use windows::Win32::System::Registry::HKEY_LOCAL_MACHINE;
use std::collections::{HashMap, HashSet};

type AdapterId = (i32, u32);
const DISPLAY_CLASS: GUID = GUID::from_u128(0x4d36e968_e325_11ce_bfc1_08002be10318);
const MONITOR_CLASS: GUID = GUID::from_u128(0x4d36e96e_e325_11ce_bfc1_08002be10318);

#[derive(Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct GraphicsAdapter {
    pub name: String,
    pub dedicated_mb: Option<f64>,
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
    pub virtual_adapters: Vec<String>,
    pub other_adapters: Vec<String>,
    pub displays: Vec<Display>,
}

pub fn read() -> HardwareInfo {
    let reg = |key: &str, name: &str| crate::launcher::client::reg_string(HKEY_LOCAL_MACHINE, key, Some(name));
    let bios = r"HARDWARE\DESCRIPTION\System\BIOS";
    let board: Vec<String> = [reg(bios, "BaseBoardManufacturer"), reg(bios, "BaseBoardProduct")].into_iter().flatten().collect();
    let (gpus, virtual_adapters, other_adapters) = adapters();
    HardwareInfo {
        board: (!board.is_empty()).then(|| board.join(" ")),
        cpu: reg(r"HARDWARE\DESCRIPTION\System\CentralProcessor\0", "ProcessorNameString").map(|s| s.trim().to_owned()),
        logical_processors: std::thread::available_parallelism().ok().map(|n| n.get() as u32),
        memory_rates: firmware().map(|b| memory_rates(&b)).unwrap_or_default(),
        gpus, virtual_adapters, other_adapters, displays: displays(),
    }
}

fn text(buf: &[u16]) -> String {
    String::from_utf16_lossy(&buf[..buf.iter().position(|c| *c == 0).unwrap_or(buf.len())]).trim().to_owned()
}

struct PnpDevice { id: String, name: String, luid: Option<AdapterId> }

fn pnp_devices(class: &GUID) -> Vec<PnpDevice> {
    use windows::Win32::Devices::DeviceAndDriverInstallation::*;
    use windows::Win32::System::Registry::{KEY_READ, RegCloseKey, RegQueryValueExW, REG_BINARY, REG_QWORD, REG_VALUE_TYPE};
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    // SAFETY: owned SetupAPI list is destroyed below; every struct/buffer is sized.
    // No device handles are opened for writes. Registry handles are always closed.
    unsafe {
        let Ok(set) = SetupDiGetClassDevsW(Some(class), PCWSTR::null(), None, DIGCF_PRESENT) else { return out };
        for i in 0..256 {
            let mut data = SP_DEVINFO_DATA { cbSize: std::mem::size_of::<SP_DEVINFO_DATA>() as u32, ..Default::default() };
            if SetupDiEnumDeviceInfo(set, i, &mut data).is_err() { break; }
            let mut id = [0u16; 1024];
            if SetupDiGetDeviceInstanceIdW(set, &data, Some(&mut id), None).is_err() { continue; }
            let id = text(&id).to_ascii_uppercase();
            if id.is_empty() || !seen.insert(id.clone()) { continue; }
            let property = |prop| {
                let mut bytes = [0u8; 8192];
                let mut size = 0;
                if SetupDiGetDeviceRegistryPropertyW(set, &data, prop, None, Some(&mut bytes), Some(&mut size)).is_err() { return None; }
                let words: Vec<_> = bytes[..(size as usize).min(bytes.len())].chunks_exact(2).map(|b| u16::from_le_bytes([b[0], b[1]])).collect();
                let name = text(&words);
                (!name.is_empty()).then_some(name)
            };
            let name = property(SPDRP_FRIENDLYNAME).or_else(|| property(SPDRP_DEVICEDESC)).unwrap_or_else(|| "Unknown display device".into());
            let luid = if *class == DISPLAY_CLASS {
                SetupDiOpenDevRegKey(set, &data, DICS_FLAG_GLOBAL.0, 0, DIREG_DRV, KEY_READ.0).ok().and_then(|key| {
                    let mut bytes = [0u8; 8];
                    let mut size = 8;
                    let mut kind = REG_VALUE_TYPE(0);
                    let result = RegQueryValueExW(key, w!("AdapterLuid"), None, Some(&mut kind), Some(bytes.as_mut_ptr()), Some(&mut size));
                    let _ = RegCloseKey(key);
                    (result.0 == 0 && size == 8 && (kind == REG_BINARY || kind == REG_QWORD)).then(|| (i32::from_le_bytes(bytes[4..].try_into().unwrap()), u32::from_le_bytes(bytes[..4].try_into().unwrap())))
                })
            } else { None };
            out.push(PnpDevice { id, name, luid });
        }
        let _ = SetupDiDestroyDeviceInfoList(set);
    }
    out
}

fn remember_adapter(seen: &mut HashSet<AdapterId>, high: i32, low: u32) -> bool { seen.insert((high, low)) }

// DXGI supplies memory only, never physical device count/name. Virtual/indirect
// drivers can expose the same rendering GPU under several adapter LUIDs.
#[derive(Default)]
struct GraphicsMemory { by_luid: HashMap<AdapterId, f64>, by_device: HashMap<String, Vec<f64>> }
impl GraphicsMemory {
    fn for_device(&self, d: &PnpDevice) -> Option<f64> {
        match self.by_device.get(&d.id) {
            Some(values) => values.first().copied().filter(|v| values.iter().all(|other| other == v)),
            None => d.luid.and_then(|id| self.by_luid.get(&id).copied()),
        }
    }
}
fn dxgi_memory() -> GraphicsMemory {
    use windows::Win32::Devices::Display::{DisplayConfigGetDeviceInfo, DISPLAYCONFIG_ADAPTER_NAME, DISPLAYCONFIG_DEVICE_INFO_HEADER, DISPLAYCONFIG_DEVICE_INFO_GET_ADAPTER_NAME};
    let mut out = GraphicsMemory::default();
    let mut seen = HashSet::new();
    // SAFETY: factory/adapter COM references are owned; GetDesc1 copies the descriptor.
    unsafe {
        let Ok(factory) = CreateDXGIFactory1::<IDXGIFactory1>() else { return out };
        for i in 0..64 {
            let Ok(adapter) = factory.EnumAdapters1(i) else { break };
            let Ok(d) = adapter.GetDesc1() else { continue };
            if d.Flags & DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32 != 0 || !remember_adapter(&mut seen, d.AdapterLuid.HighPart, d.AdapterLuid.LowPart) { continue; }
            if d.DedicatedVideoMemory > 0 {
                let mb = d.DedicatedVideoMemory as f64 / (1024.0 * 1024.0);
                out.by_luid.insert((d.AdapterLuid.HighPart, d.AdapterLuid.LowPart), mb);
                let mut name = DISPLAYCONFIG_ADAPTER_NAME { header: DISPLAYCONFIG_DEVICE_INFO_HEADER { r#type: DISPLAYCONFIG_DEVICE_INFO_GET_ADAPTER_NAME, size: std::mem::size_of::<DISPLAYCONFIG_ADAPTER_NAME>() as u32, adapterId: d.AdapterLuid, id: 0 }, ..Default::default() };
                if DisplayConfigGetDeviceInfo(&mut name.header) == 0 && let Some(id) = device_instance(&text(&name.adapterDevicePath)) {
                    out.by_device.entry(id).or_default().push(mb);
                }
            }
        }
    }
    out
}

#[derive(Debug, PartialEq)]
enum AdapterKind { Physical, Virtual, Other }
fn adapter_kind(id: &str) -> AdapterKind {
    if id.starts_with("PCI\\") { AdapterKind::Physical }
    else if id.starts_with("ROOT\\") || id.starts_with("SWD\\") { AdapterKind::Virtual }
    else { AdapterKind::Other }
}
fn adapter_inventory(devices: Vec<PnpDevice>, memory: &GraphicsMemory) -> (Vec<GraphicsAdapter>, Vec<String>, Vec<String>) {
    let (mut physical, mut virtuals, mut other) = (Vec::new(), Vec::new(), Vec::new());
    let mut seen = HashSet::new();
    for d in devices {
        if !seen.insert(d.id.clone()) { continue; }
        match adapter_kind(&d.id) {
            AdapterKind::Physical => { let dedicated_mb = memory.for_device(&d); physical.push(GraphicsAdapter { name: d.name, dedicated_mb }); },
            AdapterKind::Virtual => virtuals.push(d.name),
            AdapterKind::Other => other.push(d.name),
        }
    }
    (physical, virtuals, other)
}
fn adapters() -> (Vec<GraphicsAdapter>, Vec<String>, Vec<String>) { adapter_inventory(pnp_devices(&DISPLAY_CLASS), &dxgi_memory()) }

#[derive(Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Display {
    pub name: Option<String>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub refresh_hz: Option<f64>,
}

fn device_instance(path: &str) -> Option<String> {
    let parts: Vec<_> = path.strip_prefix(r"\\?\")?.split('#').collect();
    (parts.len() >= 4 && parts[..3].iter().all(|p| !p.is_empty())).then(|| parts[..3].join("\\").to_ascii_uppercase())
}
fn monitor_instance(path: &str) -> Option<String> { device_instance(path).filter(|id| id.starts_with("DISPLAY\\")) }
fn refresh_hz(numerator: u32, denominator: u32) -> Option<f64> {
    (numerator > 0 && denominator > 0).then(|| (numerator as f64 / denominator as f64 * 100.0).round() / 100.0)
}
fn displays() -> Vec<Display> {
    use windows::Win32::Devices::Display::*;
    let monitors: HashMap<_, _> = pnp_devices(&MONITOR_CLASS).into_iter().map(|d| (d.id, d.name)).collect();
    // A topology change can invalidate the size query; retry with fresh buffers.
    // SAFETY: arrays are bounded, initialized and allocated to the reported sizes.
    // Device-info headers specify each exact struct size. Mode unions are read
    // only after checking their discriminant, source ID and adapter identity.
    unsafe {
        for _ in 0..3 {
            let (mut np, mut nm) = (0, 0);
            if GetDisplayConfigBufferSizes(QDC_ONLY_ACTIVE_PATHS, &mut np, &mut nm).0 != 0 || np > 256 || nm > 1024 { return Vec::new(); }
            if np == 0 { return Vec::new(); }
            let mut paths = vec![DISPLAYCONFIG_PATH_INFO::default(); np as usize];
            let mut modes = vec![DISPLAYCONFIG_MODE_INFO::default(); nm as usize];
            let result = QueryDisplayConfig(QDC_ONLY_ACTIVE_PATHS, &mut np, paths.as_mut_ptr(), &mut nm, modes.as_mut_ptr(), None);
            if result.0 == 122 { continue; }
            if result.0 != 0 { return Vec::new(); }
            let mut out = Vec::new();
            let mut seen = HashSet::new();
            for p in &paths[..np as usize] {
                let mut target = DISPLAYCONFIG_TARGET_DEVICE_NAME { header: DISPLAYCONFIG_DEVICE_INFO_HEADER { r#type: DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_NAME, size: std::mem::size_of::<DISPLAYCONFIG_TARGET_DEVICE_NAME>() as u32, adapterId: p.targetInfo.adapterId, id: p.targetInfo.id }, ..Default::default() };
                let named = DisplayConfigGetDeviceInfo(&mut target.header) == 0;
                let path = if named { text(&target.monitorDevicePath) } else { String::new() };
                let key = if path.is_empty() { format!("{}:{}:{}", p.targetInfo.adapterId.HighPart, p.targetInfo.adapterId.LowPart, p.targetInfo.id) } else { path.to_ascii_uppercase() };
                if !seen.insert(key) { continue; }
                let friendly = if named { text(&target.monitorFriendlyDeviceName) } else { String::new() };
                let name = if !friendly.is_empty() && !friendly.to_ascii_lowercase().starts_with("generic") { Some(friendly) }
                    else { monitor_instance(&path).and_then(|id| monitors.get(&id).cloned()).or_else(|| (!friendly.is_empty()).then_some(friendly)) };
                let source = modes[..nm as usize].iter().find(|m| m.infoType == DISPLAYCONFIG_MODE_INFO_TYPE_SOURCE && m.id == p.sourceInfo.id && m.adapterId.HighPart == p.sourceInfo.adapterId.HighPart && m.adapterId.LowPart == p.sourceInfo.adapterId.LowPart).map(|m| m.Anonymous.sourceMode);
                out.push(Display { name, width: source.and_then(|m| (m.width > 0).then_some(m.width)), height: source.and_then(|m| (m.height > 0).then_some(m.height)), refresh_hz: refresh_hz(p.targetInfo.refreshRate.Numerator, p.targetInfo.refreshRate.Denominator) });
            }
            return out;
        }
    }
    Vec::new()
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

#[cfg(test)]
mod tests {
    #[test]
    fn adapter_device_paths_correlate_vram_without_registry_and_conflicts_stay_unknown() {
        let id = device_instance(r"\\?\PCI#VEN_10DE#CARD_A#{guid}").unwrap();
        let d = PnpDevice { id: id.clone(), name: "GPU".into(), luid: None };
        let mut memory = GraphicsMemory::default();
        memory.by_device.insert(id.clone(), vec![12_000.0, 12_000.0]);
        assert_eq!(memory.for_device(&d), Some(12_000.0));
        memory.by_device.get_mut(&id).unwrap().push(8_000.0);
        assert_eq!(memory.for_device(&d), None, "ambiguous driver data must not be guessed");
    }

    #[test]
    fn pnp_devices_not_dxgi_render_aliases_define_physical_gpu_count() {
        let device = |id: &str, name: &str, luid| PnpDevice { id: id.into(), name: name.into(), luid };
        let memory = GraphicsMemory { by_luid: HashMap::from([((0, 1), 12_000.0), ((0, 2), 12_000.0), ((0, 3), 12_000.0)]), ..Default::default() };
        let (physical, virtuals, other) = adapter_inventory(vec![
            device(r"PCI\VEN_10DE\CARD_A", "NVIDIA", Some((0, 1))),
            device(r"PCI\VEN_10DE\CARD_A", "NVIDIA duplicate", Some((0, 1))),
            device(r"ROOT\MUMU\0000", "MuMuPlayer Virtual Display Adapter", Some((0, 2))),
            device(r"SWD\STAR\0000", "StarDesk Virtual Display Adapter", Some((0, 3))),
        ], &memory);
        assert_eq!(physical.len(), 1);
        assert_eq!(physical[0].dedicated_mb, Some(12_000.0));
        assert_eq!(virtuals.len(), 2);
        assert!(other.is_empty());
        let (physical, _, _) = adapter_inventory(vec![device(r"PCI\A", "Identical GPU", Some((0, 1))), device(r"PCI\B", "Identical GPU", None)], &memory);
        assert_eq!(physical.len(), 2, "never merge identical model names");
        assert_eq!(physical[1].dedicated_mb, None, "never guess VRAM by name, vendor or array position");
        assert_eq!(adapter_kind(r"USB\DISPLAYLINK"), AdapterKind::Other, "unknown buses are not claimed to be virtual or PCI GPUs");
    }

    #[test]
    fn monitor_paths_not_list_positions_match_names_and_fractional_refresh_is_preserved() {
        assert_eq!(monitor_instance(r"\\?\DISPLAY#MSI1234#5&123&0&UID1#{guid}"), Some(r"DISPLAY\MSI1234\5&123&0&UID1".into()));
        assert_eq!(monitor_instance("not a device interface"), None);
        assert_eq!(refresh_hz(60000, 1001), Some(59.94));
        assert_eq!(refresh_hz(0, 1), None);
        assert_eq!(refresh_hz(60, 0), None);
    }

    #[test]
    fn duplicate_adapter_id_is_not_three_cards_but_identical_models_can_be_distinct() {
        let mut seen = std::collections::HashSet::new();
        assert!(remember_adapter(&mut seen, 0, 41));
        assert!(!remember_adapter(&mut seen, 0, 41));
        assert!(!remember_adapter(&mut seen, 0, 41));
        assert!(remember_adapter(&mut seen, 0, 42), "do not collapse two equal GPU model names with distinct identities");
    }
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
