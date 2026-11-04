use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use ts_rs::TS;

#[derive(Serialize, Deserialize, TS, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
#[ts(export)]
pub enum VaultMode {
    Dpapi,
    Master,
}

#[derive(Serialize, TS, Clone, Debug)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct VaultStatus {
    pub mode: VaultMode,
    pub locked: bool,
    pub accounts: u32,
    pub auto_lock_minutes: u32,
    pub has_recovery: bool,
    #[ts(optional)]
    pub retry_in: Option<u64>,
}

#[derive(Serialize, Deserialize, TS, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
#[ts(export)]
pub enum AccountStatus {
    Unknown,
    Valid,
    Invalid,
    Challenged,
}

impl AccountStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Unknown => "unknown",
            Self::Valid => "valid",
            Self::Invalid => "invalid",
            Self::Challenged => "challenged",
        }
    }

    pub fn parse(s: &str) -> Self {
        match s {
            "valid" => Self::Valid,
            "invalid" => Self::Invalid,
            "challenged" => Self::Challenged,
            "unknown" => Self::Unknown,
            other => {
                log::debug!("unrecognised account status {other:?}");
                Self::Unknown
            }
        }
    }
}

#[derive(Serialize, TS, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct AccountSummary {
    pub id: i64,
    pub user_id: i64,
    pub username: String,
    pub display_name: String,
    pub alias: String,
    pub notes: String,
    pub color: Option<String>,
    pub favourite: bool,
    pub sort_order: i64,
    pub group_ids: Vec<i64>,
    pub status: AccountStatus,
    pub status_checked_at: Option<i64>,
    pub created_at: i64,
    pub allow_disconnect: bool,
    pub launch_args: Option<String>,
    pub discord_muted: bool,
    pub fps_cap: Option<u32>,
    pub full_quality: bool,
}

#[derive(Deserialize, TS, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct AccountPatch {
    #[ts(optional)]
    pub alias: Option<String>,
    #[ts(optional)]
    pub notes: Option<String>,
    #[ts(optional)]
    pub color: Option<String>,
    #[ts(optional)]
    pub favourite: Option<bool>,
    #[ts(optional)]
    pub group_ids: Option<Vec<i64>>,
    #[ts(optional)]
    pub allow_disconnect: Option<bool>,
    #[ts(optional)]
    pub launch_args: Option<String>,
    #[ts(optional)]
    pub discord_muted: Option<bool>,
    #[ts(optional)]
    pub fps_cap: Option<u32>,
    #[ts(optional)]
    pub full_quality: Option<bool>,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Group {
    pub id: i64,
    pub name: String,
    pub color: Option<String>,
    pub sort_order: i64,
}

fn number_prefix(name: &str) -> (Option<u32>, &str) {
    let digits = name.bytes().take_while(u8::is_ascii_digit).count();
    if digits == 0 {
        return (None, name);
    }
    let n = name.as_bytes()[..digits].iter().fold(0u32, |n, d| n.saturating_mul(10).saturating_add((d - b'0') as u32));
    (Some(n), name[digits..].trim_start_matches([' ', '-', '_', '.', ':']))
}

// -- Main func
pub fn group_order(a: &Group, b: &Group) -> Ordering {
    let (na, rest_a) = number_prefix(&a.name);
    let (nb, rest_b) = number_prefix(&b.name);
    match (na, nb) {
        (Some(x), Some(y)) => x.cmp(&y).then_with(|| rest_a.to_lowercase().cmp(&rest_b.to_lowercase())),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    }
    .then(a.sort_order.cmp(&b.sort_order))
    .then(a.id.cmp(&b.id))
}

#[derive(Deserialize, TS, Clone, Debug)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct GroupInput {
    #[ts(optional)]
    pub id: Option<i64>,
    pub name: String,
    #[ts(optional)]
    pub color: Option<String>,
}

#[derive(Serialize, Deserialize, TS, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
#[ts(export)]
pub enum TabLayout {
    #[default]
    Top,
    Side,
}

