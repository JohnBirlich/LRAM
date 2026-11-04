use serde::Deserialize;

#[derive(Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AuthenticatedUser {
    pub id: i64,
    pub name: String,
    pub display_name: String,
}

#[derive(Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct UserDetails {
    pub id: i64,
    pub name: String,
    pub display_name: String,
    #[serde(default)]
    pub description: String,
    pub created: Option<String>,
    #[serde(default)]
    pub is_banned: bool,
    #[serde(default)]
    pub has_verified_badge: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SocialCounts {
    pub friends: i64,
    pub followers: i64,
    pub following: i64,
}

#[derive(Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct UserPresence {
    pub user_id: i64,
    pub user_presence_type: i64,
    pub last_location: Option<String>,
    pub place_id: Option<i64>,
    pub root_place_id: Option<i64>,
    pub game_id: Option<String>,
    pub universe_id: Option<i64>,
}

#[derive(Deserialize, Debug)]
pub struct Data<T> {
    pub data: Vec<T>,
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Game {
    pub id: Option<i64>,
    pub root_place_id: Option<i64>,
    pub name: Option<String>,
    pub creator: Option<Party>,
    pub playing: Option<i64>,
    pub visits: Option<i64>,
    pub max_players: Option<i32>,
    pub create_vip_servers_allowed: Option<bool>,
}

#[derive(Deserialize, Debug)]
pub struct Party {
    pub id: Option<i64>,
    pub name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct GameDetails {
    pub universe_id: i64,
    pub root_place_id: i64,
    pub name: String,
    pub creator: String,
    pub playing: i64,
    pub visits: i64,
    pub max_players: i32,
    pub private_servers: bool,
}

#[derive(Deserialize, Debug)]
pub struct Thumbnails {
    pub data: Vec<Thumbnail>,
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Thumbnail {
    pub target_id: i64,
    pub state: String,
    pub image_url: Option<String>,
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Profiles {
    #[serde(default)]
    pub profile_details: Vec<Profile>,
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Profile {
    pub user_id: i64,
    pub has_roblox_subscription: Option<bool>,
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Presences {
    pub user_presences: Vec<UserPresence>,
}

#[derive(Deserialize, Debug)]
pub struct TwoStepConfig {
    #[serde(default)]
    pub methods: Vec<TwoStepMethod>,
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct TwoStepMethod {
    pub media_type: serde_json::Value,
    #[serde(default)]
    pub enabled: bool,
}

#[derive(Deserialize, Debug, Clone, Default, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct ShareLinkResolution {
    pub private_server_invite_data: Option<ShareTarget>,
    pub experience_invite_data: Option<ShareTarget>,
    pub notification_experience_invite_data: Option<ShareTarget>,
    pub screenshot_invite_data: Option<ShareTarget>,
    pub experience_details_invite_data: Option<ShareTarget>,
    pub experience_affiliate_data: Option<ShareTarget>,
    pub experience_event_data: Option<ShareTarget>,
    pub profile_link_resolution_response_data: Option<ShareTarget>,
}

#[derive(Deserialize, Debug, Clone, Default, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct ShareTarget {
    pub status: Option<String>,
    pub place_id: Option<i64>,
    pub universe_id: Option<i64>,
    pub link_code: Option<String>,
    pub instance_id: Option<String>,
    pub launch_data: Option<String>,
    pub user_id: Option<i64>,
    pub join_data: Option<serde_json::Value>,
}

#[derive(Deserialize, Debug, Clone, Default, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct ShareLinkV2 {
    pub link_type: Option<String>,
    pub status: Option<String>,
    pub target_id: Option<i64>,
    pub custom_data: Option<serde_json::Value>,
}

#[derive(Deserialize, Debug, Clone, PartialEq)]
pub struct FriendOnline {
    pub id: i64,
    #[serde(default)]
    pub name: String,
    #[serde(default, rename = "displayName")]
    pub display_name: String,
    #[serde(rename = "userPresence")]
    pub presence: Option<FriendPresence>,
}

#[derive(Deserialize, Debug, Clone, Default, PartialEq)]
pub struct FriendRequestRow {
    #[serde(default)]
    pub id: Option<i64>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default, rename = "displayName")]
    pub display_name: Option<String>,
    #[serde(default, rename = "senderUserId")]
    pub sender_user_id: Option<i64>,
    #[serde(default, rename = "requesterUserId")]
    pub requester_user_id: Option<i64>,
}

impl FriendRequestRow {
    pub fn user_id(&self) -> Option<i64> {
        self.id.or(self.sender_user_id).or(self.requester_user_id)
    }
}

#[derive(Deserialize, Debug, Clone, Default, PartialEq)]
#[serde(rename_all = "PascalCase")]
pub struct FriendPresence {
    pub user_presence_type: Option<String>,
    #[serde(rename = "lastLocation")]
    pub last_location: Option<String>,
    #[serde(rename = "placeId")]
    pub place_id: Option<i64>,
    #[serde(rename = "rootPlaceId")]
    pub root_place_id: Option<i64>,
    #[serde(rename = "gameInstanceId")]
    pub game_instance_id: Option<String>,
    #[serde(rename = "universeId")]
    pub universe_id: Option<i64>,
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ServerList {
    pub data: Option<Vec<ServerRow>>,
    pub next_page_cursor: Option<String>,
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ServerRow {
    pub id: Option<String>,
    pub max_players: Option<i32>,
    pub playing: Option<i32>,
    pub fps: Option<f64>,
    pub ping: Option<i64>,
    pub friend_count: Option<i32>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PrivateServerList {
    pub data: Option<Vec<PrivateServerRow>>,
    pub next_page_cursor: Option<String>,
    pub game_join_restricted: Option<bool>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PrivateServerRow {
    pub vip_server_id: Option<i64>,
    pub name: Option<String>,
    pub owner: Option<Party>,
    pub playing: Option<i32>,
    pub max_players: Option<i32>,
    pub id: Option<String>,
    pub access_code: Option<String>,
}
