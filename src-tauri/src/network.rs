//! Public-network guard. Windows files every network as Public, Private or Domain (the user
//! picks it when joining: a café or hotel Wi-Fi is Public). While a connected network with
//! internet access is Public and `Settings.allow_public_networks` is off (it is on by default):
//! - launches are refused (`blocked`, checked by `commands::launch_tracked`, so single launches,
//!   batches, auto-rejoin and the tray alike) and a running batch stops;
//! - a master-password vault locks (on joining such a network, not again while on it).
//!
//! Read through the Network List Manager every `TICK`. Only networks with internet access
//! count: Hyper-V, WSL and VPN adapters are often filed Public but reach nothing. The network's
//! name is never read: it would show on a streamer's screen and in the log.

use std::sync::Mutex;
use std::thread::sleep;
use std::time::Duration;

use tauri::{AppHandle, Emitter, Manager};
use windows::Win32::Networking::NetworkListManager::{
    INetworkListManager, NLM_CONNECTIVITY, NLM_CONNECTIVITY_IPV4_INTERNET, NLM_CONNECTIVITY_IPV6_INTERNET,
    NLM_ENUM_NETWORK_CONNECTED, NLM_NETWORK_CATEGORY, NLM_NETWORK_CATEGORY_PUBLIC, NetworkListManager,
};
use windows::Win32::System::Com::{CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx};

use crate::App;
use crate::commands::notice;
use crate::model::{NetworkStatus, VaultMode};

const TICK: Duration = Duration::from_secs(10);

/// The network status as last read (`App.network`).
pub struct Network(Mutex<NetworkStatus>);

impl Network {
    pub fn new() -> Self {
        Self(Mutex::new(scan()))
    }

    pub fn get(&self) -> NetworkStatus {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    fn set(&self, n: NetworkStatus) {
        *self.0.lock().unwrap_or_else(|e| e.into_inner()) = n;
    }
}

/// One connected network as Windows files it.
struct Seen {
    category: NLM_NETWORK_CATEGORY,
    connectivity: NLM_CONNECTIVITY,
}

/// Whether a connected network is Public and reaches the internet (pure).
fn public_of(nets: &[Seen]) -> bool {
    let internet = NLM_CONNECTIVITY_IPV4_INTERNET.0 | NLM_CONNECTIVITY_IPV6_INTERNET.0;
    nets.iter().any(|n| n.category == NLM_NETWORK_CATEGORY_PUBLIC && n.connectivity.0 & internet != 0)
}

/// Reads the connected networks; not public when Windows can't say (logged).
pub fn scan() -> NetworkStatus {
    // On its own thread: COM as multithreaded on the main thread breaks the window's
    // `OleInitialize` (RPC_E_CHANGED_MODE, the app doesn't start).
    let read = std::thread::spawn(networks).join().unwrap_or_else(|_| {
        log::warn!("network: COM thread panicked");
        Err(windows::core::Error::empty())
    });
    match read {
        Ok(nets) => NetworkStatus { public: public_of(&nets) },
        Err(e) => {
            log::warn!("couldn't read the network category: {e}");
            NetworkStatus { public: false }
        }
    }
}

fn networks() -> windows::core::Result<Vec<Seen>> {
    // SAFETY: COM on this thread (S_FALSE: already initialised, also fine); the manager and
    // the networks are COM objects released on drop.
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let nlm: INetworkListManager = CoCreateInstance(&NetworkListManager, None, CLSCTX_ALL)?;
        let list = nlm.GetNetworks(NLM_ENUM_NETWORK_CONNECTED)?;
        let mut out = Vec::new();
        loop {
            let mut one = [None];
            let mut got = 0;
            list.Next(&mut one, Some(&mut got))?;
            let Some(net) = one[0].take().filter(|_| got == 1) else { break };
            out.push(Seen {
                category: net.GetCategory()?,
                connectivity: net.GetConnectivity()?,
            });
        }
        Ok(out)
    }
}

/// Why launching isn't allowed now; None when it is.
pub fn blocked(s: &App) -> Option<String> {
    if !s.network.get().public || s
        .db
        .settings()
        .inspect_err(|e| log::warn!("network: settings unreadable, public networks not allowed: {e}"))
        .is_ok_and(|v| v.allow_public_networks) {
        return None;
    }
    Some(tr!(
        "You're on a public network. LRAM doesn't launch there; join a private network, or allow public \
         networks in Settings, Security."
    ))
}

/// Runs for the app's lifetime on its own thread (COM calls block briefly).
pub fn spawn(app: AppHandle) {
    let started = std::thread::Builder::new().name("network".into()).spawn(move || {
        let mut was = blocked(&app.state::<App>()).is_some();
        loop {
            sleep(TICK);
            let s = app.state::<App>();
            let now = scan();
            if now != s.network.get() {
                log::info!("network: {}", if now.public { "public" } else { "not public" });
                s.network.set(now.clone());
                let _ = app.emit("app://network", now);
            }
            let why = blocked(&s);
            if let (false, Some(why)) = (was, &why) {
                enforce(&app, why);
            }
            was = why.is_some();
        }
    });
    if let Err(e) = started {
        log::warn!("public-network guard not running: {e}");
    }
}

/// Joined a public network: what runs stops (see the module docs).
fn enforce(app: &AppHandle, why: &str) {
    let s = app.state::<App>();
    if let Some(batch) = s.bulk.current().filter(|b| !b.done)
        && let Err(e) = s.bulk.cancel(&batch.batch_id)
    {
        log::warn!("public network: could not stop batch {}: {e}", batch.batch_id);
    }
    let lock = s.vault.mode() == VaultMode::Master && !s.vault.is_locked();
    if lock {
        crate::commands::lock_vault(app);
        notice(app, true, tr!("{why} Meanwhile the vault locked.", why = why));
    }
    log::info!("public network: launches blocked{}", if lock { ", vault locked" } else { "" });
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::Networking::NetworkListManager::{
        NLM_CONNECTIVITY_DISCONNECTED, NLM_CONNECTIVITY_IPV4_LOCALNETWORK, NLM_NETWORK_CATEGORY_PRIVATE,
    };

    fn net(category: NLM_NETWORK_CATEGORY, connectivity: NLM_CONNECTIVITY) -> Seen {
        Seen { category, connectivity }
    }

    #[test]
    fn only_public_networks_with_internet_count() {
        let home = || net(NLM_NETWORK_CATEGORY_PRIVATE, NLM_CONNECTIVITY_IPV4_INTERNET);
        let hyperv = net(NLM_NETWORK_CATEGORY_PUBLIC, NLM_CONNECTIVITY_IPV4_LOCALNETWORK);
        let cafe = net(NLM_NETWORK_CATEGORY_PUBLIC, NLM_CONNECTIVITY_IPV6_INTERNET);
        let gone = net(NLM_NETWORK_CATEGORY_PUBLIC, NLM_CONNECTIVITY_DISCONNECTED);
        assert!(!public_of(&[]));
        assert!(!public_of(&[home(), hyperv, gone]));
        assert!(public_of(&[home(), cafe]));
        // Both IPv4 and IPv6 internet bits.
        let both = net(NLM_NETWORK_CATEGORY_PUBLIC, NLM_CONNECTIVITY(NLM_CONNECTIVITY_IPV4_INTERNET.0 | NLM_CONNECTIVITY_IPV6_INTERNET.0));
        assert!(public_of(&[both]));
    }
}