#[derive(Serialize, Deserialize, TS, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
#[ts(export)]
pub enum Theme {
    #[default]
    #[serde(alias = "neon", alias = "bricks")]
    Dark,
    #[serde(alias = "sakura")]
    Light,
    System,
}

#[derive(Serialize, Deserialize, TS, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
#[ts(export)]
pub enum Density {
    #[default]
    Detailed,
    Compact,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase", default)]
#[ts(export)]
pub struct Settings {
    pub theme: Theme,
    pub tab_layout: TabLayout,
    pub accent: String,
    pub background_dim: u8,
    pub density: Density,
    pub auto_lock_minutes: u32,
    pub clipboard_clear_seconds: u32,
    pub cache_minutes: u32,
    pub multi_instance: bool,
    pub protect_shared_login: bool,
    pub launch_delay_ms: u32,
    pub auto_rejoin: bool,
    pub auto_restart_on_crash: bool,
    pub breaker_max: u32,
    pub breaker_window_min: u32,
    pub close_grace_s: u32,
    pub launch_args: String,
    pub close_to_tray: bool,
    pub skip_splash: bool,
    pub mem_restart_mb: u32,
    pub mem_throttle: MemThrottle,
    pub trim_every_min: u32,
    pub auto_spread: bool,
    pub fair_cap: bool,
    pub adaptive: Adaptive,
    pub ultra_cpu: UltraCpu,
    pub bandwidth_kbps: u32,
    pub anti_afk: AntiAfk,
    pub mute_clients: bool,
    pub discord: DiscordEvents,
    pub auto_update: bool,
    pub self_update: bool,
    pub minimize_on_launch: bool,
    pub live_hidden_previews: bool,
    pub panic_hotkey: String,
    pub clean_logs: bool,
    pub fps_cap: u32,
    pub low_graphics: bool,
    pub lighter_rendering: bool,
    pub render_flags: RenderFlags,
    pub micro_width: u32,
    pub micro_height: u32,
    pub tiny_on_launch: bool,
    pub auto_tile: bool,
    pub allow_public_networks: bool,
}

#[derive(Serialize, TS, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct NetworkStatus {
    pub public: bool,
}

#[derive(Serialize, Deserialize, TS, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase", default)]
#[ts(export)]
pub struct MemThrottle {
    pub mb: u32,
    pub cpu_cap: u8,
}

impl Default for MemThrottle {
    fn default() -> Self {
        Self { mb: 0, cpu_cap: 10 }
    }
}

#[derive(Serialize, Deserialize, TS, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum Msaa {
    Default,
    #[default]
    X1,
    X2,
    X4,
}

#[derive(Serialize, Deserialize, TS, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum TextureQuality {
    Default,
    #[default]
    Lowest,
    Low,
    Medium,
    High,
}

#[derive(Serialize, Deserialize, TS, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum RenderMode {
    #[default]
    Default,
    D3d11,
    Vulkan,
    Opengl,
}

#[derive(Serialize, Deserialize, TS, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase", default)]
#[ts(export)]
pub struct RenderFlags {
    pub msaa: Msaa,
    pub texture: TextureQuality,
    pub mode: RenderMode,
    pub gray_sky: bool,
    pub no_grass: bool,
    pub lowest_frm: bool,
    pub pause_voxelizer: bool,
}

impl Default for RenderFlags {
    fn default() -> Self {
        Self { msaa: Msaa::X1, texture: TextureQuality::Lowest, mode: RenderMode::Default, gray_sky: false, no_grass: true, lowest_frm: true, pause_voxelizer: false }
    }
}

#[derive(Serialize, Deserialize, TS, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase", default)]
#[ts(export)]
pub struct AntiAfk {
    pub enabled: bool,
    pub interval_min: u32,
    pub mode: AfkMode,
    pub press: AfkPress,
}

impl Default for AntiAfk {
    fn default() -> Self {
        Self { enabled: false, interval_min: 15, mode: AfkMode::Focus, press: AfkPress::Space }
    }
}

#[derive(Serialize, Deserialize, TS, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
#[ts(export)]
pub enum AfkPress {
    #[default]
    Space,
    Camera,
    Mouse,
}

#[derive(Serialize, Deserialize, TS, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
#[ts(export)]
pub enum AfkMode {
    #[default]
    Focus,
    Background,
}

#[derive(Serialize, Deserialize, TS, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase", default)]
#[ts(export)]
pub struct DiscordEvents {
    pub crash: bool,
    pub hang: bool,
    pub mem_restart: bool,
    pub disconnect: bool,
    pub restart: bool,
    pub breaker: bool,
    pub batch_done: bool,
    pub launch: bool,
    pub status_every_min: u32,
}

impl Default for DiscordEvents {
    fn default() -> Self {
        Self { crash: true, hang: true, mem_restart: true, disconnect: true, restart: true, breaker: true, batch_done: true, launch: false, status_every_min: 0 }
    }
}

#[derive(Serialize, TS, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct DiscordStatus {
    pub configured: bool,
    pub webhook: Option<String>,
}

#[derive(Serialize, Deserialize, TS, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase", default)]
#[ts(export)]
pub struct Adaptive {
    pub enabled: bool,
    pub background: Limits,
}

impl Default for Adaptive {
    fn default() -> Self {
        Self { enabled: true, background: Limits { eco: true, ..Limits::default() } }
    }
}

#[derive(Serialize, Deserialize, TS, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase", default)]
#[ts(export)]
pub struct UltraCpu {
    pub enabled: bool,
    pub cap: u8,
    pub warmup_s: u32,
}
impl Default for UltraCpu {
    fn default() -> Self { Self { enabled: false, cap: 3, warmup_s: 90 } }
}

#[derive(Serialize, Deserialize, TS, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum CpuMode {
    #[default]
    Standard,
    Checking,
    Loading,
    Warmup,
    Foreground,
    Background,
    Manual,
    FullQuality,
}

impl Adaptive {
    fn untouched(&self) -> bool {
        !self.enabled && self.background == Limits { cpu_cap: Some(25), eco: true, ..Limits::default() }
    }
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            theme: Theme::Dark,
            tab_layout: TabLayout::Top,
            accent: String::new(),
            background_dim: 60,
            density: Density::Detailed,
            auto_lock_minutes: 15,
            clipboard_clear_seconds: 30,
            cache_minutes: 10,
            multi_instance: true,
            protect_shared_login: true,
            launch_delay_ms: 6_000,
            auto_rejoin: false,
            auto_restart_on_crash: false,
            breaker_max: 5,
            breaker_window_min: 10,
            close_grace_s: 5,
            launch_args: String::new(),
            close_to_tray: false,
            skip_splash: false,
            mem_restart_mb: 0,
            mem_throttle: MemThrottle::default(),
            trim_every_min: 0,
            auto_spread: false,
            fair_cap: false,
            adaptive: Adaptive::default(),
            ultra_cpu: UltraCpu::default(),
            bandwidth_kbps: 0,
            anti_afk: AntiAfk::default(),
            mute_clients: false,
            discord: DiscordEvents::default(),
            auto_update: false,
            self_update: false,
            minimize_on_launch: false,
            live_hidden_previews: true,
            panic_hotkey: String::new(),
            clean_logs: false,
            fps_cap: 0,
            low_graphics: true,
            lighter_rendering: true,
            render_flags: RenderFlags::default(),
            micro_width: 320,
            micro_height: 240,
            tiny_on_launch: false,
            auto_tile: false,
            allow_public_networks: true,
        }
    }
}

impl Settings {
    pub fn upgrade(&mut self) -> bool {
        if self.adaptive.untouched() {
            self.adaptive = Adaptive::default();
            return true;
        }
        false
    }
}

#[derive(Deserialize, TS, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SettingsPatch {
    #[ts(optional)]
    pub theme: Option<Theme>,
    #[ts(optional)]
    pub tab_layout: Option<TabLayout>,
    #[ts(optional)]
    pub accent: Option<String>,
    #[ts(optional)]
    pub background_dim: Option<u8>,
    #[ts(optional)]
    pub density: Option<Density>,
    #[ts(optional)]
    pub auto_lock_minutes: Option<u32>,
    #[ts(optional)]
    pub clipboard_clear_seconds: Option<u32>,
    #[ts(optional)]
    pub cache_minutes: Option<u32>,
    #[ts(optional)]
    pub protect_shared_login: Option<bool>,
    #[ts(optional)]
    pub launch_delay_ms: Option<u32>,
    #[ts(optional)]
    pub auto_rejoin: Option<bool>,
    #[ts(optional)]
    pub auto_restart_on_crash: Option<bool>,
    #[ts(optional)]
    pub breaker_max: Option<u32>,
    #[ts(optional)]
    pub breaker_window_min: Option<u32>,
    #[ts(optional)]
    pub close_grace_s: Option<u32>,
    #[ts(optional)]
    pub launch_args: Option<String>,
    #[ts(optional)]
    pub close_to_tray: Option<bool>,
    #[ts(optional)]
    pub skip_splash: Option<bool>,
    #[ts(optional)]
    pub mem_restart_mb: Option<u32>,
    #[ts(optional)]
    pub mem_throttle: Option<MemThrottle>,
    #[ts(optional)]
    pub trim_every_min: Option<u32>,
    #[ts(optional)]
    pub auto_spread: Option<bool>,
    #[ts(optional)]
    pub fair_cap: Option<bool>,
    #[ts(optional)]
    pub adaptive: Option<Adaptive>,
    #[ts(optional)]
    pub ultra_cpu: Option<UltraCpu>,
    #[ts(optional)]
    pub anti_afk: Option<AntiAfk>,
    #[ts(optional)]
    pub mute_clients: Option<bool>,
    #[ts(optional)]
    pub discord: Option<DiscordEvents>,
    #[ts(optional)]
    pub auto_update: Option<bool>,
    #[ts(optional)]
    pub self_update: Option<bool>,
    #[ts(optional)]
    pub minimize_on_launch: Option<bool>,
    #[ts(optional)]
    pub live_hidden_previews: Option<bool>,
    #[ts(optional)]
    pub panic_hotkey: Option<String>,
    #[ts(optional)]
    pub clean_logs: Option<bool>,
    #[ts(optional)]
    pub fps_cap: Option<u32>,
    #[ts(optional)]
    pub low_graphics: Option<bool>,
    #[ts(optional)]
    pub lighter_rendering: Option<bool>,
    #[ts(optional)]
    pub render_flags: Option<RenderFlags>,
    #[ts(optional)]
    pub micro_width: Option<u32>,
    #[ts(optional)]
    pub micro_height: Option<u32>,
    #[ts(optional)]
    pub tiny_on_launch: Option<bool>,
    #[ts(optional)]
    pub auto_tile: Option<bool>,
    #[ts(optional)]
    pub allow_public_networks: Option<bool>,
}

#[derive(Serialize, TS, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct LogsCleaned {
    pub files: u32,
    #[ts(type = "number")]
    pub bytes: u64,
}

#[derive(Serialize, Deserialize, TS, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
#[ts(export)]
pub enum Subscription {
    None,
    Premium,
    Plus,
}

#[derive(Serialize, Deserialize, TS, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum PresenceKind {
    Offline,
    Online,
    InGame,
    Studio,
    Unknown,
}

impl PresenceKind {
    pub fn from_code(code: i64) -> Self {
        match code {
            0 => Self::Offline,
            1 => Self::Online,
            2 => Self::InGame,
            3 => Self::Studio,
            other => {
                log::debug!("unrecognised presence code {other}");
                Self::Unknown
            }
        }
    }
}

#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Presence {
    pub kind: PresenceKind,
    pub last_location: Option<String>,
    pub place_id: Option<i64>,
    pub root_place_id: Option<i64>,
    pub universe_id: Option<i64>,
    pub game_id: Option<String>,
}

#[derive(Serialize, TS, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct AccountPresence {
    pub account_id: i64,
    pub presence: Presence,
}

#[derive(Serialize, TS, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Headshot {
    pub account_id: i64,
    pub url: String,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct AccountInfo {
    pub account_id: i64,
    pub user_id: i64,
    pub username: String,
    pub display_name: String,
    pub description: String,
    pub created: Option<String>,
    pub is_banned: bool,
    pub verified_badge: bool,
    pub headshot_url: Option<String>,
    pub robux: Option<i64>,
    pub subscription: Option<Subscription>,
    pub friends: Option<i64>,
    pub followers: Option<i64>,
    pub following: Option<i64>,
    pub email_verified: Option<bool>,
    pub two_step_methods: Option<Vec<String>>,
    pub presence: Option<Presence>,
    pub fetched_at: i64,
    pub unavailable: Vec<String>,
}

#[derive(Deserialize, TS, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
#[ts(export)]
pub enum ConflictPolicy {
    Skip,
    Replace,
}

#[derive(Serialize, TS, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ImportReport {
    pub added: u32,
    pub updated: u32,
    pub skipped: u32,
    pub failed: u32,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
#[ts(export)]
pub enum LaunchTarget {
    Place { place_id: i64, launch_data: Option<String> },
    Server { place_id: i64, job_id: String },
    PrivateLink { place_id: i64, link_code: String },
    PrivateAccess { place_id: i64, access_code: String },
    ShareLink { code: String, link_type: String },
    ShareLinkV2 { code: String },
    Universe { universe_id: i64 },
    FollowUser { user_id: i64 },
}

#[derive(Serialize, TS, Clone, Debug, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
#[ts(export)]
pub enum ParsedInput {
    Target { target: LaunchTarget },
    JobIdOnly { job_id: String },
    Username { name: String },
    Invalid { reason: String },
    Unsupported { reason: String },
}

#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Favourite {
    pub id: i64,
    pub name: String,
    pub target: LaunchTarget,
    pub account_ids: Vec<i64>,
    pub in_tray: bool,
    pub sort_order: i64,
    pub last_used_at: Option<i64>,
}

#[derive(Deserialize, TS, Clone, Debug)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct FavouriteInput {
    #[ts(optional)]
    pub id: Option<i64>,
    pub name: String,
    pub target: LaunchTarget,
    pub account_ids: Vec<i64>,
    pub in_tray: bool,
}

#[derive(Serialize, TS, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct JoinCandidate {
    pub user_id: i64,
    pub username: String,
    pub display_name: String,
    pub account_id: Option<i64>,
    pub presence: Presence,
}

#[derive(Serialize, TS, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct UserRef {
    pub user_id: i64,
    pub username: String,
    pub display_name: String,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct LaunchInfo {
    pub account_id: i64,
    pub pid: u32,
    pub browser_tracker_id: i64,
    pub place_id: Option<i64>,
    pub started_at: i64,
    pub via_bootstrapper: bool,
    pub warning: Option<String>,
    pub fps_cap: Option<u32>,
    #[serde(skip)]
    #[ts(skip)]
    pub pid_start: Option<u64>,
}

#[derive(Serialize, Deserialize, TS, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum InstanceState {
    Starting,
    Joining,
    InGame,
    Disconnected,
    Exited,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Instance {
    pub id: String,
    pub account_id: i64,
    pub pid: u32,
    pub state: InstanceState,
    pub target: LaunchTarget,
    pub place_id: Option<i64>,
    pub job_id: Option<String>,
    pub started_at: i64,
    pub ended_at: Option<i64>,
    pub via_bootstrapper: bool,
    pub note: Option<String>,
    pub rejoins: u32,
    pub limits: Limits,
    pub anti_afk: bool,
    #[serde(default)]
    pub muted: bool,
    pub afk_at: Option<i64>,
    pub throttled: bool,
    pub micro: bool,
    pub fps_cap: Option<u32>,
    #[serde(default, skip_deserializing)]
    pub relaunching: bool,
    #[serde(default, skip_deserializing)]
    pub relaunch_problem: Option<String>,
    #[serde(default)]
    pub sign_in_problem: bool,
    #[serde(default, skip_deserializing)]
    pub applied_limits: Limits,
    #[serde(default, skip_deserializing)]
    pub checking_process: bool,
    #[serde(default, skip_deserializing)]
    pub cpu_mode: CpuMode,
    #[serde(default, skip_deserializing)]
    pub cpu_warmup_remaining_s: u32,
}

#[derive(Serialize, Deserialize, TS, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase", default)]
#[ts(export)]
pub struct Limits {
    pub cpu_cap: Option<u8>,
    pub eco: bool,
    pub trim_every_min: Option<u32>,
    #[ts(type = "number | null")]
    pub affinity: Option<u64>,
}

#[derive(Serialize, TS, Clone, Copy, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SystemUsage {
    pub cpu_pct: Option<f64>,
    pub mem_used_mb: f64,
    pub mem_total_mb: f64,
}

#[derive(Serialize, TS, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ClientMetrics {
    pub instance_id: String,
    pub pid: u32,
    pub cpu_pct: Option<f64>,
    pub memory_mb: f64,
    pub private_mb: f64,
    pub read_bps: Option<f64>,
    pub write_bps: Option<f64>,
    pub gpu_pct: Option<f64>,
    pub gpu_mem_mb: Option<f64>,
}

#[derive(Serialize, TS, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct BandwidthStatus {
    pub kbps: u32,
    pub active: bool,
}

#[derive(Serialize, TS, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct MultiInstanceStatus {
    pub enabled: bool,
    pub active: bool,
    pub protect_shared_login: bool,
    pub blocked_by: Vec<u32>,
    pub message: Option<String>,
}

#[derive(Serialize, Deserialize, TS, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum SameServer {
    #[default]
    Off,
    PickFirst,
    FollowFirst,
}

#[derive(Deserialize, TS, Clone, Debug)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct BulkRequest {
    pub account_ids: Vec<i64>,
    pub target: LaunchTarget,
    pub delay_ms: u32,
    pub same_server: SameServer,
    pub favourite_id: Option<i64>,
    #[serde(default)]
    pub fps_cap: Option<u32>,
}

#[derive(Serialize, TS, Clone, Debug, PartialEq)]
#[serde(tag = "kind", rename_all = "camelCase")]
#[ts(export)]
pub enum BulkStage {
    Queued,
    Ticket,
    Starting,
    Joining,
    InGame,
    Done,
    Failed { message: String },
    Cancelled,
}

#[derive(Serialize, TS, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct BulkItem {
    pub account_id: i64,
    pub user_id: i64,
    pub stage: BulkStage,
    pub instance_id: Option<String>,
}

#[derive(Serialize, TS, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct BulkProgress {
    pub batch_id: String,
    pub target: LaunchTarget,
    pub same_server: SameServer,
    pub delay_ms: u32,
    pub job_id: Option<String>,
    pub items: Vec<BulkItem>,
    pub stopped: Option<String>,
    pub done: bool,
    pub note: Option<String>,
}

#[derive(Serialize, Deserialize, TS, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum ServerOrder {
    OccupancyDesc,
    OccupancyAsc,
    Recommended,
    BestLatency,
}

#[derive(Serialize, TS, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct GameServer {
    pub id: String,
    pub playing: i32,
    pub max_players: i32,
    pub fps: Option<f64>,
    pub ping: Option<i64>,
    pub friend_count: Option<i32>,
}

#[derive(Serialize, TS, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ServerPage {
    pub servers: Vec<GameServer>,
    pub next_cursor: Option<String>,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct GameInfo {
    pub place_id: i64,
    pub universe_id: i64,
    pub root_place_id: i64,
    pub name: String,
    pub creator: String,
    pub playing: i64,
    pub visits: i64,
    pub max_players: i32,
    pub icon_url: Option<String>,
    pub private_servers: bool,
}

#[derive(Serialize, TS, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PrivateServer {
    pub vip_server_id: i64,
    pub name: String,
    pub owner_id: Option<i64>,
    pub owner_name: Option<String>,
    pub playing: i32,
    pub max_players: i32,
    pub job_id: Option<String>,
    pub access_code: Option<String>,
}

#[derive(Serialize, TS, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PrivateServerPage {
    pub servers: Vec<PrivateServer>,
    pub next_cursor: Option<String>,
    pub join_restricted: bool,
}

#[derive(Serialize, TS, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct RobloxClient {
    pub path: Option<String>,
    pub bootstrapper: bool,
    pub version: Option<String>,
    pub latest: Option<String>,
    pub outdated: bool,
}

#[derive(Serialize, TS, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct AppUpdate {
    pub current: String,
    pub available: Option<String>,
}

#[derive(Serialize, TS, Clone, Debug, PartialEq)]
#[serde(tag = "stage", rename_all = "camelCase")]
#[ts(export)]
pub enum InstallProgress {
    Downloading { percent: u8 },
    Verifying,
    Installing,
    Done,
    Error { message: String },
}

#[derive(Serialize, TS, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct DataDir {
    pub path: String,
    pub default: bool,
    pub overridden: bool,
}

#[derive(Serialize, Deserialize, TS, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum CrashKind {
    Crash,
    Disconnect,
    Hang,
    Mem,
}

impl CrashKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Crash => "crash",
            Self::Disconnect => "disconnect",
            Self::Hang => "hang",
            Self::Mem => "mem",
        }
    }

    pub fn parse(s: &str) -> Self {
        match s {
            "disconnect" => Self::Disconnect,
            "hang" => Self::Hang,
            "mem" => Self::Mem,
            "crash" => Self::Crash,
            other => {
                log::debug!("unrecognised crash kind {other:?}");
                Self::Crash
            }
        }
    }
}

#[derive(Serialize, TS, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ServerVisit {
    pub place_id: i64,
    pub job_id: String,
    pub private: bool,
    #[ts(type = "number")]
    pub joined_at: i64,
}

#[derive(Serialize, TS, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Crash {
    pub id: i64,
    pub instance_id: String,
    pub account_id: i64,
    pub kind: CrashKind,
    pub at: i64,
    pub uptime_ms: i64,
    pub reason: Option<String>,
    pub place_id: Option<i64>,
    pub has_log: bool,
}

#[derive(Serialize, TS, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Notice {
    pub error: bool,
    pub message: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn group(id: i64, name: &str, sort_order: i64) -> Group {
        Group { id, name: name.into(), color: None, sort_order }
    }

    #[test]
    fn numbered_groups_come_first_by_their_number() {
        let mut all = [
            group(1, "Farm", 0),
            group(2, "007 Bank", 1),
            group(3, "PvP", 2),
            group(4, "1Main", 3),
            group(5, "12 trade", 4),
        ];
        all.sort_by(group_order);
        let names: Vec<&str> = all.iter().map(|g| g.name.as_str()).collect();
        assert_eq!(names, ["1Main", "007 Bank", "12 trade", "Farm", "PvP"]);
    }

    #[test]
    fn the_old_untouched_manager_is_taken_along_and_a_choice_is_left_alone() {
        let mut old = Settings { adaptive: Adaptive { enabled: false, background: Limits { cpu_cap: Some(25), eco: true, ..Limits::default() } }, ..Settings::default() };
        assert!(old.upgrade());
        assert_eq!(old.adaptive, Adaptive::default());
        assert!(!old.upgrade(), "nothing left to do");
        let mut off = Settings { adaptive: Adaptive { enabled: false, background: Adaptive::default().background }, ..Settings::default() };
        assert!(!off.upgrade(), "turned off by hand");
        assert!(!off.adaptive.enabled);
        assert!(Adaptive::default().enabled && Adaptive::default().background.cpu_cap.is_none(), "on, and no cap out of the box");
    }

    #[test]
    fn a_number_that_does_not_fit_is_no_number() {
        assert_eq!(number_prefix("1Main"), (Some(1), "Main"));
        assert_eq!(number_prefix("007  Bank"), (Some(7), "Bank"));
        assert_eq!(number_prefix("Bank 2"), (None, "Bank 2"));
        assert_eq!(number_prefix("99999999999999 X").0, Some(u32::MAX));
    }
}
