//! What a front end draws, derived from the SDK's room list and timelines: room rows and message rows with their formatting, replies,
//! reactions and pictures already worked out.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use matrix_sdk::{
    media::{MediaFormat, MediaRequestParameters},
    ruma::{
        events::room::message::{MessageType, RoomMessageEventContent, RoomMessageEventContentWithoutRelation},
        html::{sanitize_html, HtmlSanitizerMode, RemoveReplyFallback},
        OwnedEventId,
    },
    Client,
};
use matrix_sdk_ui::timeline::{EventTimelineItem, Timeline, TimelineDetails, TimelineEventItemId, TimelineItem, TimelineItemContent};
use serde::Serialize;

/// One row of the room list as the UI draws it.
#[derive(Debug, Clone, PartialEq, Serialize, Default)]
pub struct UiRoom { pub space_id: String, pub id: String, pub title: String, pub section: String, pub unread: u32, pub highlight: bool, pub invite: bool, pub favourite: bool, pub low_priority: bool, pub notify: String, pub avatar_mxc: String, pub avatar_path: String }

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct UiReaction { pub key: String, pub count: u32, pub mine: bool }

/// The message a reply answers.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct UiReply { pub event_id: String, pub sender: String, pub preview: String }

fn is_false(b: &bool) -> bool { !*b }
fn is_zero32(n: &u32) -> bool { *n == 0 }
fn is_zero64(n: &u64) -> bool { *n == 0 }

/// One message of a timeline as the UI draws it. A field that has its default (empty, false, 0, none) is left out of the JSON, and the UI reads a missing field
/// as that default: of 29 fields a plain text message has 8, which is a third of the JSON and of what the UI holds per message. Only the first seven are always sent.
#[derive(Debug, Clone, PartialEq, Serialize, Default)]
pub struct UiMessage {
    pub id: String,
    pub sender: String,
    pub sender_id: String,
    /// plain text (also what a UI without rich text shows)
    pub body: String,
    /// sanitized HTML of a formatted message, empty for plain text
    #[serde(skip_serializing_if = "String::is_empty")]
    pub html: String,
    /// "text", "emote", "notice", "image", "file", "video", "audio", "location", "undecryptable", "other"
    pub kind: String,
    pub time: String,
    /// milliseconds since the epoch (sorting; `time` is only the clock for display)
    pub ts: u64,
    #[serde(skip_serializing_if = "is_false")]
    pub own: bool,
    #[serde(skip_serializing_if = "is_false")]
    pub edited: bool,
    #[serde(skip_serializing_if = "is_false")]
    pub pending: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reply: Option<UiReply>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub reactions: Vec<UiReaction>,
    /// a picture: where the cached file is once it has been downloaded (empty before), and its size
    #[serde(skip_serializing_if = "String::is_empty")]
    pub image_path: String,
    /// `image_path` is a smaller version of the picture (a thumbnail), not the picture itself: opening it has to fetch the original
    #[serde(skip_serializing_if = "is_false")]
    pub image_thumb: bool,
    #[serde(skip_serializing_if = "is_zero32")]
    pub image_w: u32,
    #[serde(skip_serializing_if = "is_zero32")]
    pub image_h: u32,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub file_name: String,
    /// a file whose contents can be shown as text (by its type or name): the UI offers "Open file"
    #[serde(skip_serializing_if = "is_false")]
    pub text_file: bool,
    #[serde(skip_serializing_if = "is_zero64")]
    pub size: u64,
    /// display names of other people whose newest read receipt is on this message
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub seen_by: Vec<String>,
    /// replies in the thread that starts at this message (0: no thread)
    #[serde(skip_serializing_if = "is_zero32")]
    pub thread_replies: u32,
    /// the card of the first link (filled in by the app when previews are on)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preview: Option<UiPreview>,
    /// who-wrote-this warning for encrypted messages
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shield: Option<UiShield>,
    /// pinned in the room (set by the app from the room's state)
    #[serde(skip_serializing_if = "is_false")]
    pub pinned: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub poll: Option<UiPoll>,
    /// a gallery (several pictures or files sent as one message, MSC4274): its items in order; `body` is the caption
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub gallery: Vec<UiGalleryItem>,
    /// the sender's picture: a file the app downloaded (set by the app, empty until then)
    #[serde(skip_serializing_if = "String::is_empty")]
    pub avatar_path: String,
    /// custom emoji pictures inside `html` (`mxc://` address and, once the app has downloaded it, the file)
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub emoji: Vec<UiEmojiRef>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Default)]
pub struct UiEmojiRef { pub mxc: String, pub path: String }

/// The other person's picture in a direct chat: from the room's heroes, else from the member event of the person it was made with,
/// else from their profile (asked once per person; bridged chats often lack the hero's picture).
async fn direct_avatar(client: &Client, r: &matrix_sdk::Room) -> Option<matrix_sdk::ruma::OwnedMxcUri> {
    use std::sync::Mutex;
    static PROFILES: Mutex<Option<HashMap<String, Option<matrix_sdk::ruma::OwnedMxcUri>>>> = Mutex::new(None);
    if let Some(a) = r.heroes().await.into_iter().find_map(|h| h.avatar_url) { return Some(a); }
    let me = client.user_id().map(|u| u.to_owned());
    let mut people: Vec<matrix_sdk::ruma::OwnedUserId> = r.direct_targets().into_iter().filter_map(|t| t.as_user_id().map(|u| u.to_owned())).collect();
    people.retain(|u| Some(u) != me.as_ref());
    let who = people.into_iter().next()?;
    if let Ok(Some(m)) = r.get_member_no_sync(&who).await { if let Some(a) = m.avatar_url() { return Some(a.to_owned()); } }
    let key = who.to_string();
    if let Some(known) = PROFILES.lock().unwrap().get_or_insert_with(HashMap::new).get(&key) { return known.clone(); }
    let got = client.account().fetch_user_profile_of(&who).await.ok().and_then(|p| p.get_static::<matrix_sdk::ruma::api::client::profile::AvatarUrl>().ok().flatten());
    PROFILES.lock().unwrap().get_or_insert_with(HashMap::new).insert(key, got.clone());
    got
}

/// The joined rooms, grouped the way the sidebar shows them.
pub async fn ui_rooms(client: &Client) -> Vec<UiRoom> {
    let mut out = Vec::new();
    let settings = client.notification_settings().await;
    for r in client.joined_rooms() {
        if r.is_space() { continue; } /* spaces are sections, not conversations */
        let title = r.display_name().await.map(|n| n.to_string()).unwrap_or_else(|_| r.room_id().to_string());
        let direct = r.is_direct().await.unwrap_or(false);
        /* a direct chat shows the other person's picture when it has none of its own */
        let avatar = match r.avatar_url() { Some(a) => Some(a), None if direct => direct_avatar(client, &r).await, None => None };
        out.push(UiRoom {
            id: r.room_id().to_string(),
            title,
            section: if direct { "Direct Messages" } else { "Rooms" }.into(),
            unread: r.num_unread_notifications() as u32,
            highlight: r.num_unread_mentions() > 0,
            invite: false,
            favourite: r.is_favourite(),
            low_priority: r.is_low_priority(),
            notify: notify_name(settings.get_user_defined_room_notification_mode(r.room_id()).await).into(),
            avatar_mxc: avatar.map(|u| u.to_string()).unwrap_or_default(),
            ..Default::default()
        });
    }
    for r in client.invited_rooms() {
        let title = r.display_name().await.map(|n| n.to_string()).unwrap_or_else(|_| r.room_id().to_string());
        out.push(UiRoom { id: r.room_id().to_string(), title, section: "Invites".into(), invite: true, avatar_mxc: r.avatar_url().map(|u| u.to_string()).unwrap_or_default(), ..Default::default() });
    }
    out.sort_by(|a, b| (a.section != "Invites", a.section != "Direct Messages", a.title.to_lowercase()).cmp(&(b.section != "Invites", b.section != "Direct Messages", b.title.to_lowercase())));
    out
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct UiMember { pub user_id: String, pub name: String, pub role: String, pub can_kick: bool, pub can_ban: bool, pub verified: bool, pub avatar_mxc: String, pub avatar_path: String }

/// What the room details panel shows. `members` is everybody when `members_complete`; otherwise only the people the sync has brought (the ones who have
/// written lately, see `crate::sync_settings`) and `member_count` is how many there are in all.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct UiRoomDetails { pub id: String, pub name: String, pub topic: String, pub encrypted: bool, pub can_edit: bool, pub can_set_roles: bool, pub can_pin: bool, pub can_invite: bool, pub member_count: u64, pub members_complete: bool, pub members: Vec<UiMember>, pub banned: Vec<UiMember> }

/// The name to show for a room member; one that another member also has comes with the user id (spec: "Calculating the display name for a user").
fn member_name(m: &matrix_sdk::room::RoomMember) -> String {
    match m.display_name() {
        Some(n) if m.name_ambiguous() => format!("{n} ({})", m.user_id()),
        Some(n) => n.to_string(),
        None => m.user_id().localpart().to_string(),
    }
}

/// A room with at most this many people lists all of them as soon as it is opened (the member list is one request); a bigger one lists the people the sync has
/// brought until the member list is opened (`load_members`): a room of 20 000 people is 10 MB of JSON, as many rows in the UI and a minute of waiting at every first open.
pub const MEMBERS_AT_OPEN: u64 = 1000;

/// Name, topic, encryption and the joined members of a room as it is opened: everybody, unless it is a big room whose member list has not been loaded.
pub async fn room_details(client: &Client, room_id: &str) -> Result<UiRoomDetails, String> { room_details_for(client, room_id, false, MEMBERS_AT_OPEN).await }

/// `room_details`, asking for the whole member list when `everybody` or when the room has at most `up_to` people (the sync only brings the ones it needs).
pub async fn room_details_for(client: &Client, room_id: &str, everybody: bool, up_to: u64) -> Result<UiRoomDetails, String> {
    use matrix_sdk::RoomMemberships;
    let rid = <&matrix_sdk::ruma::RoomId>::try_from(room_id).map_err(|e| e.to_string())?;
    let room = client.get_room(rid).ok_or("unknown room")?;
    let fetch = everybody || room.joined_members_count() <= up_to;
    let name = room.display_name().await.map(|n| n.to_string()).unwrap_or_else(|_| room_id.to_string());
    let encrypted = room.latest_encryption_state().await.map(|s| s.is_encrypted()).unwrap_or(false);
    let me = client.user_id().ok_or("not signed in")?.to_owned();
    let levels = room.power_levels().await.ok();
    let allowed = |f: &dyn Fn(&matrix_sdk::ruma::events::room::power_levels::RoomPowerLevels) -> bool| levels.as_ref().map(f).unwrap_or(false);
    let joined = if fetch { room.members(RoomMemberships::JOIN).await } else { room.members_no_sync(RoomMemberships::JOIN).await };
    let mut members: Vec<UiMember> = joined.map_err(|e| e.to_string())?.iter().map(|m| UiMember {
        user_id: m.user_id().to_string(),
        name: member_name(m),
        role: match m.suggested_role_for_power_level() { matrix_sdk::room::RoomMemberRole::Creator => "Creator", matrix_sdk::room::RoomMemberRole::Administrator => "Admin", matrix_sdk::room::RoomMemberRole::Moderator => "Moderator", _ => "" }.into(),
        can_kick: m.user_id() != me && allowed(&|l| l.user_can_kick_user(&me, m.user_id())),
        can_ban: m.user_id() != me && allowed(&|l| l.user_can_ban_user(&me, m.user_id())),
        verified: false,
        avatar_mxc: m.avatar_url().map(|u| u.to_string()).unwrap_or_default(),
        avatar_path: String::new(),
    }).collect();
    for m in members.iter_mut() { m.verified = crate::crypto::user_verified(client, &m.user_id).await; }
    members.sort_by(|a, b| (a.role.is_empty(), a.name.to_lowercase()).cmp(&(b.role.is_empty(), b.name.to_lowercase())));
    use matrix_sdk::ruma::events::StateEventType;
    let can_edit = allowed(&|l| l.user_can_send_state(&me, StateEventType::RoomName) && l.user_can_send_state(&me, StateEventType::RoomTopic));
    let can_invite = levels.as_ref().map(|l| l.user_can_invite(&me)).unwrap_or(false);
    let can_pin = allowed(&|l| l.user_can_send_state(&me, StateEventType::RoomPinnedEvents));
    let can_set_roles = allowed(&|l| l.user_can_send_state(&me, StateEventType::RoomPowerLevels));
    let banned = if fetch { room.members(RoomMemberships::BAN).await } else { room.members_no_sync(RoomMemberships::BAN).await };
    let banned: Vec<UiMember> = banned.map(|v| v.iter().map(|m| UiMember {
        user_id: m.user_id().to_string(),
        name: member_name(m),
        role: String::new(), can_kick: false, can_ban: allowed(&|l| l.user_can_ban(&me)), verified: false, avatar_mxc: m.avatar_url().map(|u| u.to_string()).unwrap_or_default(), avatar_path: String::new(),
    }).collect()).unwrap_or_default();
    let members_complete = fetch || room.are_members_synced();
    let member_count = if members_complete { members.len() as u64 } else { room.joined_members_count().max(members.len() as u64) };
    Ok(UiRoomDetails { id: room_id.into(), name, topic: room.topic().unwrap_or_default(), encrypted, can_edit, can_set_roles, can_pin, can_invite, member_count, members_complete, members, banned })
}

/// Online status of people: user id -> "online", "unavailable" (away) or "offline". People the server will not tell about are left out
/// (servers often have presence switched off); at most 150 are asked, all at once.
pub async fn presence_of(client: &Client, users: &[String]) -> HashMap<String, String> {
    use matrix_sdk::ruma::{api::client::presence::get_presence, presence::PresenceState, UserId};
    let asks = users.iter().take(150).filter_map(|u| <&UserId>::try_from(u.as_str()).ok().map(|id| id.to_owned())).map(|id| async move {
        let r = client.send(get_presence::v3::Request::new(id.clone())).await.ok()?;
        let state = match r.presence { PresenceState::Online => "online", PresenceState::Unavailable => "unavailable", PresenceState::Offline => "offline", _ => return None };
        Some((id.to_string(), state.to_string()))
    });
    futures_util::future::join_all(asks).await.into_iter().flatten().collect()
}

/// Tell the server how we are: "online", "unavailable" or "offline".
pub async fn set_own_presence(client: &Client, state: &str) -> Result<(), String> {
    use matrix_sdk::ruma::{api::client::presence::set_presence, presence::PresenceState};
    let me = client.user_id().ok_or("not signed in")?.to_owned();
    let st = match state { "online" => PresenceState::Online, "unavailable" => PresenceState::Unavailable, "offline" => PresenceState::Offline, o => return Err(format!("unknown status {o}")) };
    client.send(set_presence::v3::Request::new(me, st)).await.map(|_| ()).map_err(|e| e.to_string())
}

/// A room that got new unread messages since the last look: what a desktop notification should say.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct UiAlert { pub room_id: String, pub title: String, pub new: u32, pub highlight: bool }

/// Compare the rooms with the unread counts seen last time (`seen` is updated). The room that is open in a focused window never alerts;
/// the first call only learns the counts, so starting the app does not announce everything that was already unread.
pub fn new_alerts(seen: &mut Option<HashMap<String, u32>>, rooms: &[UiRoom], open_and_focused: Option<&str>) -> Vec<UiAlert> {
    let now: HashMap<String, u32> = rooms.iter().filter(|r| !r.invite).map(|r| (r.id.clone(), r.unread)).collect();
    let mut out = Vec::new();
    if let Some(before) = seen.as_ref() {
        for r in rooms.iter().filter(|r| !r.invite) {
            let old = before.get(&r.id).copied().unwrap_or(0);
            if r.unread > old && open_and_focused != Some(r.id.as_str()) {
                out.push(UiAlert { room_id: r.id.clone(), title: r.title.clone(), new: r.unread - old, highlight: r.highlight });
            }
        }
    }
    *seen = Some(now);
    out
}

fn room_of(client: &Client, id: &str) -> Result<matrix_sdk::Room, String> {
    let rid = <&matrix_sdk::ruma::RoomId>::try_from(id).map_err(|e| e.to_string())?;
    client.get_room(rid).ok_or_else(|| "unknown room".to_string())
}

fn user_of(id: &str) -> Result<matrix_sdk::ruma::OwnedUserId, String> { id.try_into().map_err(|e| format!("not a Matrix user id: {e}")) }

/// Remove a member from the room (they can come back).
pub async fn kick_member(client: &Client, room_id: &str, user_id: &str, reason: &str) -> Result<(), String> {
    room_of(client, room_id)?.kick_user(&user_of(user_id)?, Some(reason).filter(|r| !r.is_empty())).await.map_err(|e| e.to_string())
}

/// Ban a member (they cannot come back until unbanned).
pub async fn ban_member(client: &Client, room_id: &str, user_id: &str, reason: &str) -> Result<(), String> {
    room_of(client, room_id)?.ban_user(&user_of(user_id)?, Some(reason).filter(|r| !r.is_empty())).await.map_err(|e| e.to_string())
}

/// Let a banned person back (they can then be invited or join again).
pub async fn unban_member(client: &Client, room_id: &str, user_id: &str) -> Result<(), String> {
    room_of(client, room_id)?.unban_user(&user_of(user_id)?, None).await.map_err(|e| e.to_string())
}

/// Invite a person by their Matrix id.
pub async fn invite_member(client: &Client, room_id: &str, user_id: &str) -> Result<(), String> {
    room_of(client, room_id)?.invite_user_by_id(&user_of(user_id.trim())?).await.map_err(|e| e.to_string())
}

/// Change the room's name and topic (each only when given, i.e. non-empty after trimming for the name).
pub async fn set_room_texts(client: &Client, room_id: &str, name: &str, topic: &str) -> Result<(), String> {
    let room = room_of(client, room_id)?;
    if !name.trim().is_empty() && room.name().as_deref() != Some(name.trim()) { room.set_name(name.trim().to_string()).await.map_err(|e| e.to_string())?; }
    if room.topic().unwrap_or_default() != topic.trim() { room.set_room_topic(topic.trim()).await.map_err(|e| e.to_string())?; }
    Ok(())
}

/// Pin or unpin a message in its room.
pub async fn set_pinned(client: &Client, room_id: &str, event_id: &str, pinned: bool) -> Result<(), String> {
    let room = room_of(client, room_id)?;
    let id: OwnedEventId = event_id.try_into().map_err(|e| format!("bad event id: {e}"))?;
    (if pinned { room.pin_event(&id).await } else { room.unpin_event(&id).await }).map(|_| ()).map_err(|e| e.to_string())
}

/// The ids of the room's pinned messages.
pub fn pinned_ids(client: &Client, room_id: &str) -> Vec<String> {
    room_of(client, room_id).ok().and_then(|r| r.pinned_event_ids()).map(|v| v.iter().map(|e| e.to_string()).collect()).unwrap_or_default()
}

/// Give a member a role: "admin" (100), "moderator" (50) or "member" (0).
pub async fn set_member_role(client: &Client, room_id: &str, user_id: &str, role: &str) -> Result<(), String> {
    let level: i32 = match role { "admin" => 100, "moderator" => 50, "member" => 0, other => return Err(format!("unknown role {other}")) };
    let uid = user_of(user_id)?;
    room_of(client, room_id)?.update_power_levels(vec![(&uid, level.into())]).await.map(|_| ()).map_err(|e| e.to_string())
}

fn notify_name(mode: Option<matrix_sdk::notification_settings::RoomNotificationMode>) -> &'static str {
    use matrix_sdk::notification_settings::RoomNotificationMode as M;
    match mode { None => "default", Some(M::AllMessages) => "all", Some(M::MentionsAndKeywordsOnly) => "mentions", Some(M::Mute) => "mute" }
}

/// What a room's notification level is set to by the user: "default" (the account's own rules), "all", "mentions" or "mute".
pub async fn room_notification_level(client: &Client, room_id: &str) -> Result<String, String> {
    let rid = <&matrix_sdk::ruma::RoomId>::try_from(room_id).map_err(|e| e.to_string())?;
    Ok(notify_name(client.notification_settings().await.get_user_defined_room_notification_mode(rid).await).into())
}

/// Set a room's notification level (`level` as in `room_notification_level`; "default" removes the room's own rule).
pub async fn set_room_notification_level(client: &Client, room_id: &str, level: &str) -> Result<(), String> {
    use matrix_sdk::notification_settings::RoomNotificationMode as M;
    let rid = <&matrix_sdk::ruma::RoomId>::try_from(room_id).map_err(|e| e.to_string())?;
    let settings = client.notification_settings().await;
    match level {
        "default" => settings.delete_user_defined_room_rules(rid).await.map_err(|e| e.to_string()),
        "all" => settings.set_room_notification_mode(rid, M::AllMessages).await.map_err(|e| e.to_string()),
        "mentions" => settings.set_room_notification_mode(rid, M::MentionsAndKeywordsOnly).await.map_err(|e| e.to_string()),
        "mute" => settings.set_room_notification_mode(rid, M::Mute).await.map_err(|e| e.to_string()),
        other => Err(format!("unknown notification level {other}")),
    }
}

/// Mark a room as favourite / low priority (the two are exclusive: setting one clears the other). `kind`: "favourite", "low_priority" or "none".
pub async fn set_room_tag(client: &Client, room_id: &str, kind: &str) -> Result<(), String> {
    let room = room_of(client, room_id)?;
    match kind {
        "favourite" => room.set_is_favourite(true, None).await,
        "low_priority" => room.set_is_low_priority(true, None).await,
        "none" => async { if room.is_favourite() { room.set_is_favourite(false, None).await?; } if room.is_low_priority() { room.set_is_low_priority(false, None).await?; } Ok::<(), matrix_sdk::Error>(()) }.await,
        other => return Err(format!("unknown tag {other}")),
    }.map_err(|e| e.to_string())
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct UiUser { pub user_id: String, pub name: String, pub avatar_mxc: String }

/// People matching a search term in the server's user directory.
pub async fn search_users(client: &Client, term: &str) -> Result<Vec<UiUser>, String> {
    if term.trim().len() < 2 { return Ok(Vec::new()); }
    let r = client.search_users(term.trim(), 12).await.map_err(|e| e.to_string())?;
    Ok(r.results.into_iter().map(|u| UiUser { name: u.display_name.clone().unwrap_or_else(|| u.user_id.localpart().to_string()), avatar_mxc: u.avatar_url.map(|a| a.to_string()).unwrap_or_default(), user_id: u.user_id.to_string() }).collect())
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct UiPublicRoom { pub room_id: String, pub name: String, pub alias: String, pub topic: String, pub members: u64 }

/// Rooms in a server's public directory (the user's own server when `server` is empty), matching `term` when given.
pub async fn public_directory(client: &Client, term: &str, server: &str) -> Result<Vec<UiPublicRoom>, String> {
    use matrix_sdk::ruma::{api::client::directory::get_public_rooms_filtered::v3::Request, directory::Filter};
    let mut req = Request::new();
    req.limit = Some(30u32.into());
    if !term.trim().is_empty() { let mut f = Filter::new(); f.generic_search_term = Some(term.trim().to_string()); req.filter = f; }
    if !server.trim().is_empty() { req.server = Some(<&matrix_sdk::ruma::ServerName>::try_from(server.trim()).map_err(|e| format!("not a server name: {e}"))?.to_owned()); }
    let r = client.public_rooms_filtered(req).await.map_err(|e| e.to_string())?;
    Ok(r.chunk.into_iter().map(|c| UiPublicRoom { room_id: c.room_id.to_string(), name: c.name.clone().unwrap_or_default(), alias: c.canonical_alias.map(|a| a.to_string()).unwrap_or_default(), topic: c.topic.unwrap_or_default(), members: u64::from(c.num_joined_members) }).collect())
}

/// The rooms of a space as the server lists them (also the ones we have not joined): what "explore this space" shows. Rooms that are
/// themselves spaces and the space itself are left out. `alias` carries the room id when there is no address, and `joined` says whether we are in.
pub async fn space_rooms(client: &Client, space_id: &str) -> Result<Vec<UiSpaceRoom>, String> {
    use matrix_sdk::ruma::api::client::space::get_hierarchy::v1::Request;
    let rid = <&matrix_sdk::ruma::RoomId>::try_from(space_id).map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    let mut from: Option<String> = None;
    for _ in 0..10 { /* pages of the hierarchy */
        let mut req = Request::new(rid.to_owned());
        req.from = from.clone();
        req.max_depth = Some(1u32.into());
        let r = client.send(req).await.map_err(|e| e.to_string())?;
        for c in r.rooms {
            let sm = c.summary;
            if sm.room_id == rid || sm.room_type.as_ref().map(|t| t.to_string() == "m.space").unwrap_or(false) { continue; }
            let joined = client.get_room(&sm.room_id).map(|x| x.state() == matrix_sdk::RoomState::Joined).unwrap_or(false);
            out.push(UiSpaceRoom { room_id: sm.room_id.to_string(), name: sm.name.unwrap_or_default(), alias: sm.canonical_alias.map(|a| a.to_string()).unwrap_or_default(), topic: sm.topic.unwrap_or_default(), members: u64::from(sm.num_joined_members), joined });
        }
        from = r.next_batch;
        if from.is_none() { break; }
    }
    out.sort_by(|a, b| (a.joined, a.name.to_lowercase()).cmp(&(b.joined, b.name.to_lowercase())));
    Ok(out)
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct UiSpaceRoom { pub room_id: String, pub name: String, pub alias: String, pub topic: String, pub members: u64, pub joined: bool }

/// Join a room by id or address (#room:server); a room id needs the servers to try (`via`) when it is not on ours.
pub async fn join_by_address(client: &Client, address: &str) -> Result<String, String> { join_via(client, address, &[]).await }

pub async fn join_via(client: &Client, address: &str, via: &[String]) -> Result<String, String> {
    let id = <&matrix_sdk::ruma::RoomOrAliasId>::try_from(address.trim()).map_err(|e| format!("not a room id or address: {e}"))?;
    let via: Vec<matrix_sdk::ruma::OwnedServerName> = via.iter().filter_map(|v| <&matrix_sdk::ruma::ServerName>::try_from(v.as_str()).ok().map(|s| s.to_owned())).collect();
    client.join_room_by_id_or_alias(id, &via).await.map(|r| r.room_id().to_string()).map_err(|e| e.to_string())
}

/// Send a copy of a message (text, notice, emote; pictures and files by their original content) to other rooms. Returns how many were sent.
pub async fn forward_message(client: &Client, timeline: &Timeline, event_id: &str, targets: &[String]) -> Result<usize, String> {
    let items = timeline.items().await;
    let ev = items.iter().filter_map(|i| i.as_event()).find(|e| e.event_id().map(|i| i.as_str()) == Some(event_id)).ok_or("that message is not in the timeline")?;
    let m = ev.content().as_message().ok_or("only messages can be forwarded")?;
    let content = RoomMessageEventContent::new(m.msgtype().clone());
    let mut sent = 0;
    for t in targets {
        let room = room_of(client, t)?;
        if room.send(content.clone()).await.is_ok() { sent += 1; }
    }
    Ok(sent)
}

/// One picture or file of a gallery message. It is addressed as `<event id>#<index>` wherever an event id is taken (save, open).
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct UiGalleryItem { pub index: u32, pub name: String, pub kind: String, pub size: u64, pub image_path: String, pub image_w: u32, pub image_h: u32 }

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct UiRevision { pub time: String, pub ts: u64, pub text: String }

/// Every version of an edited message, oldest first.
pub async fn edit_history(timeline: &Timeline, event_id: &str) -> Result<Vec<UiRevision>, String> {
    let id: OwnedEventId = event_id.try_into().map_err(|e| format!("bad event id: {e}"))?;
    let revs = timeline.edit_revisions(&id).await.map_err(|e| e.to_string())?;
    let mut out: Vec<UiRevision> = revs.iter().map(|r| {
        let ts = r.timestamp.map(|t| u64::from(t.0)).unwrap_or(0);
        UiRevision { time: clock(ts), ts, text: describe(&r.content).1 }
    }).collect();
    out.sort_by_key(|r| r.ts);
    Ok(out)
}

/// Accept an invitation.
pub async fn accept_invite(client: &Client, room_id: &str) -> Result<(), String> { room_of(client, room_id)?.join().await.map_err(|e| e.to_string()) }

/// Decline an invitation, or leave a room.
pub async fn leave_room(client: &Client, room_id: &str) -> Result<(), String> { room_of(client, room_id)?.leave().await.map_err(|e| e.to_string()) }

/// Create a room and return its id. `public`: listed in the room directory and open to anyone; `invites`: people to invite at once.
pub async fn create_room(client: &Client, name: &str, topic: &str, encrypted: bool) -> Result<String, String> { create_room_with(client, name, topic, encrypted, false, &[]).await }

pub async fn create_room_with(client: &Client, name: &str, topic: &str, encrypted: bool, public: bool, invites: &[String]) -> Result<String, String> {
    use matrix_sdk::ruma::{api::client::room::{create_room::v3::Request, Visibility}, events::{room::encryption::RoomEncryptionEventContent, InitialStateEvent}, serde::Raw};
    let mut req = Request::new();
    if !name.is_empty() { req.name = Some(name.to_string()); }
    if !topic.is_empty() { req.topic = Some(topic.to_string()); }
    if public { req.visibility = Visibility::Public; req.preset = Some(matrix_sdk::ruma::api::client::room::create_room::v3::RoomPreset::PublicChat); }
    req.invite = invites.iter().map(|u| user_of(u)).collect::<Result<Vec<_>, _>>()?;
    if encrypted {
        let ev = InitialStateEvent::with_empty_state_key(RoomEncryptionEventContent::with_recommended_defaults());
        req.initial_state = vec![Raw::new(&ev).map_err(|e| e.to_string())?.cast_unchecked()];
    }
    /* everybody may announce a group call: its presence is a state event, and state events need power level 50 unless the room says otherwise. The
       server replaces the whole `events` map of its defaults, so those are repeated here. */
    let levels = serde_json::json!({"events": {
        "m.room.name": 50, "m.room.power_levels": 100, "m.room.history_visibility": 100, "m.room.canonical_alias": 50, "m.room.avatar": 50,
        "m.room.tombstone": 100, "m.room.server_acl": 100, "m.room.encryption": 100, (crate::group_calls::MEMBER_EVENT): 0 }});
    req.power_level_content_override = Raw::new(&levels).ok().map(|r| r.cast_unchecked());
    client.create_room(req).await.map(|r| r.room_id().to_string()).map_err(|e| e.to_string())
}

/// Open (or create) the direct message with a user.
pub async fn start_dm(client: &Client, user_id: &str) -> Result<String, String> {
    let uid = <&matrix_sdk::ruma::UserId>::try_from(user_id).map_err(|e| format!("not a Matrix user id: {e}"))?;
    client.create_dm(uid).await.map(|r| r.room_id().to_string()).map_err(|e| e.to_string())
}

/// `ui_rooms`, with the rooms that belong to a joined space put in a section named after that space.
pub async fn ui_rooms_with_spaces(client: &Client, spaces: &matrix_sdk_ui::spaces::SpaceService) -> Vec<UiRoom> {
    let mut rooms = ui_rooms(client).await;
    for r in rooms.iter_mut().filter(|r| !r.invite && r.section != "Direct Messages") {
        let Ok(rid) = <&matrix_sdk::ruma::RoomId>::try_from(r.id.as_str()) else { continue };
        if let Some(parent) = spaces.joined_parents_of_child(rid).await.into_iter().next() { r.section = parent.display_name; r.space_id = parent.room_id.to_string(); }
    }
    for r in rooms.iter_mut().filter(|r| !r.invite) {
        if r.favourite { r.section = "Favourites".into(); } else if r.low_priority { r.section = "Low priority".into(); }
    }
    let rank = |r: &UiRoom| match r.section.as_str() { "Invites" => 0, "Favourites" => 1, "Direct Messages" => 2, "Rooms" => 4, "Low priority" => 5, _ => 3 };
    rooms.sort_by(|a, b| (rank(a), a.section.to_lowercase(), a.title.to_lowercase()).cmp(&(rank(b), b.section.to_lowercase(), b.title.to_lowercase())));
    rooms
}

pub(crate) fn clock(ms: u64) -> String { let s = ms / 1000; format!("{:02}:{:02}", (s / 3600) % 24, (s / 60) % 60) }

fn name_of(profile: &TimelineDetails<matrix_sdk_ui::timeline::Profile>, id: &str) -> String {
    /* spec: "Calculating the display name for a user": a name two members share is shown with the user id, so nobody can pass as somebody else */
    let name = match profile { TimelineDetails::Ready(p) => p.display_name.clone().map(|n| if p.display_name_ambiguous { format!("{n} ({id})") } else { n }), _ => None };
    name.unwrap_or_else(|| id.trim_start_matches('@').split(':').next().unwrap_or(id).to_string())
}

/// The HTML of a message: only what the specification allows, no reply fallback, and empty when the message is plain text.
fn html_of(t: &MessageType) -> String {
    use matrix_sdk::ruma::events::room::message::FormattedBody;
    let formatted: Option<&FormattedBody> = match t {
        MessageType::Text(m) => m.formatted.as_ref(),
        MessageType::Emote(m) => m.formatted.as_ref(),
        MessageType::Notice(m) => m.formatted.as_ref(),
        _ => None,
    };
    match formatted {
        Some(f) if f.format == matrix_sdk::ruma::events::room::message::MessageFormat::Html => sanitize_html(&f.body, HtmlSanitizerMode::Compat, RemoveReplyFallback::Yes),
        _ => String::new(),
    }
}

fn describe(c: &TimelineItemContent) -> (String, String, String) {
    /* (kind, body, file name) for anything that is not plain text */
    if let Some(m) = c.as_message() {
        let (kind, name) = match m.msgtype() {
            MessageType::Text(_) => ("text", ""),
            MessageType::Emote(_) => ("emote", ""),
            MessageType::Notice(_) => ("notice", ""),
            MessageType::Image(i) => ("image", i.filename()),
            MessageType::Video(v) => ("video", v.filename()),
            MessageType::Audio(a) => ("audio", a.filename()),
            MessageType::File(f) => ("file", f.filename()),
            MessageType::Location(_) => ("location", ""),
            MessageType::Gallery(_) => ("gallery", ""),
            _ => ("other", ""),
        };
        return (kind.into(), m.body().to_string(), name.into());
    }
    if c.is_unable_to_decrypt() { return ("undecryptable".into(), "[encrypted message: no key yet]".into(), String::new()); }
    if let Some(p) = c.as_poll() { return ("poll".into(), p.results().question, String::new()); }
    if let Some(st) = c.as_sticker() { return ("sticker".into(), st.content().body.clone(), String::new()); }
    ("other".into(), String::new(), String::new())
}

fn preview_of(c: &TimelineItemContent) -> String {
    let (_, body, _) = describe(c);
    let first = body.lines().next().unwrap_or("");
    if first.chars().count() > 120 { first.chars().take(117).collect::<String>() + "..." } else { first.to_string() }
}

/// A picture of the timeline on disk: the picture itself, or (`thumb`) a smaller version of it that the server or the sender made.
#[derive(Debug, Clone, PartialEq)]
pub struct Pic { pub path: PathBuf, pub thumb: bool }

fn message_row(ev: &EventTimelineItem, me: &str, images: &HashMap<String, Pic>) -> Option<UiMessage> {
    let content = ev.content();
    let (kind, body, file_name) = describe(content);
    if kind == "other" && body.is_empty() { return None; } /* membership changes, state, ... : not shown (yet) */
    let id = ev.event_id().map(|e| e.to_string()).unwrap_or_else(|| ev.transaction_id().map(|t| format!("~{t}")).unwrap_or_default());
    let mut row = UiMessage {
        id: id.clone(),
        sender: name_of(ev.sender_profile(), ev.sender().as_str()),
        sender_id: ev.sender().to_string(),
        body,
        kind,
        time: clock(ev.timestamp().0.into()),
        ts: u64::from(ev.timestamp().0),
        own: ev.sender().as_str() == me,
        pending: ev.event_id().is_none(),
        file_name,
        ..Default::default()
    };
    if let Some(m) = content.as_message() {
        row.html = html_of(m.msgtype());
        row.edited = m.is_edited();
        if let MessageType::File(f) = m.msgtype() {
            let mime = f.info.as_ref().and_then(|i| i.mimetype.clone()).unwrap_or_default();
            row.size = f.info.as_ref().and_then(|i| i.size).map(u64::from).unwrap_or(0);
            row.text_file = text_like(&mime, f.filename());
        }
        if let MessageType::Gallery(g) = m.msgtype() {
            row.gallery = g.itemtypes.iter().enumerate().map(|(n, it)| gallery_item(n, it, images.get(&format!("{id}#{n}")))).collect();
            /* the body of a gallery is its caption (or, without one, a list of the file names): show only a real caption */
            if g.itemtypes.iter().map(|it| it.body()).collect::<Vec<_>>().join(", ") == row.body || row.body.trim().is_empty() { row.body.clear(); }
        }
        if let MessageType::Image(i) = m.msgtype() {
            if let Some(info) = &i.info { row.image_w = info.width.map(|w| u64::from(w) as u32).unwrap_or(0); row.image_h = info.height.map(|h| u64::from(h) as u32).unwrap_or(0); }
            if let Some(pic) = images.get(&id) { row.image_path = pic.path.to_string_lossy().into_owned(); row.image_thumb = pic.thumb; }
        }
    }
    if let Some(st) = content.as_sticker() {
        let uint = |v: Option<matrix_sdk::ruma::UInt>| v.map(u64::from).unwrap_or(0) as u32;
        row.image_w = uint(st.content().info.width);
        row.image_h = uint(st.content().info.height);
        row.image_path = images.get(&id).map(|p| p.path.to_string_lossy().into_owned()).unwrap_or_default();
    }
    row.emoji = crate::emotes::emoji_sources(&row.html).into_iter().map(|mxc| UiEmojiRef { mxc, path: String::new() }).collect();
    row.shield = shield_of(ev);
    row.poll = poll_of(content, me);
    row.thread_replies = content.thread_summary().map(|t| t.num_replies as u32).unwrap_or(0);
    row.seen_by = ev.read_receipts().keys().filter(|u| u.as_str() != me).map(|u| u.localpart().to_string()).collect();
    if let Some(r) = content.in_reply_to() {
        row.reply = Some(match &r.event {
            TimelineDetails::Ready(e) => UiReply { event_id: r.event_id.to_string(), sender: name_of(&e.sender_profile, e.sender.as_str()), preview: preview_of(&e.content) },
            _ => UiReply { event_id: r.event_id.to_string(), sender: String::new(), preview: "(a message that is not loaded)".into() },
        });
    }
    if let Some(reactions) = content.reactions() {
        for (key, senders) in reactions.iter() {
            row.reactions.push(UiReaction { key: key.clone(), count: senders.len() as u32, mine: senders.keys().any(|u| u.as_str() == me) });
        }
    }
    Some(row)
}

fn gallery_item(index: usize, it: &matrix_sdk::ruma::events::room::message::GalleryItemType, image: Option<&Pic>) -> UiGalleryItem {
    use matrix_sdk::ruma::events::room::message::GalleryItemType as G;
    let uint = |v: Option<matrix_sdk::ruma::UInt>| v.map(u64::from).unwrap_or(0);
    let mut item = UiGalleryItem { index: index as u32, name: it.body().to_string(), ..Default::default() };
    match it {
        G::Image(i) => {
            item.kind = "image".into(); item.name = i.filename().to_string();
            if let Some(info) = &i.info { item.image_w = uint(info.width) as u32; item.image_h = uint(info.height) as u32; item.size = uint(info.size); }
            item.image_path = image.map(|p| p.path.to_string_lossy().into_owned()).unwrap_or_default();
        }
        G::Video(v) => { item.kind = "video".into(); item.name = v.filename().to_string(); item.size = uint(v.info.as_ref().and_then(|i| i.size)); }
        G::Audio(a) => { item.kind = "audio".into(); item.name = a.filename().to_string(); item.size = uint(a.info.as_ref().and_then(|i| i.size)); }
        G::File(f) => { item.kind = "file".into(); item.name = f.filename().to_string(); item.size = uint(f.info.as_ref().and_then(|i| i.size)); }
        _ => item.kind = "other".into(),
    }
    item
}

/// The source of the attachment of a gallery item.
fn gallery_source(it: &matrix_sdk::ruma::events::room::message::GalleryItemType) -> Option<matrix_sdk::ruma::events::room::MediaSource> {
    use matrix_sdk::ruma::events::room::message::GalleryItemType as G;
    match it { G::Image(c) => Some(c.source.clone()), G::Video(c) => Some(c.source.clone()), G::Audio(c) => Some(c.source.clone()), G::File(c) => Some(c.source.clone()), _ => None }
}

/// The messages of a timeline in order (decrypted; what cannot be read shows as a placeholder). `images`: event id -> cached picture file.
pub fn ui_messages_with(items: &[Arc<TimelineItem>], me: &str, images: &HashMap<String, Pic>) -> Vec<UiMessage> {
    items.iter().filter_map(|i| i.as_event()).filter_map(|ev| message_row(ev, me, images)).collect()
}

pub fn ui_messages(items: &[Arc<TimelineItem>], me: &str) -> Vec<UiMessage> { ui_messages_with(items, me, &HashMap::new()) }

/// One answer of a poll with its votes.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct UiPollAnswer { pub id: String, pub text: String, pub votes: u32, pub mine: bool }

/// A poll as drawn in the timeline.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct UiPoll { pub question: String, pub answers: Vec<UiPollAnswer>, pub total_votes: u32, pub max_selections: u32, pub ended: bool }

fn poll_of(content: &TimelineItemContent, me: &str) -> Option<UiPoll> {
    let r = content.as_poll()?.results();
    let answers: Vec<UiPollAnswer> = r.answers.iter().map(|a| {
        let voters = r.votes.get(&a.id).cloned().unwrap_or_default();
        UiPollAnswer { id: a.id.clone(), text: a.text.clone(), votes: voters.len() as u32, mine: voters.iter().any(|v| v == me) }
    }).collect();
    let total_votes = answers.iter().map(|a| a.votes).sum();
    Some(UiPoll { question: r.question, answers, total_votes, max_selections: r.max_selections as u32, ended: r.end_time.is_some() })
}

/// Start a poll (the `org.matrix.msc3381.*` event names that Element reads). `max_selections` 1 = one answer only.
pub async fn create_poll(timeline: &Timeline, question: &str, options: &[String], max_selections: u32) -> Result<(), String> {
    use matrix_sdk::ruma::events::{poll::unstable_start::{NewUnstablePollStartEventContent, UnstablePollAnswer, UnstablePollAnswers, UnstablePollStartContentBlock}, AnyMessageLikeEventContent};
    let options: Vec<&String> = options.iter().filter(|o| !o.trim().is_empty()).collect();
    if question.trim().is_empty() { return Err("a poll needs a question".into()); }
    if options.len() < 2 { return Err("a poll needs at least two answers".into()); }
    let answers: Vec<UnstablePollAnswer> = options.iter().enumerate().map(|(i, o)| UnstablePollAnswer::new(format!("a{i}"), o.trim())).collect();
    let answers = UnstablePollAnswers::try_from(answers).map_err(|_| "a poll needs between 2 and 20 answers".to_string())?;
    let mut block = UnstablePollStartContentBlock::new(question.trim(), answers);
    block.max_selections = (max_selections.clamp(1, options.len() as u32) as u16).into();
    let fallback = format!("{}\n{}", question.trim(), options.iter().enumerate().map(|(i, o)| format!("{}. {}", i + 1, o.trim())).collect::<Vec<_>>().join("\n"));
    timeline.send(AnyMessageLikeEventContent::UnstablePollStart(NewUnstablePollStartEventContent::plain_text(fallback, block).into())).await.map(|_| ()).map_err(|e| e.to_string())
}

/// Vote: the answer ids chosen (replaces our earlier vote).
pub async fn vote_poll(timeline: &Timeline, poll_id: &str, answer_ids: Vec<String>) -> Result<(), String> {
    use matrix_sdk::ruma::events::{poll::unstable_response::UnstablePollResponseEventContent, AnyMessageLikeEventContent};
    let id: OwnedEventId = poll_id.try_into().map_err(|e| format!("bad event id: {e}"))?;
    timeline.send(AnyMessageLikeEventContent::UnstablePollResponse(UnstablePollResponseEventContent::new(answer_ids, id))).await.map(|_| ()).map_err(|e| e.to_string())
}

/// Close a poll we started.
pub async fn end_poll(timeline: &Timeline, poll_id: &str) -> Result<(), String> {
    use matrix_sdk::ruma::events::{poll::unstable_end::UnstablePollEndEventContent, AnyMessageLikeEventContent};
    let id: OwnedEventId = poll_id.try_into().map_err(|e| format!("bad event id: {e}"))?;
    timeline.send(AnyMessageLikeEventContent::UnstablePollEnd(UnstablePollEndEventContent::new("The poll has ended.", id))).await.map(|_| ()).map_err(|e| e.to_string())
}

/// A warning about who really wrote an encrypted message: level "red" (be careful) or "grey" (not verified), with the reason in words.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct UiShield { pub level: String, pub text: String }

fn shield_of(ev: &EventTimelineItem) -> Option<UiShield> {
    use matrix_sdk_ui::timeline::{TimelineEventShieldState as S, TimelineEventShieldStateCode as C};
    let (level, code) = match ev.get_shield(false) { S::Red { code } => ("red", code), S::Grey { code } => ("grey", code), S::None => return None };
    let text = match code {
        C::AuthenticityNotGuaranteed => "The authenticity of this encrypted message cannot be guaranteed",
        C::UnknownDevice => "Sent by a session that is not known (it may have been deleted)",
        C::UnsignedDevice => "Sent by a session its owner has not verified",
        C::UnverifiedIdentity => "The sender has not been verified by you",
        C::VerificationViolation => "The sender was verified before but their identity has changed",
        C::MismatchedSender => "The sender does not match the session that encrypted this message",
        _ => "This message was not encrypted",
    };
    Some(UiShield { level: level.into(), text: text.into() })
}

/// What a link preview card shows.
#[derive(Debug, Clone, PartialEq, Serialize, Default)]
pub struct UiPreview { pub url: String, pub title: String, pub description: String, pub site: String, pub image_path: String }

/// The first http(s) link in a text, without trailing punctuation (a closing bracket stays when the link has an opening one, like Wikipedia's).
pub fn first_url(text: &str) -> Option<String> {
    let start = text.find("https://").into_iter().chain(text.find("http://")).min()?;
    let raw: String = text[start..].chars().take_while(|c| !c.is_whitespace() && !matches!(c, '<' | '>' | '"')).collect();
    let mut url = raw.as_str();
    loop {
        let Some(last) = url.chars().last() else { break };
        let trim = match last {
            '.' | ',' | ';' | ':' | '!' | '?' | '\'' => true,
            ')' => url.matches('(').count() < url.matches(')').count(),
            _ => false,
        };
        if !trim { break; }
        url = &url[..url.len() - last.len_utf8()];
    }
    if url.len() > "https://".len() { Some(url.to_string()) } else { None }
}

/// Ask the homeserver for the preview of a link (it fetches the page, so the server learns which links are shown) and cache its picture in `cache`.
/// Tries the authenticated media endpoint first, like the rest of the media code, then the legacy one. None when the server has nothing.
pub async fn link_preview(client: &Client, url: &str, cache: &Path) -> Option<UiPreview> {
    use matrix_sdk::ruma::api::client::authenticated_media::get_media_preview as v1;
    #[allow(deprecated)]
    use matrix_sdk::ruma::api::client::media::get_media_preview::v3 as legacy;
    let raw = match client.send(v1::v1::Request::new(url.to_string())).await {
        Ok(r) => r.data,
        #[allow(deprecated)]
        Err(_) => client.send(legacy::Request::new(url.to_string())).await.ok()?.data,
    }?;
    let og: serde_json::Value = serde_json::from_str(raw.get()).ok()?;
    let text = |k: &str| og[k].as_str().unwrap_or("").to_string();
    let mut p = UiPreview { url: url.to_string(), title: text("og:title"), description: text("og:description"), site: text("og:site_name"), image_path: String::new() };
    if p.title.is_empty() && p.description.is_empty() { return None; }
    if let Some(mxc) = og["og:image"].as_str().and_then(|m| <&matrix_sdk::ruma::MxcUri>::try_from(m).ok()) {
        let req = MediaRequestParameters { source: matrix_sdk::ruma::events::room::MediaSource::Plain(mxc.to_owned()), format: MediaFormat::File };
        if let Ok(bytes) = client.media().get_media_content(&req, false).await {
            let _ = std::fs::create_dir_all(cache);
            let name: String = url.chars().filter(|c| c.is_ascii_alphanumeric()).take(60).collect();
            let path = cache.join(format!("preview-{name}.img"));
            if std::fs::write(&path, bytes).is_ok() { p.image_path = path.to_string_lossy().into_owned(); }
        }
    }
    Some(p)
}

/// The main timeline of a room: thread replies are not mixed into it (the thread root shows how many there are).
pub async fn open_timeline(room: &matrix_sdk::Room) -> Result<Timeline, String> {
    use matrix_sdk_ui::timeline::{RoomExt, TimelineFocus};
    room.timeline_builder().with_focus(TimelineFocus::Live { hide_threaded_events: true }).build().await.map_err(|e| e.to_string())
}

/// The timeline of one thread (its root and the replies).
pub async fn open_thread(room: &matrix_sdk::Room, root_event_id: &str) -> Result<Timeline, String> {
    use matrix_sdk_ui::timeline::{RoomExt, TimelineFocus};
    let root: OwnedEventId = root_event_id.try_into().map_err(|e| format!("bad event id: {e}"))?;
    room.timeline_builder().with_focus(TimelineFocus::Thread { root_event_id: root }).build().await.map_err(|e| e.to_string())
}

/// The messages whose text or sender contains every word of `query` (case-insensitive), newest first, at most `limit`.
/// Works on what the timeline has loaded: decrypted text of encrypted rooms is searchable too, the server could not do that.
pub fn search_messages(items: &[Arc<TimelineItem>], me: &str, query: &str, limit: usize) -> Vec<UiMessage> {
    let words: Vec<String> = query.split_whitespace().map(|w| w.to_lowercase()).collect();
    if words.is_empty() { return Vec::new(); }
    ui_messages(items, me).into_iter().rev().filter(|m| {
        let hay = format!("{} {}", m.body, m.sender).to_lowercase();
        m.kind != "undecryptable" && words.iter().all(|w| hay.contains(w))
    }).take(limit).collect()
}

/// A picture whose message declares it at most this big is fetched as it is even when thumbnails are on: a thumbnail would hardly be smaller.
const SMALL_PICTURE: u64 = 256 * 1024;

/// The size asked of the server for a timeline thumbnail. The timeline draws a picture at most 480x320 points, which is this many pixels on a screen with twice as many.
const THUMBNAIL_BOX: (u32, u32) = (960, 640);

/// Pictures downloaded by one call of `fetch_images`: the newest come first and the app calls again for the rest, so the pictures of the end of a room show up
/// at once instead of after the last one of a long room has arrived.
const PICTURES_PER_PASS: usize = 8;

/// Write a file so that a crash or a kill never leaves half of it under its name: a file found on disk is used without a look.
fn write_whole(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut part = path.as_os_str().to_owned();
    part.push(".part");
    let part = PathBuf::from(part);
    std::fs::write(&part, bytes)?;
    std::fs::rename(&part, path).inspect_err(|_| { let _ = std::fs::remove_file(&part); })
}

fn has_content(path: &Path) -> bool { std::fs::metadata(path).map(|m| m.is_file() && m.len() > 0).unwrap_or(false) }

/// The bytes of a picture of a message and whether they are a thumbnail. With `thumbs` the server is asked for a thumbnail of an unencrypted picture; the sender's own
/// (`info.thumbnail_source`) is the next choice and the only one for an encrypted picture, which the server cannot scale. A picture the message declares small, a failed
/// or empty answer and a message without a thumbnail all end in the picture itself.
async fn fetch_picture(client: &Client, source: &matrix_sdk::ruma::events::room::MediaSource, info: Option<&matrix_sdk::ruma::events::room::ImageInfo>, thumbs: bool) -> Option<(Vec<u8>, bool)> {
    use matrix_sdk::{media::MediaThumbnailSettings, ruma::events::room::MediaSource};
    let get = |source: MediaSource, format: MediaFormat| async move { client.media().get_media_content(&MediaRequestParameters { source, format }, false).await.ok() };
    let small = info.and_then(|i| i.size).is_some_and(|size| u64::from(size) <= SMALL_PICTURE);
    if thumbs && !small {
        if matches!(source, MediaSource::Plain(_)) {
            let (w, h) = THUMBNAIL_BOX;
            if let Some(bytes) = get(source.clone(), MediaFormat::Thumbnail(MediaThumbnailSettings::new(w.into(), h.into()))).await.filter(|b| !b.is_empty()) { return Some((bytes, true)); }
        }
        if let Some(thumb) = info.and_then(|i| i.thumbnail_source.clone()) {
            if let Some(bytes) = get(thumb, MediaFormat::File).await.filter(|b| !b.is_empty()) { return Some((bytes, true)); }
        }
    }
    get(source.clone(), MediaFormat::File).await.map(|bytes| (bytes, false))
}

/// Download (and decrypt) the pictures of these items that are not shown yet into `cache`; returns what was added, at most `PICTURES_PER_PASS` downloads at a time,
/// the newest first. What an earlier run left in `cache` is used as it is: the picture itself first, else (with `thumbs`) its thumbnail, else it is downloaded.
/// With `thumbs` the timeline gets a smaller version where there is one (see `fetch_picture`); a thumbnail in `have` is fetched again as the picture when
/// thumbnails have been switched off since.
pub async fn fetch_images(client: &Client, items: &[Arc<TimelineItem>], cache: &Path, have: &HashMap<String, Pic>, thumbs: bool) -> HashMap<String, Pic> {
    use matrix_sdk::ruma::events::room::{message::GalleryItemType, ImageInfo, MediaSource};
    let mut added = HashMap::new();
    let mut downloaded = 0;
    let _ = std::fs::create_dir_all(cache);
    let done = |key: &str, added: &HashMap<String, Pic>| have.get(key).is_some_and(|p| thumbs || !p.thumb) || added.contains_key(key);
    for item in items.iter().rev() {
        if downloaded >= PICTURES_PER_PASS { break; }
        let Some(ev) = item.as_event() else { continue };
        let Some(id) = ev.event_id().map(|e| e.to_string()) else { continue };
        let safe_of = |key: &str| -> String { key.chars().filter(|c| c.is_ascii_alphanumeric()).collect() };
        if let Some(st) = ev.content().as_sticker() { /* a sticker is a picture of its own */
            if done(&id, &added) { continue; }
            let path = cache.join(format!("{}.img", safe_of(&id)));
            if has_content(&path) { added.insert(id, Pic { path, thumb: false }); continue; }
            let req = MediaRequestParameters { source: st.content().source.clone().into(), format: MediaFormat::File };
            if let Ok(bytes) = client.media().get_media_content(&req, false).await {
                if write_whole(&path, &bytes).is_ok() { added.insert(id, Pic { path, thumb: false }); downloaded += 1; }
            }
            continue;
        }
        let Some(m) = ev.content().as_message() else { continue };
        /* (key, source, file name, info) of every picture of the message; a gallery's pictures are keyed "<event id>#<index>" */
        let wanted: Vec<(String, MediaSource, String, Option<&ImageInfo>)> = match m.msgtype() {
            MessageType::Image(img) => vec![(id.clone(), img.source.clone(), img.filename().to_string(), img.info.as_deref())],
            MessageType::Gallery(g) => g.itemtypes.iter().enumerate().filter_map(|(n, it)| match it {
                GalleryItemType::Image(img) => Some((format!("{id}#{n}"), img.source.clone(), img.filename().to_string(), img.info.as_deref())),
                _ => None,
            }).collect(),
            _ => continue,
        };
        for (key, source, name, info) in wanted {
            if done(&key, &added) { continue; }
            let ext = name.rsplit('.').next().filter(|e| e.len() <= 5 && !e.contains('/')).unwrap_or("img");
            let safe = safe_of(&key);
            let (full, thumb) = (cache.join(format!("{safe}.{ext}")), cache.join(format!("{safe}-t.img")));
            if has_content(&full) { added.insert(key, Pic { path: full, thumb: false }); continue; }
            if thumbs && has_content(&thumb) { added.insert(key, Pic { path: thumb, thumb: true }); continue; }
            let Some((bytes, is_thumb)) = fetch_picture(client, &source, info, thumbs).await else { continue };
            let path = if is_thumb { thumb } else { full };
            if write_whole(&path, &bytes).is_ok() { added.insert(key, Pic { path, thumb: is_thumb }); downloaded += 1; }
        }
    }
    added
}

/// Does a file of this type / name hold text a person can read? (Text types, source code, data and config formats.)
pub fn text_like(mime: &str, name: &str) -> bool {
    let m = mime.to_ascii_lowercase();
    if m.starts_with("text/") || m.contains("json") || m.contains("xml") || m.contains("yaml") || m.contains("toml") || m.contains("javascript") || m.contains("x-sh") || m.contains("x-shellscript") || m.contains("sql") || m.contains("csv") { return true; }
    let ext = name.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    name.contains('.') && matches!(ext.as_str(),
        "txt" | "text" | "md" | "markdown" | "rst" | "log" | "csv" | "tsv" | "json" | "jsonl" | "xml" | "html" | "htm" | "css" | "js" | "mjs" | "ts" | "tsx" | "jsx" | "yaml" | "yml" | "toml" | "ini" | "cfg" | "conf" | "env"
        | "sh" | "bash" | "zsh" | "fish" | "bat" | "ps1" | "py" | "rs" | "c" | "h" | "cc" | "cpp" | "hpp" | "cs" | "java" | "kt" | "go" | "rb" | "php" | "pl" | "lua" | "swift" | "sql" | "diff" | "patch" | "tex" | "bib" | "srt" | "vtt" | "gitignore" | "dockerfile" | "makefile" | "cmake" | "gradle" | "properties" | "lock")
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct UiTextFile { pub event_id: String, pub name: String, pub size: u64, pub text: String, pub truncated: bool }

/// Download a text file of a message and return what to show: at most `max_chars` characters (the rest is cut and `truncated` says so).
/// Files that are not text (binary data) or are too large to be worth previewing are refused with a reason.
pub async fn read_text_file(client: &Client, timeline: &Timeline, event_id: &str, max_chars: usize) -> Result<UiTextFile, String> {
    const MAX_DOWNLOAD: u64 = 16 * 1024 * 1024;
    let items: Vec<_> = timeline.items().await.iter().cloned().collect();
    let m = ui_messages(&items, "").into_iter().find(|m| m.id == event_id).ok_or("that message is not in the timeline")?;
    if m.kind != "file" { return Err("only files are opened as text".into()); }
    if m.size > MAX_DOWNLOAD { return Err(format!("this file is {} MB: too large to preview, save it instead", m.size / (1024 * 1024))); }
    let dest = std::env::temp_dir().join(format!("qvector-text-{}-{}", std::process::id(), event_id.chars().filter(|c| c.is_ascii_alphanumeric()).take(16).collect::<String>()));
    let n = save_attachment(client, timeline, event_id, &dest).await;
    let bytes = std::fs::read(&dest); /* read back: save_attachment writes the decrypted bytes */
    let _ = std::fs::remove_file(&dest);
    let (n, bytes) = (n?, bytes.map_err(|e| e.to_string())?);
    if bytes.iter().take(8000).any(|b| *b == 0) { return Err("this file looks binary, not text: save it instead".into()); }
    let all = String::from_utf8_lossy(&bytes);
    let truncated = all.chars().count() > max_chars;
    let text: String = if truncated { all.chars().take(max_chars).collect() } else { all.into_owned() };
    Ok(UiTextFile { event_id: event_id.to_string(), name: m.file_name, size: n, text, truncated })
}

/// Download (and decrypt) the file, picture, video or audio of the message `event_id` and write it to `dest`; returns the size.
pub async fn save_attachment(client: &Client, timeline: &Timeline, event_id: &str, dest: &Path) -> Result<u64, String> {
    let (event_id, item) = match event_id.split_once('#') { Some((e, n)) => (e, Some(n.parse::<usize>().map_err(|_| "bad gallery item number")?)), None => (event_id, None) };
    let items = timeline.items().await;
    let ev = items.iter().filter_map(|i| i.as_event()).find(|e| e.event_id().map(|i| i.as_str()) == Some(event_id)).ok_or("that message is not in the timeline")?;
    let m = ev.content().as_message().ok_or("that message has no attachment")?;
    let source = match m.msgtype() {
        MessageType::Gallery(g) => item.and_then(|n| g.itemtypes.get(n)).and_then(gallery_source).ok_or("that gallery item does not exist")?,
        MessageType::Image(c) => c.source.clone(),
        MessageType::Video(c) => c.source.clone(),
        MessageType::Audio(c) => c.source.clone(),
        MessageType::File(c) => c.source.clone(),
        _ => return Err("that message has no attachment".into()),
    };
    let bytes = client.media().get_media_content(&MediaRequestParameters { source, format: MediaFormat::File }, false).await.map_err(|e| e.to_string())?;
    std::fs::write(dest, &bytes).map_err(|e| format!("cannot write {}: {e}", dest.display()))?;
    Ok(bytes.len() as u64)
}

/// A file name that is safe to hand to the system's opener: the sender chooses the name, so only characters that cannot form a path are kept, every dot
/// but the last is dropped, and the extension must be a known one for this kind of media (otherwise it is replaced by a neutral one), so a "video" called
/// `clip.bat` or `clip.desktop` is never opened as a program.
fn safe_media_name(kind: &str, file_name: &str) -> String {
    let clean: String = file_name.rsplit(['/', '\\']).next().unwrap_or("").chars().map(|c| if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') { c } else { '_' }).collect();
    let (stem, ext) = match clean.rsplit_once('.') { Some((s, e)) => (s.to_string(), e.to_ascii_lowercase()), None => (clean.clone(), String::new()) };
    let known: &[&str] = match kind {
        "image" => &["png", "jpg", "jpeg", "gif", "webp", "bmp", "avif", "heic", "tif", "tiff", "svg"],
        "video" => &["mp4", "webm", "mkv", "mov", "avi", "m4v", "ogv", "3gp"],
        _ => &["mp3", "ogg", "oga", "opus", "wav", "flac", "m4a", "aac", "weba"],
    };
    let ext = if known.contains(&ext.as_str()) { ext } else { match kind { "image" => "img", "video" => "vid", _ => "aud" }.to_string() };
    let stem: String = stem.replace('.', "_");
    let stem = stem.trim_start_matches('_');
    format!("{}.{ext}", if stem.is_empty() { "media" } else { stem })
}

/// Download a picture, video or audio message into `dir` (under a safe version of its file name) so a player can open it; returns the path.
/// Other kinds of file are refused: they are saved by the user, never opened by the app.
pub async fn media_copy(client: &Client, timeline: &Timeline, event_id: &str, dir: &Path) -> Result<PathBuf, String> {
    let items: Vec<_> = timeline.items().await.iter().cloned().collect();
    let (base, item) = match event_id.split_once('#') { Some((e, n)) => (e, n.parse::<usize>().ok()), None => (event_id, None) };
    let mut m = ui_messages(&items, "").into_iter().find(|m| m.id == base).ok_or("that message is not in the timeline")?;
    if let Some(g) = item.and_then(|n| m.gallery.get(n)) { m.kind = g.kind.clone(); m.file_name = g.name.clone(); }
    if !matches!(m.kind.as_str(), "image" | "video" | "audio") { return Err("only pictures, videos and audio are opened from here; save other files instead".into()); }
    let stem = safe_media_name(&m.kind, &m.file_name);
    let id: String = event_id.chars().filter(|c| c.is_ascii_alphanumeric()).take(12).collect::<String>() + item.map(|n| format!("n{n}")).unwrap_or_default().as_str();
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let dest = dir.join(format!("{id}-{stem}"));
    save_attachment(client, timeline, event_id, &dest).await?;
    Ok(dest)
}

/// Ask the timeline to try again with the messages it could not decrypt (a key may have arrived since). Returns how many there were.
pub async fn retry_undecryptable(timeline: &Timeline) -> usize {
    let ids: Vec<String> = timeline.items().await.iter().filter_map(|i| i.as_event())
        .filter_map(|e| match e.content().as_unable_to_decrypt() { Some(matrix_sdk_ui::timeline::EncryptedMessage::MegolmV1AesSha2 { session_id, .. }) => Some(session_id.clone()), _ => None }).collect();
    if !ids.is_empty() { timeline.retry_decryption(ids.clone()).await; }
    ids.len()
}

/// Send text (Markdown becomes formatted text) into the timeline's room, optionally as a reply.
pub async fn send_text(timeline: &Timeline, text: &str, reply_to: Option<&str>) -> Result<(), String> { send_text_with_emoji(timeline, text, reply_to, &HashMap::new()).await }

/// `send_text`, with `:shortcode:`s of the given custom emoji (shortcode -> `mxc://` address) sent as pictures.
pub async fn send_text_with_emoji(timeline: &Timeline, text: &str, reply_to: Option<&str>, emoji: &HashMap<String, String>) -> Result<(), String> {
    send_text_full(timeline, text, reply_to, emoji, &[]).await
}

/// The formatted body of a message that mentions people: every `@label` of `mentions` (user id, label) becomes a matrix.to link (a "pill" in
/// Element and Cinny). Starts from the Markdown's HTML, or from the escaped text when it had none.
/// Does `@label` occur in the text as a whole word (not as the start of a longer name)?
fn mentions_name(text: &str, label: &str) -> bool {
    let at = format!("@{label}");
    text.match_indices(&at).any(|(i, _)| text[i + at.len()..].chars().next().map_or(true, |c| !c.is_alphanumeric()))
}

pub fn with_mentions(text: &str, html: Option<&str>, mentions: &[(String, String)]) -> Option<String> {
    let esc = |s: &str| s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;");
    let mut used: Vec<&(String, String)> = mentions.iter().filter(|(i, l)| !l.is_empty() && i != "@room" && mentions_name(text, l)).collect();
    if used.is_empty() { return None; }
    used.sort_by(|a, b| b.1.len().cmp(&a.1.len())); /* longer names first, so `@Bob Smith` is not eaten by `@Bob` */
    let mut out = match html { Some(h) => h.to_string(), None => esc(text).replace('\n', "<br>") };
    /* one pass, longest name first at each '@', and never inside a tag */
    let src = std::mem::take(&mut out);
    let names: Vec<(String, String)> = used.iter().map(|(id, l)| (format!("@{}", esc(l)), esc(id))).collect();
    let mut i = 0;
    let mut in_tag = false;
    while i < src.len() {
        let rest = &src[i..];
        let c = rest.chars().next().unwrap();
        if c == '<' { in_tag = true; } else if c == '>' { in_tag = false; }
        if c == '@' && !in_tag {
            if let Some((at, id)) = names.iter().find(|(at, _)| rest.starts_with(at.as_str()) && rest[at.len()..].chars().next().map_or(true, |c| !c.is_alphanumeric())) {
                out.push_str(&format!("<a href=\"https://matrix.to/#/{id}\">{at}</a>"));
                i += at.len();
                continue;
            }
        }
        out.push(c);
        i += c.len_utf8();
    }
    Some(out)
}

/// `send_text_with_emoji`, with the people the user picked from the `@` list as `(user id, name)`: they get `m.mentions` (what makes the
/// other clients notify them) and a link in the formatted body.
pub async fn send_text_full(timeline: &Timeline, text: &str, reply_to: Option<&str>, emoji: &HashMap<String, String>, mentions: &[(String, String)]) -> Result<(), String> {
    let (msgtype, m) = text_content(text, emoji, mentions);
    /* always present, empty when nobody is mentioned: servers and clients that still have the old body-matching push rules would otherwise notify on a name in the text */
    let mut content = RoomMessageEventContentWithoutRelation::new(msgtype.clone()).add_mentions(m.clone());
    match reply_to.filter(|r| !r.is_empty()) {
        Some(id) => {
            let id: OwnedEventId = id.try_into().map_err(|e| format!("bad event id: {e}"))?;
            timeline.send_reply(content, id).await.map_err(|e| e.to_string())?;
        }
        None => {
            let mut plain = RoomMessageEventContent::new(msgtype);
            plain = plain.add_mentions(m);
            timeline.send(plain.into()).await.map_err(|e| e.to_string())?; }
    }
    Ok(())
}

/// The message a typed text becomes: Markdown as formatted text, `:shortcode:` custom emoji as pictures, picked `@names` as links; and
/// the user ids those links mention.
fn text_content(text: &str, emoji: &HashMap<String, String>, mentions: &[(String, String)]) -> (MessageType, matrix_sdk::ruma::events::Mentions) {
    use matrix_sdk::ruma::events::room::message::FormattedBody;
    /* "/me waves" is an emote (m.emote): shown as "* name waves" */
    let (text, emote) = match text.strip_prefix("/me ") { Some(rest) if !rest.trim().is_empty() => (rest, true), _ => (text, false) };
    let mut msgtype = if emote { RoomMessageEventContent::emote_markdown(text).msgtype } else { RoomMessageEventContent::text_markdown(text).msgtype };
    let formatted = match &mut msgtype { MessageType::Text(t) => Some(&mut t.formatted), MessageType::Emote(t) => Some(&mut t.formatted), _ => None };
    if let Some(formatted) = formatted {
        let html = formatted.as_ref().map(|f| f.body.clone());
        if let Some(h) = crate::emotes::with_custom_emoji(text, html.as_deref(), emoji) { *formatted = Some(FormattedBody::html(h)); }
        let html = formatted.as_ref().map(|f| f.body.clone());
        if let Some(h) = with_mentions(text, html.as_deref(), mentions) { *formatted = Some(FormattedBody::html(h)); }
    }
    let ids: Vec<matrix_sdk::ruma::OwnedUserId> = mentions.iter().filter(|(i, l)| i != "@room" && mentions_name(text, l)).filter_map(|(i, _)| i.as_str().try_into().ok()).collect();
    let mut m = matrix_sdk::ruma::events::Mentions::with_user_ids(ids);
    m.room = mentions.iter().any(|(i, l)| i == "@room" && mentions_name(text, l)); /* @room: notifies everybody (m.mentions.room) */
    (msgtype, m)
}

/// Send a file (a picture when it looks like one) into the timeline's room; encrypted rooms get an encrypted upload.
/// Send a sticker (a picture that is already on the server).
pub async fn send_sticker(timeline: &Timeline, mxc: &str, body: &str, width: u32, height: u32, mime: &str) -> Result<(), String> {
    use matrix_sdk::ruma::events::{room::ImageInfo, sticker::StickerEventContent, AnyMessageLikeEventContent};
    let uri = <&matrix_sdk::ruma::MxcUri>::from(mxc);
    if !uri.is_valid() { return Err(format!("not a media address: {mxc}")); }
    let mut info = ImageInfo::new();
    if width > 0 && height > 0 { info.width = Some(width.into()); info.height = Some(height.into()); }
    if !mime.is_empty() { info.mimetype = Some(mime.to_string()); }
    timeline.send(AnyMessageLikeEventContent::Sticker(StickerEventContent::new(body.to_string(), info, uri.to_owned()))).await.map(|_| ()).map_err(|e| e.to_string())
}

pub async fn send_file(timeline: &Timeline, path: &Path, caption: Option<&str>) -> Result<(), String> {
    use matrix_sdk::ruma::events::room::message::TextMessageEventContent;
    use matrix_sdk_ui::timeline::AttachmentConfig;
    let mut config = AttachmentConfig::default();
    if let Some(c) = caption.filter(|c| !c.is_empty()) { config.caption = Some(TextMessageEventContent::markdown(c)); }
    timeline.send_attachment(path.to_path_buf(), mime_of(path), config).await.map_err(|e| e.to_string())?;
    Ok(())
}

fn mime_of(path: &Path) -> mime::Mime {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    match ext.as_str() {
        "png" => mime::IMAGE_PNG, "jpg" | "jpeg" => mime::IMAGE_JPEG, "gif" => mime::IMAGE_GIF, "webp" => "image/webp".parse().unwrap(),
        "mp4" => "video/mp4".parse().unwrap(), "mp3" => "audio/mpeg".parse().unwrap(),
        "json" => "application/json".parse().unwrap(), "xml" => "application/xml".parse().unwrap(), "html" | "htm" => "text/html".parse().unwrap(),
        "csv" => "text/csv".parse().unwrap(), "md" | "markdown" => "text/markdown".parse().unwrap(),
        e if text_like("", &format!("x.{e}")) => mime::TEXT_PLAIN,
        _ => mime::APPLICATION_OCTET_STREAM,
    }
}

/// Send several files as ONE message (a media gallery, MSC4274: `dm.filament.gallery`, the way Element X shows several pictures at once);
/// `caption` (Markdown) belongs to the whole message. One file is sent as an ordinary attachment, as clients that do not know galleries show nothing useful for them.
pub async fn send_gallery(timeline: &Timeline, paths: &[PathBuf], caption: Option<&str>) -> Result<(), String> {
    use matrix_sdk::attachment::{AttachmentInfo, BaseFileInfo, BaseImageInfo};
    use matrix_sdk::ruma::events::room::message::TextMessageEventContent;
    use matrix_sdk_ui::timeline::{GalleryConfig, GalleryItemInfo};
    match paths {
        [] => return Err("no files to send".into()),
        [one] => return send_file(timeline, one, caption).await,
        _ => {}
    }
    let mut config = GalleryConfig::new();
    for p in paths {
        let mime = mime_of(p);
        let attachment_info = if mime.type_() == mime::IMAGE { AttachmentInfo::Image(BaseImageInfo::default()) } else { AttachmentInfo::File(BaseFileInfo::default()) };
        config = config.add_item(GalleryItemInfo { source: p.clone().into(), content_type: mime, attachment_info, caption: None, thumbnail: None });
    }
    config = config.caption(caption.filter(|c| !c.is_empty()).map(TextMessageEventContent::markdown));
    timeline.send_gallery(config).await.map_err(|e| e.to_string())?;
    Ok(())
}

/// Add or remove our reaction `key` on the message (an event id).
pub async fn toggle_reaction(timeline: &Timeline, event_id: &str, key: &str) -> Result<bool, String> {
    let id: OwnedEventId = event_id.try_into().map_err(|e| format!("bad event id: {e}"))?;
    timeline.toggle_reaction(&TimelineEventItemId::EventId(id), key).await.map_err(|e| e.to_string())
}

/// Call `on_change` with the display names of the other people typing in the room whenever that changes (an empty list when nobody is).
/// The watch lasts until the returned task is aborted.
pub fn watch_typing(client: &Client, room_id: &str, on_change: impl Fn(Vec<String>) + Send + 'static) -> Result<tokio::task::JoinHandle<()>, String> {
    let room = room_of(client, room_id)?;
    let (guard, mut rx) = room.subscribe_to_typing_notifications();
    Ok(tokio::spawn(async move {
        let _guard = guard; /* dropping it ends the subscription */
        while let Ok(ids) = rx.recv().await {
            let mut names = Vec::new();
            for id in ids {
                let name = match room.get_member_no_sync(&id).await { Ok(Some(m)) => m.display_name().map(String::from), _ => None };
                names.push(name.unwrap_or_else(|| id.localpart().to_string()));
            }
            on_change(names);
        }
    }))
}

/// Fetch older messages into the timeline. Returns true when the start of the room has been reached (nothing more to load).
/// Fetch the message a reply answers when the timeline does not hold it (the row then updates with its sender and text).
pub async fn load_reply(timeline: &Timeline, reply_event_id: &str) -> Result<(), String> {
    let id = matrix_sdk::ruma::EventId::parse(reply_event_id).map_err(|e| e.to_string())?;
    timeline.fetch_details_for_event(&id).await.map_err(|e| e.to_string())
}

pub async fn load_older(timeline: &Timeline) -> Result<bool, String> { timeline.paginate_backwards(30).await.map_err(|e| e.to_string()) }

/// Give back the memory of rooms nobody is looking at. The SDK keeps every event a sync brings for a room in that room's in-memory cache and only
/// drops all but the newest chunk when the room's last subscriber goes away: a room nobody has opened since the app started never does, and a busy
/// channel adds thousands of events a day (about a kilobyte each) for as long as the app runs. A subscriber that comes and goes asks for that
/// shrink. `skip` is the open room (it has a subscriber, so there is nothing to do there). Returns how many rooms were asked.
pub async fn shrink_event_caches(client: &Client, rooms: &[matrix_sdk::ruma::OwnedRoomId], skip: Option<&str>) -> usize {
    let mut asked = 0;
    for id in rooms {
        if Some(id.as_str()) == skip { continue; }
        let Some(room) = client.get_room(id) else { continue };
        let Ok((cache, _handles)) = room.event_cache().await else { continue };
        if let Ok((_events, subscriber)) = cache.subscribe().await { drop(subscriber); asked += 1; }
    }
    asked
}

/// Tell the room we have read up to its newest message (so the server's unread counts and other people's receipts follow). True if a receipt was sent.
pub async fn mark_read(timeline: &Timeline) -> Result<bool, String> {
    let sent = timeline.mark_as_read(matrix_sdk::ruma::api::client::receipt::create_receipt::v3::ReceiptType::Read).await.map_err(|e| e.to_string())?;
    if sent {
        /* the public receipt tells other people; m.fully_read ("unread since here") is what the user's other devices and clients follow */
        let newest = timeline.items().await.iter().rev().filter_map(|i| i.as_event()).find_map(|e| e.event_id().map(|i| i.to_owned()));
        if let Some(id) = newest {
            let r = matrix_sdk::room::Receipts::new().fully_read_marker(id);
            timeline.room().send_multiple_receipts(r).await.map_err(|e| e.to_string())?;
        }
    }
    Ok(sent)
}

/// Say that we are (or are no longer) typing in a room.
pub async fn set_typing(client: &Client, room_id: &str, typing: bool) -> Result<(), String> {
    room_of(client, room_id)?.typing_notice(typing).await.map_err(|e| e.to_string())
}

/// Replace the text of one of our messages (Markdown becomes formatted text).
pub async fn edit_text(timeline: &Timeline, event_id: &str, text: &str) -> Result<(), String> { edit_text_full(timeline, event_id, text, &HashMap::new(), &[]).await }

/// `edit_text` with custom emoji and picked mentions, like `send_text_full`. The SDK puts the people not mentioned by the *original* message in
/// the edit's own `m.mentions` and all of them in `m.new_content` (spec: "Edits of events with mentions"); a later edit therefore repeats
/// people an earlier edit added (known limit: the SDK does not compare with the previous edit).
pub async fn edit_text_full(timeline: &Timeline, event_id: &str, text: &str, emoji: &HashMap<String, String>, mentions: &[(String, String)]) -> Result<(), String> {
    let id: OwnedEventId = event_id.try_into().map_err(|e| format!("bad event id: {e}"))?;
    let (msgtype, m) = text_content(text, emoji, mentions);
    let content = RoomMessageEventContentWithoutRelation::new(msgtype).add_mentions(m);
    timeline.edit(&TimelineEventItemId::EventId(id), matrix_sdk::room::edit::EditedContent::RoomMessage(content)).await.map_err(|e| e.to_string())
}

/// Delete one of our messages.
pub async fn redact(timeline: &Timeline, event_id: &str) -> Result<(), String> {
    let id: OwnedEventId = event_id.try_into().map_err(|e| format!("bad event id: {e}"))?;
    timeline.redact(&TimelineEventItemId::EventId(id), None).await.map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit::{FakeHs, ROOM};
    use matrix_sdk_ui::timeline::RoomExt;

    #[test]
    fn media_names_never_keep_a_program_extension() {
        assert_eq!(safe_media_name("video", "clip.bat"), "clip.vid");
        assert_eq!(safe_media_name("video", "../../x/clip.mp4.desktop"), "clip_mp4.vid");
        assert_eq!(safe_media_name("image", "a.HTA"), "a.img");
        assert_eq!(safe_media_name("audio", "song.opus"), "song.opus");
        assert_eq!(safe_media_name("image", "C:\\dir\\pic.PNG"), "pic.png");
        assert_eq!(safe_media_name("audio", ""), "media.aud");
        assert_eq!(safe_media_name("video", ".hidden"), "media.vid");
    }

    struct Fixture { hs: FakeHs, client: Client, timeline: Arc<Timeline>, _dir: tempfile::TempDir }

    async fn fixture() -> Fixture {
        let hs = FakeHs::start().await;
        let dir = tempfile::tempdir().unwrap();
        let client = crate::session::sign_in(dir.path(), &hs.uri(), "alice", "pw", None, "Vector").await.unwrap();
        crate::sync_once(&client).await.unwrap();
        let room = client.get_room(<&matrix_sdk::ruma::RoomId>::try_from(ROOM).unwrap()).unwrap();
        let timeline = Arc::new(room.timeline().await.unwrap());
        let _ = timeline.subscribe().await;
        Fixture { hs, client, timeline, _dir: dir }
    }

    /// Sync and look at the timeline until `done` is satisfied.
    async fn wait_for(f: &Fixture, done: impl Fn(&[UiMessage]) -> bool) -> Vec<UiMessage> {
        let mut last = Vec::new();
        for _ in 0..400 { /* generous: the whole suite runs in parallel and bob's login can be slow */
            crate::sync_once(&f.client).await.unwrap();
            let items: Vec<_> = f.timeline.items().await.iter().cloned().collect();
            last = ui_messages(&items, "@alice:hs");
            if last.iter().any(|m| m.kind == "undecryptable") { retry_undecryptable(&f.timeline).await; }
            if done(&last) { return last; }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        panic!("the timeline never looked as expected: {last:#?}");
    }

    #[test]
    fn a_picked_mention_becomes_a_matrix_to_link_and_only_when_its_name_is_still_in_the_text() {
        let m = vec![("@bob:hs".to_string(), "Bob".to_string()), ("@bs:hs".to_string(), "Bob Smith".to_string())];
        let h = with_mentions("hi @Bob Smith and @Bob <3", None, &m).unwrap();
        assert_eq!(h, "hi <a href=\"https://matrix.to/#/@bs:hs\">@Bob Smith</a> and <a href=\"https://matrix.to/#/@bob:hs\">@Bob</a> &lt;3");
        assert!(with_mentions("hi @Bobby", None, &m).is_none(), "@Bobby is not a mention of Bob");
        assert!(with_mentions("no names here", None, &m).is_none(), "an edited-away mention is not linked");
        let md = with_mentions("**hey** @Bob", Some("<strong>hey</strong> @Bob"), &m).unwrap();
        assert!(md.starts_with("<strong>hey</strong> <a href="));
    }

    #[tokio::test]
    async fn markdown_goes_out_formatted_and_incoming_html_is_sanitized() {
        let f = fixture().await;
        send_text(&f.timeline, "**bold** and `code`", None).await.unwrap();
        f.timeline.send(RoomMessageEventContent::text_html("shown", "<b>ok</b><script>alert(1)</script><a href=\"javascript:evil()\">link</a><img src=\"https://tracker.example/x.png\">").into()).await.unwrap();
        let msgs = wait_for(&f, |m| m.len() == 2 && m.iter().all(|x| !x.pending)).await;
        assert_eq!(msgs[0].body, "**bold** and `code`", "the plain text keeps the Markdown for clients that cannot show formatting");
        assert!(msgs[0].html.contains("<strong>bold</strong>") && msgs[0].html.contains("<code>code</code>"), "{}", msgs[0].html);
        assert!(msgs[1].html.contains("<b>ok</b>"));
        assert!(!msgs[1].html.contains("script") && !msgs[1].html.contains("javascript:") && !msgs[1].html.contains("tracker.example"), "dangerous markup is removed: {}", msgs[1].html);
        assert!(f.hs.room_events().iter().any(|e| e["type"] == "m.room.encrypted"), "in an encrypted room it travels as ciphertext");
    }

    #[tokio::test]
    async fn a_picked_mention_is_sent_in_m_mentions_and_as_a_link() {
        let f = fixture().await;
        send_text_full(&f.timeline, "hello @Bob, **welcome**", None, &HashMap::new(), &[("@bob:hs".to_string(), "Bob".to_string())]).await.unwrap();
        let msgs = wait_for(&f, |m| m.len() == 1 && !m[0].pending).await;
        assert!(msgs[0].html.contains("href=\"https://matrix.to/#/@bob:hs\"") && msgs[0].html.contains("<strong>welcome</strong>"), "{}", msgs[0].html);
        let items = f.timeline.items().await;
        let ev = items.iter().filter_map(|i| i.as_event()).next().unwrap();
        let ids = ev.content().as_message().unwrap().mentions().map(|m| m.user_ids.iter().map(|u| u.to_string()).collect::<Vec<_>>());
        assert_eq!(ids, Some(vec!["@bob:hs".to_string()]), "other clients notify from m.mentions, not from the text");
    }

    /// The event JSON as the client saw it after decryption (what other clients get to see), of the item at `index` among the events.
    async fn raw_event(f: &Fixture, index: usize) -> serde_json::Value {
        let ev = f.timeline.items().await.iter().filter_map(|i| i.as_event().cloned()).nth(index).unwrap();
        serde_json::from_str(ev.original_json().expect("a sent event has its JSON").json().get()).unwrap()
    }

    // ---- conformance with the Client-Server API v1.19 (https://spec.matrix.org/v1.19/client-server-api/) ----

    #[tokio::test]
    async fn spec_an_edit_keeps_the_mentions_and_links_of_the_text() {
        let f = fixture().await;
        send_text(&f.timeline, "hello", None).await.unwrap();
        let first = wait_for(&f, |m| m.len() == 1 && !m[0].pending).await[0].id.clone();
        let bob = [("@bob:hs".to_string(), "Bob".to_string())];
        edit_text_full(&f.timeline, &first, "hello @Bob", &HashMap::new(), &bob).await.unwrap();
        let msgs = wait_for(&f, |m| m.len() == 1 && m[0].edited && !m[0].pending).await;
        assert!(msgs[0].html.contains("href=\"https://matrix.to/#/@bob:hs\""), "the edited text links the mention: {}", msgs[0].html);
        let edit = latest_edit(&f).await;
        assert_eq!(edit["content"]["m.mentions"]["user_ids"], serde_json::json!(["@bob:hs"]), "Bob is newly mentioned: {edit}");
        assert_eq!(edit["content"]["m.new_content"]["m.mentions"]["user_ids"], serde_json::json!(["@bob:hs"]), "{edit}");
    }

    /// The JSON of the newest edit of the first message, as the client sees it.
    async fn latest_edit(f: &Fixture) -> serde_json::Value {
        for _ in 0..200 {
            crate::sync_once(&f.client).await.unwrap();
            let items = f.timeline.items().await;
            if let Some(j) = items.iter().filter_map(|i| i.as_event()).next().and_then(|e| e.latest_edit_json().cloned()) { return serde_json::from_str(j.json().get()).unwrap(); }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        panic!("no edit arrived");
    }

    #[tokio::test]
    async fn spec_every_message_carries_m_mentions_even_when_empty() {
        let f = fixture().await;
        send_text(&f.timeline, "hello Bob, plain", None).await.unwrap();
        for _ in 0..200 {
            crate::sync_once(&f.client).await.unwrap();
            if f.timeline.items().await.iter().filter_map(|i| i.as_event()).any(|e| e.event_id().is_some() && e.original_json().is_some()) { break; }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        let v = raw_event(&f, 0).await;
        assert_eq!(v["content"]["m.mentions"], serde_json::json!({}), "an empty m.mentions keeps old push rules from matching names in the text: {v}");
    }

    #[tokio::test]
    async fn spec_a_thread_reply_relates_to_the_root_with_a_reply_fallback() {
        let f = fixture().await;
        let room = f.client.get_room(<&matrix_sdk::ruma::RoomId>::try_from(ROOM).unwrap()).unwrap();
        let main = Arc::new(open_timeline(&room).await.unwrap());
        let _ = main.subscribe().await;
        send_text(&main, "root", None).await.unwrap();
        let root = wait_for(&f, |m| m.len() == 1 && !m[0].pending).await[0].id.clone();
        let thread = Arc::new(open_thread(&room, &root).await.unwrap());
        let _ = thread.subscribe().await;
        send_text(&thread, "in the thread", None).await.unwrap();
        let mut rel = serde_json::Value::Null;
        for _ in 0..120 {
            crate::sync_once(&f.client).await.unwrap();
            for it in thread.items().await.iter() {
                if let Some(e) = it.as_event() {
                    if e.content().as_message().map(|m| m.body() == "in the thread").unwrap_or(false) && e.event_id().is_some() {
                        if let Some(j) = e.original_json() { rel = serde_json::from_str::<serde_json::Value>(j.json().get()).unwrap()["content"]["m.relates_to"].clone(); }
                    }
                }
            }
            if !rel.is_null() { break; }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        assert_eq!(rel["rel_type"], "m.thread", "{rel}");
        assert_eq!(rel["event_id"], root.as_str(), "always the thread root");
        assert_eq!(rel["is_falling_back"], true, "unthreaded clients get a reply chain: {rel}");
        assert!(rel["m.in_reply_to"]["event_id"].is_string(), "{rel}");
    }

    #[tokio::test]
    async fn spec_a_caption_is_the_body_with_the_file_name_in_filename() {
        let f = fixture().await;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("dog.png");
        std::fs::write(&path, PNG).unwrap();
        send_file(&f.timeline, &path, Some("this is a **cat**")).await.unwrap();
        let mut v = serde_json::Value::Null;
        for _ in 0..200 {
            crate::sync_once(&f.client).await.unwrap();
            let evs: Vec<_> = f.timeline.items().await.iter().filter_map(|i| i.as_event().cloned()).collect();
            if let Some(e) = evs.first().filter(|e| e.event_id().is_some()) { v = serde_json::from_str(e.original_json().unwrap().json().get()).unwrap(); break; }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        let c = &v["content"];
        assert_eq!(c["msgtype"], "m.image", "{v}");
        assert_eq!(c["filename"], "dog.png", "the file name travels in filename (v1.10): {c}");
        assert_eq!(c["body"], "this is a **cat**", "body is the caption: {c}");
        assert!(c["formatted_body"].as_str().unwrap_or("").contains("<strong>cat</strong>"), "{c}");
    }

    #[tokio::test]
    async fn spec_the_reply_fallback_of_older_clients_is_removed_before_showing() {
        let f = fixture().await;
        send_text(&f.timeline, "the original", None).await.unwrap();
        let first = wait_for(&f, |m| m.len() == 1 && !m[0].pending).await[0].id.clone();
        let content = RoomMessageEventContent::text_html("> <@alice:hs> the original\n\nthe answer", "<mx-reply><blockquote>the original</blockquote></mx-reply>the answer");
        f.timeline.send_reply(RoomMessageEventContentWithoutRelation::new(content.msgtype), first.as_str().try_into().unwrap()).await.unwrap();
        let msgs = wait_for(&f, |m| m.len() == 2 && !m[1].pending).await;
        assert!(!msgs[1].html.contains("mx-reply") && !msgs[1].html.contains("blockquote"), "{}", msgs[1].html);
        assert_eq!(msgs[1].body, "the answer", "the quoted lines of the plain body are stripped too");
    }

    #[test]
    fn spec_spoilers_colours_and_only_those_survive_the_sanitizer() {
        let t = MessageType::Text(matrix_sdk::ruma::events::room::message::TextMessageEventContent::html("x",
            "<span data-mx-spoiler=\"why\">secret</span> <font data-mx-color=\"#ff0000\" onclick=\"x()\">red</font> <span style=\"position:fixed\">s</span> <a href=\"javascript:x()\">j</a> <a href=\"magnet:?xt=urn:btih:1\">m</a><ol start=\"5\"><li>five</li></ol>"));
        let h = html_of(&t);
        assert!(h.contains("data-mx-spoiler=\"why\"") && h.contains("data-mx-color=\"#ff0000\""), "{h}");
        assert!(!h.contains("onclick") && !h.contains("position") && !h.contains("javascript:"), "{h}");
        assert!(h.contains("magnet:"), "magnet links are allowed by the spec: {h}");
        assert!(h.contains("start=\"5\""), "a numbered list keeps its start (v1.18): {h}");
    }

    #[tokio::test]
    async fn spec_reading_also_moves_the_fully_read_marker() {
        let f = fixture().await;
        send_text(&f.timeline, "hello", None).await.unwrap();
        let id = wait_for(&f, |m| m.len() == 1 && !m[0].pending).await[0].id.clone();
        for _ in 0..20 { if mark_read(&f.timeline).await.unwrap_or(false) { break; } tokio::time::sleep(std::time::Duration::from_millis(100)).await; }
        let log = f.hs.log();
        assert!(log.iter().any(|l| l.starts_with("read_markers") && l.contains("m.fully_read") && l.contains(&id)), "m.fully_read is what other devices use for \"unread since here\": {log:#?}");
    }

    #[tokio::test]
    async fn spec_at_room_sets_m_mentions_room_and_no_link() {
        let f = fixture().await;
        send_text_full(&f.timeline, "@room dinner", None, &HashMap::new(), &[("@room".to_string(), "room".to_string())]).await.unwrap();
        let msgs = wait_for(&f, |m| m.len() == 1 && !m[0].pending).await;
        assert!(!msgs[0].html.contains("matrix.to"), "{}", msgs[0].html);
        let items = f.timeline.items().await;
        let ev = items.iter().filter_map(|i| i.as_event()).next().unwrap();
        assert!(ev.content().as_message().unwrap().mentions().map_or(false, |m| m.room), "m.mentions.room is what notifies the whole room");
    }

    #[test]
    fn spec_a_display_name_two_people_share_is_shown_with_the_user_id() {
        use matrix_sdk_ui::timeline::Profile;
        let p = |ambiguous| TimelineDetails::Ready(Profile { display_name: Some("Alice".into()), display_name_ambiguous: ambiguous, avatar_url: None });
        assert_eq!(name_of(&p(false), "@a:hs"), "Alice");
        assert_eq!(name_of(&p(true), "@a2:evil.example"), "Alice (@a2:evil.example)");
    }

    #[tokio::test]
    async fn spec_slash_me_is_sent_as_an_emote() {
        let f = fixture().await;
        send_text(&f.timeline, "/me waves **hard**", None).await.unwrap();
        let msgs = wait_for(&f, |m| m.len() == 1 && !m[0].pending).await;
        assert_eq!(msgs[0].kind, "emote");
        assert_eq!(msgs[0].body, "waves **hard**", "{:?}", msgs[0]);
        assert!(msgs[0].html.contains("<strong>hard</strong>"), "{}", msgs[0].html);
    }

    #[tokio::test]
    async fn a_reply_carries_the_relation_the_mention_and_no_quoted_fallback() {
        let f = fixture().await;
        tokio::spawn(crate::testkit::bob_says(f.hs.clone(), vec!["first message".to_string()]));
        let first = wait_for(&f, |m| m.len() == 1 && m[0].body == "first message").await[0].id.clone();
        send_text(&f.timeline, "an answer", Some(&first)).await.unwrap();
        let msgs = wait_for(&f, |m| m.len() == 2 && !m[1].pending).await;
        let r = msgs[1].reply.as_ref().expect("the second message is a reply");
        assert_eq!(r.event_id, first);
        assert_eq!(r.sender, "bob");
        assert_eq!(r.preview, "first message");
        assert_eq!(msgs[1].body, "an answer", "no quoted fallback in the body");
        let ev = f.timeline.items().await.iter().filter_map(|i| i.as_event().cloned()).nth(1).unwrap();
        assert_eq!(ev.content().in_reply_to().unwrap().event_id.to_string(), first);
        assert_eq!(ev.content().as_message().unwrap().mentions().map(|m| m.user_ids.iter().map(|u| u.to_string()).collect::<Vec<_>>()), Some(vec!["@bob:hs".to_string()]), "the author of the replied-to message is mentioned");
    }

    #[tokio::test]
    async fn fetching_the_details_of_a_loaded_reply_is_harmless() {
        let f = fixture().await;
        tokio::spawn(crate::testkit::bob_says(f.hs.clone(), vec!["original".to_string()]));
        let first = wait_for(&f, |m| m.len() == 1).await[0].id.clone();
        send_text(&f.timeline, "answer", Some(&first)).await.unwrap();
        let msgs = wait_for(&f, |m| m.len() == 2 && !m[1].pending).await;
        load_reply(&f.timeline, &msgs[1].id).await.unwrap();
        assert_eq!(wait_for(&f, |m| m.len() == 2 && m[1].reply.as_ref().is_some_and(|r| r.preview == "original")).await.len(), 2);
    }

    #[tokio::test]
    async fn reactions_are_counted_toggled_and_marked_as_ours() {
        let f = fixture().await;
        send_text(&f.timeline, "react to me", None).await.unwrap();
        let id = wait_for(&f, |m| m.len() == 1 && !m[0].pending).await[0].id.clone();
        let added = toggle_reaction(&f.timeline, &id, "👍").await;
        assert!(added.clone().unwrap_or(false), "added: {added:?}; unhandled requests: {:#?}", f.hs.log().iter().filter(|l| l.contains("UNHANDLED")).collect::<Vec<_>>());
        let msgs = wait_for(&f, |m| m[0].reactions.len() == 1).await;
        assert_eq!(msgs[0].reactions, vec![UiReaction { key: "👍".into(), count: 1, mine: true }]);
        assert!(!toggle_reaction(&f.timeline, &id, "👍").await.unwrap(), "removed again");
        wait_for(&f, |m| m[0].reactions.is_empty()).await;
    }

    /// A tiny valid PNG (1x1, red).
    const PNG: &[u8] = &[0x89,0x50,0x4e,0x47,0x0d,0x0a,0x1a,0x0a,0,0,0,0x0d,0x49,0x48,0x44,0x52,0,0,0,1,0,0,0,1,8,2,0,0,0,0x90,0x77,0x53,0xde,0,0,0,0x0c,0x49,0x44,0x41,0x54,8,0xd7,0x63,0xf8,0xcf,0xc0,0,0,3,1,1,0,0x18,0xdd,0x8d,0xb0,0,0,0,0,0x49,0x45,0x4e,0x44,0xae,0x42,0x60,0x82];

    #[tokio::test]
    async fn a_picture_is_sent_encrypted_and_comes_back_as_the_same_bytes() {
        let f = fixture().await;
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("red.png");
        std::fs::write(&file, PNG).unwrap();
        send_file(&f.timeline, &file, Some("a red dot")).await.unwrap_or_else(|e| panic!("{e}; unhandled: {:#?}", f.hs.log().iter().filter(|l| l.contains("UNHANDLED")).collect::<Vec<_>>()));
        let msgs = wait_for(&f, |m| m.len() == 1 && m[0].kind == "image" && !m[0].pending).await;
        assert_eq!(msgs[0].file_name, "red.png");
        let up = f.hs.uploaded();
        assert_eq!(up.len(), 1, "one upload");
        assert_ne!(up[0], PNG, "what the server holds is not the picture: the room is encrypted");
        let items: Vec<_> = f.timeline.items().await.iter().cloned().collect();
        let cache = dir.path().join("cache");
        let got = fetch_images(&f.client, &items, &cache, &HashMap::new(), true).await;
        assert_eq!(got.len(), 1);
        let pic = got.values().next().unwrap();
        assert!(!pic.thumb, "this picture carries no thumbnail");
        assert_eq!(std::fs::read(&pic.path).unwrap(), PNG, "downloaded and decrypted to the original bytes");
        let again = ui_messages_with(&items, "@alice:hs", &got);
        assert!(again[0].image_path.ends_with(".png"));
    }

    /// What a picture and its thumbnail are in the thumbnail tests: the engine does not decode them, so any bytes do.
    fn big() -> Vec<u8> { vec![1u8; 300_000] }
    fn small() -> Vec<u8> { vec![2u8; 5_000] }

    async fn items_of(f: &Fixture) -> Vec<Arc<TimelineItem>> { f.timeline.items().await.iter().cloned().collect() }

    #[tokio::test]
    async fn the_timeline_gets_the_servers_thumbnail_when_thumbnails_are_on_and_the_picture_itself_when_they_go_off() {
        let f = fixture().await;
        f.hs.put_picture("photo", big(), Some(small()));
        let id = f.hs.say_picture("bob", "photo", Some(3_000_000), None);
        wait_for(&f, |m| m.len() == 1 && m[0].kind == "image").await;
        let (items, dir) = (items_of(&f).await, tempfile::tempdir().unwrap());
        let got = fetch_images(&f.client, &items, dir.path(), &HashMap::new(), true).await;
        assert!(got[&id].thumb);
        assert_eq!(std::fs::read(&got[&id].path).unwrap(), small(), "the thumbnail's bytes");
        let log = f.hs.log();
        assert!(log.iter().any(|l| l == "thumbnail pic_photo 960x640 scale"), "asked for a scaled thumbnail the timeline can draw sharp at twice the pixels: {log:?}");
        assert!(!log.iter().any(|l| l == "download pic_photo"), "the picture itself was not downloaded: {log:?}");
        let row = &ui_messages_with(&items, "@alice:hs", &got)[0];
        assert!(row.image_thumb && row.image_path.ends_with("-t.img"), "the row says its picture is a thumbnail: {} {}", row.image_thumb, row.image_path);
        assert_eq!((row.image_w, row.image_h), (4000, 3000), "the row keeps the size of the picture, so the layout does not move when the original arrives");
        assert!(serde_json::to_value(row).unwrap().get("image_thumb").is_some());
        /* switched off: what is held is a thumbnail, so the picture is fetched and replaces it */
        let again = fetch_images(&f.client, &items, dir.path(), &got, false).await;
        assert!(!again[&id].thumb);
        assert_eq!(std::fs::read(&again[&id].path).unwrap(), big());
        let row = &ui_messages_with(&items, "@alice:hs", &again)[0];
        assert!(!row.image_thumb && !row.image_path.ends_with("-t.img"));
        assert!(serde_json::to_value(row).unwrap().get("image_thumb").is_none(), "a picture that is not a thumbnail says nothing");
        /* and switched on again, the picture it already has is good enough */
        assert!(fetch_images(&f.client, &items, dir.path(), &again, true).await.is_empty());
        assert_eq!(f.hs.log().iter().filter(|l| l.starts_with("thumbnail ")).count(), 1, "one thumbnail request in all");
    }

    #[tokio::test]
    async fn small_pictures_and_pictures_without_a_thumbnail_come_whole() {
        let f = fixture().await;
        f.hs.put_picture("tiny", big(), Some(small()));
        f.hs.put_picture("plain", big(), None);
        f.hs.put_picture("mini", small(), None);
        f.hs.put_picture("sent", big(), None);
        let tiny = f.hs.say_picture("bob", "tiny", Some(100_000), None);
        let plain = f.hs.say_picture("bob", "plain", Some(3_000_000), None);
        let sent = f.hs.say_picture("bob", "sent", Some(3_000_000), Some("mini"));
        let unknown = { f.hs.put_picture("unknown", big(), Some(small())); f.hs.say_picture("bob", "unknown", None, None) };
        wait_for(&f, |m| m.len() == 4).await;
        let (items, dir) = (items_of(&f).await, tempfile::tempdir().unwrap());
        let got = fetch_images(&f.client, &items, dir.path(), &HashMap::new(), true).await;
        assert!(!got[&tiny].thumb && std::fs::read(&got[&tiny].path).unwrap() == big(), "declared small: a thumbnail would hardly be smaller");
        assert!(!f.hs.log().iter().any(|l| l.starts_with("thumbnail pic_tiny")));
        assert!(!got[&plain].thumb && std::fs::read(&got[&plain].path).unwrap() == big(), "the server has no thumbnail and the sender sent none: the picture");
        assert!(f.hs.log().iter().any(|l| l.starts_with("thumbnail pic_plain")), "it was asked first");
        assert!(got[&sent].thumb && std::fs::read(&got[&sent].path).unwrap() == small(), "the server has none, the sender's own thumbnail does");
        assert!(got[&unknown].thumb, "a message that does not say how big the picture is gets the thumbnail");
    }

    #[tokio::test]
    async fn an_encrypted_picture_shows_the_thumbnail_its_sender_attached_and_otherwise_comes_whole() {
        use matrix_sdk::attachment::{AttachmentInfo, BaseImageInfo, Thumbnail};
        use matrix_sdk_ui::timeline::AttachmentConfig;
        let f = fixture().await;
        let dir = tempfile::tempdir().unwrap();
        let (with, without) = (dir.path().join("with.jpg"), dir.path().join("without.jpg"));
        std::fs::write(&with, big()).unwrap();
        std::fs::write(&without, big()).unwrap();
        let info = || AttachmentInfo::Image(BaseImageInfo { width: Some(4000u32.into()), height: Some(3000u32.into()), size: Some(3_000_000u32.into()), blurhash: None, is_animated: None });
        let thumb = Thumbnail { data: small(), content_type: mime::IMAGE_JPEG, height: 600u32.into(), width: 800u32.into(), size: (small().len() as u32).into() };
        f.timeline.send_attachment(with.clone(), mime::IMAGE_JPEG, AttachmentConfig { info: Some(info()), thumbnail: Some(thumb), ..Default::default() }).await.unwrap();
        wait_for(&f, |m| m.len() == 1 && m[0].kind == "image" && !m[0].pending).await;
        f.timeline.send_attachment(without.clone(), mime::IMAGE_JPEG, AttachmentConfig { info: Some(info()), ..Default::default() }).await.unwrap();
        let msgs = wait_for(&f, |m| m.len() == 2 && m.iter().all(|x| !x.pending)).await;
        let (a, b) = (msgs[0].id.clone(), msgs[1].id.clone());
        assert_eq!(f.hs.uploaded().len(), 3, "two pictures and one thumbnail, all encrypted");
        let items = items_of(&f).await;
        let got = fetch_images(&f.client, &items, &dir.path().join("cache"), &HashMap::new(), true).await;
        assert!(got[&a].thumb && std::fs::read(&got[&a].path).unwrap() == small(), "the sender's thumbnail, decrypted");
        assert!(!got[&b].thumb && std::fs::read(&got[&b].path).unwrap() == big(), "no thumbnail was sent and the server cannot make one of what it cannot read");
        assert!(!f.hs.log().iter().any(|l| l.starts_with("thumbnail ")), "an encrypted picture is never asked of the thumbnail endpoint");
        let off = fetch_images(&f.client, &items, &dir.path().join("cache"), &got, false).await;
        assert!(off.len() == 1 && !off[&a].thumb && std::fs::read(&off[&a].path).unwrap() == big(), "switched off: the first one comes whole too");
    }

    #[tokio::test]
    async fn pictures_from_an_earlier_run_are_used_and_a_download_is_never_left_half_written() {
        let f = fixture().await;
        f.hs.put_picture("one", big(), Some(small()));
        f.hs.put_picture("two", big(), Some(small()));
        let one = f.hs.say_picture("bob", "one", Some(3_000_000), None);
        let two = f.hs.say_picture("bob", "two", Some(3_000_000), None);
        wait_for(&f, |m| m.len() == 2).await;
        let (items, dir) = (items_of(&f).await, tempfile::tempdir().unwrap());
        let first = fetch_images(&f.client, &items, dir.path(), &HashMap::new(), true).await;
        assert_eq!(first.len(), 2);
        let asked = f.hs.log().len();
        let again = fetch_images(&f.client, &items, dir.path(), &HashMap::new(), true).await; /* a new run: nothing is held in memory */
        assert_eq!(again, first, "the files of the first run are used");
        assert_eq!(f.hs.log().len(), asked, "and nothing is asked of the server");
        let names: Vec<String> = std::fs::read_dir(dir.path()).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        assert!(names.iter().all(|n| !n.ends_with(".part")), "no half-written file stays: {names:?}");
        /* the picture itself, when it is there, wins over the thumbnail; off, the thumbnail is not used at all */
        std::fs::write(dir.path().join(format!("{}.jpg", one.chars().filter(|c| c.is_ascii_alphanumeric()).collect::<String>())), big()).unwrap();
        let better = fetch_images(&f.client, &items, dir.path(), &HashMap::new(), true).await;
        assert!(!better[&one].thumb && better[&two].thumb);
        let off = fetch_images(&f.client, &items, dir.path(), &HashMap::new(), false).await;
        assert!(!off[&one].thumb && !off[&two].thumb, "off: the picture on disk is used, the other one is downloaded");
        assert_eq!(std::fs::read(&off[&two].path).unwrap(), big());
    }

    #[tokio::test]
    async fn pictures_come_in_batches_of_eight_the_newest_first() {
        let f = fixture().await;
        f.hs.put_picture("p", big(), None);
        let ids: Vec<String> = (0..11).map(|_| f.hs.say_picture("bob", "p", Some(100), None)).collect();
        wait_for(&f, |m| m.len() == 11).await;
        let (items, dir) = (items_of(&f).await, tempfile::tempdir().unwrap());
        let first = fetch_images(&f.client, &items, dir.path(), &HashMap::new(), false).await;
        let mut got: Vec<&String> = first.keys().collect();
        got.sort();
        let mut newest: Vec<&String> = ids[3..].iter().collect();
        newest.sort();
        assert_eq!(got, newest, "the eight newest");
        let mut have = first;
        let rest = fetch_images(&f.client, &items, dir.path(), &have, false).await;
        assert_eq!(rest.len(), 3, "the three older ones");
        have.extend(rest);
        assert!(fetch_images(&f.client, &items, dir.path(), &have, false).await.is_empty());
    }

    #[tokio::test]
    async fn several_pictures_are_one_gallery_message_that_comes_back_item_by_item() {
        let f = fixture().await;
        let dir = tempfile::tempdir().unwrap();
        let mut files = Vec::new();
        for (n, name) in ["a.png", "b.png", "c.png"].iter().enumerate() {
            let mut bytes = PNG.to_vec(); bytes.extend_from_slice(&[n as u8; 4]); /* trailing bytes: the three files differ */
            let p = dir.path().join(name); std::fs::write(&p, &bytes).unwrap(); files.push(p);
        }
        send_gallery(&f.timeline, &files, Some("three dots")).await.unwrap_or_else(|e| panic!("{e}; unhandled: {:#?}", f.hs.log().iter().filter(|l| l.contains("UNHANDLED")).collect::<Vec<_>>()));
        let msgs = wait_for(&f, |m| m.len() == 1 && m[0].kind == "gallery" && !m[0].pending).await;
        assert_eq!(msgs[0].body, "three dots");
        assert_eq!(msgs[0].gallery.iter().map(|g| (g.name.as_str(), g.kind.as_str())).collect::<Vec<_>>(), vec![("a.png", "image"), ("b.png", "image"), ("c.png", "image")]);
        assert_eq!(f.hs.room_events().iter().filter(|e| e["type"] == "m.room.encrypted").count(), 1, "one event for all three");
        assert_eq!(f.hs.uploaded().len(), 3);
        let items: Vec<_> = f.timeline.items().await.iter().cloned().collect();
        let got = fetch_images(&f.client, &items, &dir.path().join("cache"), &HashMap::new(), true).await;
        assert_eq!(got.len(), 3);
        let id = msgs[0].id.clone();
        for (n, p) in files.iter().enumerate() {
            assert_eq!(std::fs::read(&got[&format!("{id}#{n}")].path).unwrap(), std::fs::read(p).unwrap(), "item {n}");
        }
        let rows = ui_messages_with(&items, "@alice:hs", &got);
        assert!(rows[0].gallery.iter().all(|g| g.image_path.ends_with(".png")));
        let dest = dir.path().join("saved.png");
        save_attachment(&f.client, &f.timeline, &format!("{id}#1"), &dest).await.unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), std::fs::read(&files[1]).unwrap(), "item 1 saved on its own");
        assert!(save_attachment(&f.client, &f.timeline, &format!("{id}#7"), &dest).await.is_err());
        let opened = media_copy(&f.client, &f.timeline, &format!("{id}#2"), &dir.path().join("open")).await.unwrap();
        assert_eq!(std::fs::read(opened).unwrap(), std::fs::read(&files[2]).unwrap());
    }

    #[tokio::test]
    async fn one_file_is_not_a_gallery() {
        let f = fixture().await;
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("one.png"); std::fs::write(&p, PNG).unwrap();
        send_gallery(&f.timeline, &[p], None).await.unwrap();
        wait_for(&f, |m| m.len() == 1 && m[0].kind == "image" && !m[0].pending).await;
        assert!(send_gallery(&f.timeline, &[], None).await.is_err());
    }

    #[tokio::test]
    async fn a_deleted_message_leaves_the_timeline_view_of_text() {
        let f = fixture().await;
        send_text(&f.timeline, "oops", None).await.unwrap();
        let id = wait_for(&f, |m| m.len() == 1 && !m[0].pending).await[0].id.clone();
        redact(&f.timeline, &id).await.unwrap();
        let msgs = wait_for(&f, |m| m.iter().all(|x| x.body != "oops")).await;
        assert!(msgs.iter().all(|m| m.kind != "text" || m.body != "oops"));
    }

    #[tokio::test]
    async fn an_edit_replaces_the_text_and_marks_the_message() {
        let f = fixture().await;
        send_text(&f.timeline, "typo", None).await.unwrap();
        let id = wait_for(&f, |m| m.len() == 1 && !m[0].pending).await[0].id.clone();
        edit_text(&f.timeline, &id, "fixed **it**").await.unwrap();
        let m = wait_for(&f, |m| m.len() == 1 && m[0].edited && m[0].body.contains("fixed")).await;
        assert_eq!(m[0].id, id);
        assert!(m[0].html.contains("<strong>it</strong>"), "{:?}", m[0].html);
        let mut sent = 0;
        for _ in 0..100 { sent = f.hs.room_events().iter().filter(|e| e["type"] == "m.room.encrypted").count(); if sent == 2 { break; } tokio::time::sleep(std::time::Duration::from_millis(50)).await; }
        assert_eq!(sent, 2, "the edit travelled encrypted too: {:?}", f.hs.room_events());
    }

    #[tokio::test]
    async fn an_attachment_can_be_saved_from_the_timeline_byte_for_byte() {
        let f = fixture().await;
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("notes.bin");
        let data: Vec<u8> = (0..5000u32).map(|i| (i * 7 % 251) as u8).collect();
        std::fs::write(&src, &data).unwrap();
        send_file(&f.timeline, &src, None).await.unwrap();
        let m = wait_for(&f, |m| m.len() == 1 && m[0].kind == "file" && !m[0].pending).await;
        assert_eq!(m[0].file_name, "notes.bin");
        let out = dir.path().join("saved.bin");
        assert_eq!(save_attachment(&f.client, &f.timeline, &m[0].id, &out).await.unwrap(), 5000);
        assert_eq!(std::fs::read(&out).unwrap(), data);
        assert!(save_attachment(&f.client, &f.timeline, "$nope", &out).await.is_err());
    }

    #[tokio::test]
    async fn room_details_give_topic_encryption_and_members_with_roles() {
        let f = fixture().await;
        let d = room_details(&f.client, ROOM).await.unwrap_or_else(|e| panic!("{e}; {:#?}", f.hs.log().iter().filter(|l| l.contains("UNHANDLED")).collect::<Vec<_>>()));
        assert_eq!(d.topic, "Where alice and bob test the Rust rebuild");
        assert!(d.encrypted);
        let names: Vec<_> = d.members.iter().map(|m| (m.name.as_str(), m.role.as_str())).collect();
        assert_eq!(names, vec![("alice", "Admin"), ("bob", "")], "{names:?}");
    }

    /// How many times the whole member list has been asked for.
    fn member_list_requests(f: &Fixture) -> usize { f.hs.log().iter().filter(|l| l.starts_with("GET ") && l.contains("/members (")).count() }

    fn localparts(members: &[matrix_sdk::room::RoomMember]) -> Vec<String> { let mut v: Vec<String> = members.iter().map(|m| m.user_id().localpart().to_string()).collect(); v.sort(); v }

    #[tokio::test]
    async fn every_sync_asks_for_lazy_loading_of_members_and_for_no_presence() {
        let f = fixture().await;
        crate::sync_once(&f.client).await.unwrap();
        let filters: Vec<serde_json::Value> = f.hs.log().iter().filter_map(|l| l.strip_prefix("sync filter ")).filter_map(|j| serde_json::from_str(j).ok()).collect();
        assert!(filters.len() >= 2, "the fixture's syncs and this one all sent a filter: {filters:?}");
        for filter in &filters {
            assert_eq!(filter["room"]["state"]["lazy_load_members"], true, "{filter}");
            assert_eq!(filter["presence"]["not_types"], serde_json::json!(["*"]), "{filter}");
        }
    }

    #[tokio::test]
    async fn a_sync_brings_the_people_who_wrote_and_the_heroes_and_the_rest_wait_for_a_request() {
        let f = fixture().await;
        for n in ["carol", "dave", "erin", "frank", "gina", "hugo", "ivan"] { f.hs.add_member(n); }
        f.hs.say_text("ivan", "I am here");
        crate::sync_once(&f.client).await.unwrap();
        wait_for(&f, |m| m.iter().any(|x| x.body == "I am here")).await;
        let room = f.client.get_room(<&matrix_sdk::ruma::RoomId>::try_from(ROOM).unwrap()).unwrap();
        assert_eq!(room.joined_members_count(), 9, "the summary has the count");
        assert_eq!(localparts(&room.members_no_sync(matrix_sdk::RoomMemberships::JOIN).await.unwrap()), ["alice", "bob", "carol", "dave", "erin", "frank", "ivan"],
            "us, the first five others (the heroes) and the one who wrote; not gina and hugo");
        assert!(!room.are_members_synced());
        assert_eq!(member_list_requests(&f), 0, "nothing asked for the whole list so far");
        /* a big room: the details say so and list the ones that are known */
        let part = room_details_for(&f.client, ROOM, false, 5).await.unwrap();
        assert!(!part.members_complete);
        assert_eq!((part.member_count, part.members.len()), (9, 7));
        assert_eq!(member_list_requests(&f), 0, "still not");
        /* asked for: one request, then everybody */
        let all = room_details_for(&f.client, ROOM, true, 5).await.unwrap();
        assert!(all.members_complete && all.member_count == 9 && all.members.len() == 9, "{}", all.members.len());
        assert_eq!(member_list_requests(&f), 1);
        assert!(room_details_for(&f.client, ROOM, false, 5).await.unwrap().members_complete, "what has been loaded stays");
        assert_eq!(member_list_requests(&f), 1);
    }

    #[tokio::test]
    async fn a_small_room_lists_everybody_when_it_is_opened() {
        let f = fixture().await;
        for n in ["carol", "dave", "erin", "frank", "gina"] { f.hs.add_member(n); }
        crate::sync_once(&f.client).await.unwrap();
        let d = room_details(&f.client, ROOM).await.unwrap();
        assert!(d.members_complete && d.member_count == 7 && d.members.len() == 7, "{} {}", d.member_count, d.members.len());
        assert_eq!(member_list_requests(&f), 1);
    }

    #[tokio::test]
    async fn a_sender_nobody_has_heard_of_gets_a_name_and_a_picture_once_the_member_list_is_in() {
        use matrix_sdk_ui::timeline::TimelineDetails;
        let f = fixture().await;
        f.hs.add_member("gina");
        f.hs.add_member("hugo"); /* alice's heroes are bob, gina and hugo: add more people so that gina is not one of them */
        for n in ["ivan", "jack", "kate"] { f.hs.add_member(n); }
        f.hs.set_history_by(&["kate"], 2);
        f.hs.say_text("alice", "newest"); /* not sent through the timeline: sending into an encrypted room asks for the whole member list first */
        wait_for(&f, |m| m.iter().any(|x| x.body == "newest")).await;
        for _ in 0..3 { if load_older(&f.timeline).await.unwrap_or(false) { break; } }
        wait_for(&f, |m| m.iter().filter(|x| x.body.starts_with("old message")).count() == 2).await;
        let profile = |items: Vec<Arc<TimelineItem>>| items.iter().filter_map(|i| i.as_event()).find(|e| e.sender().as_str() == "@kate:hs").map(|e| e.sender_profile().clone());
        let before = profile(items_of(&f).await).unwrap();
        assert!(matches!(before, TimelineDetails::Unavailable), "kate wrote long ago and her member event never came: {before:?}");
        assert_eq!(ui_messages(&items_of(&f).await, "@alice:hs").iter().find(|m| m.sender_id == "@kate:hs").unwrap().sender, "kate", "her user name stands in for her name");
        f.timeline.fetch_members().await;
        let after = profile(items_of(&f).await).unwrap();
        match after { TimelineDetails::Ready(p) => assert_eq!(p.avatar_url.as_deref().map(|u| u.as_str()), Some("mxc://hs/av_kate")), other => panic!("{other:?}") }
        assert_eq!(member_list_requests(&f), 1);
    }

    #[tokio::test]
    async fn invites_are_listed_and_accepted_and_rooms_and_dms_can_be_created_and_left() {
        let f = fixture().await;
        let inv = f.hs.invite_alice("Bob's club");
        crate::sync_once(&f.client).await.unwrap();
        let rooms = ui_rooms(&f.client).await;
        assert_eq!(rooms[0].section, "Invites", "{rooms:?}");
        assert!(rooms[0].invite && rooms[0].title == "Bob's club", "{rooms:?}");
        accept_invite(&f.client, &inv).await.unwrap_or_else(|e| panic!("{e} {:#?}", f.hs.log().iter().filter(|l| l.contains("UNHANDLED")).collect::<Vec<_>>()));
        crate::sync_once(&f.client).await.unwrap();
        let rooms = ui_rooms(&f.client).await;
        assert!(rooms.iter().any(|r| r.id == inv && !r.invite), "{rooms:?}");

        let id = create_room(&f.client, "Plans", "next steps", true).await.unwrap_or_else(|e| panic!("{e} {:#?}", f.hs.log().iter().filter(|l| l.contains("UNHANDLED")).collect::<Vec<_>>()));
        let dm = start_dm(&f.client, "@bob:hs").await.unwrap();
        assert!(start_dm(&f.client, "bob").await.is_err());
        crate::sync_once(&f.client).await.unwrap();
        let rooms = ui_rooms(&f.client).await;
        assert!(rooms.iter().any(|r| r.id == id && r.title == "Plans"), "{rooms:?}");
        assert!(rooms.iter().any(|r| r.id == dm), "{rooms:?}");
        let log = f.hs.log();
        assert!(log.iter().any(|l| l == "createRoom name=Plans direct=null encrypted=true"), "{log:#?}");
        assert!(log.iter().any(|l| l.starts_with("createRoom name=bob direct=true")), "{log:#?}");

        leave_room(&f.client, &id).await.unwrap();
        crate::sync_once(&f.client).await.unwrap();
        let log = f.hs.log();
        assert!(log.iter().all(|l| !l.contains("UNHANDLED") || l.contains("well-known")), "{log:#?}");
    }

    #[tokio::test]
    async fn reading_sends_a_receipt_for_the_newest_event_and_typing_is_announced() {
        let f = fixture().await;
        send_text(&f.timeline, "hello", None).await.unwrap();
        let id = wait_for(&f, |m| m.len() == 1 && !m[0].pending).await[0].id.clone();
        let mut sent = false;
        for _ in 0..20 { if mark_read(&f.timeline).await.unwrap_or(false) { sent = true; break; } tokio::time::sleep(std::time::Duration::from_millis(100)).await; }
        let log = f.hs.log();
        assert!(sent && log.iter().any(|l| l.starts_with("receipt m.read") && l.ends_with(&id)), "{:#?}", log);
        set_typing(&f.client, ROOM, true).await.unwrap();
        set_typing(&f.client, ROOM, false).await.unwrap();
        let log = f.hs.log();
        assert!(log.iter().any(|l| l == "typing true") && log.iter().any(|l| l == "typing false"), "{:#?}", log);
    }

    fn row(id: &str, unread: u32) -> UiRoom { UiRoom { id: id.into(), title: id.into(), section: "Rooms".into(), unread, highlight: false, invite: false, ..Default::default() } }

    #[test]
    fn alerts_come_from_rooms_whose_unread_count_grew_except_the_open_one() {
        let mut seen = None;
        assert!(new_alerts(&mut seen, &[row("a", 5), row("b", 0)], None).is_empty(), "the first look only learns");
        let a = new_alerts(&mut seen, &[row("a", 5), row("b", 2), row("c", 1)], None);
        assert_eq!(a.iter().map(|x| (x.room_id.as_str(), x.new)).collect::<Vec<_>>(), vec![("b", 2), ("c", 1)]);
        assert!(new_alerts(&mut seen, &[row("a", 5), row("b", 2), row("c", 1)], None).is_empty(), "nothing new, nothing said");
        let open = new_alerts(&mut seen, &[row("a", 6), row("b", 3), row("c", 0)], Some("b"));
        assert_eq!(open.iter().map(|x| x.room_id.as_str()).collect::<Vec<_>>(), vec!["a"], "the focused open room stays quiet, a read room too");
    }

    #[tokio::test]
    async fn other_peoples_typing_is_reported_by_name() {
        let f = fixture().await;
        let seen: Arc<std::sync::Mutex<Vec<Vec<String>>>> = Default::default();
        let sink = seen.clone();
        let task = watch_typing(&f.client, ROOM, move |n| sink.lock().unwrap().push(n)).unwrap();
        f.hs.set_typing(&["bob"]);
        for _ in 0..40 { crate::sync_once(&f.client).await.unwrap(); if !seen.lock().unwrap().is_empty() { break; } tokio::time::sleep(std::time::Duration::from_millis(50)).await; }
        f.hs.set_typing(&[]);
        for _ in 0..40 { crate::sync_once(&f.client).await.unwrap(); if seen.lock().unwrap().len() >= 2 { break; } tokio::time::sleep(std::time::Duration::from_millis(50)).await; }
        task.abort();
        let seen = seen.lock().unwrap().clone();
        assert_eq!(seen.first(), Some(&vec!["bob".to_string()]), "{seen:?}");
        assert_eq!(seen.last(), Some(&Vec::<String>::new()), "{seen:?}");
    }

    #[tokio::test]
    async fn other_peoples_read_receipts_show_on_the_message() {
        let f = fixture().await;
        send_text(&f.timeline, "read me", None).await.unwrap();
        let id = wait_for(&f, |m| m.len() == 1 && !m[0].pending).await[0].id.clone();
        f.hs.read_by("bob", &id);
        let m = wait_for(&f, |m| m.len() == 1 && !m[0].seen_by.is_empty()).await;
        assert_eq!(m[0].seen_by, vec!["bob".to_string()]);
    }

    #[tokio::test]
    async fn older_messages_load_on_request_until_the_start_of_the_room() {
        let f = fixture().await;
        f.hs.set_history(5);
        send_text(&f.timeline, "newest", None).await.unwrap();
        wait_for(&f, |m| m.iter().any(|x| x.body == "newest")).await;
        let mut reached = false;
        for _ in 0..3 { if load_older(&f.timeline).await.unwrap_or_else(|e| panic!("{e}; {:#?}", f.hs.log())) { reached = true; break; } }
        assert!(reached, "the start of the room is reported after at most a few pages: {:#?}", f.hs.log().iter().filter(|l| l.contains("messages")).collect::<Vec<_>>());
        let m = wait_for(&f, |m| m.iter().any(|x| x.body == "old message 0")).await;
        let bodies: Vec<&str> = m.iter().map(|x| x.body.as_str()).collect();
        assert_eq!(bodies, vec!["old message 0", "old message 1", "old message 2", "old message 3", "old message 4", "newest"], "{bodies:?}");
    }

    #[tokio::test]
    async fn search_finds_loaded_messages_by_words_newest_first() {
        let f = fixture().await;
        f.hs.set_history(3);
        for t in ["Lunch at noon", "lunch is cancelled", "something else"] { send_text(&f.timeline, t, None).await.unwrap(); }
        wait_for(&f, |m| m.len() == 3 && m.iter().all(|x| !x.pending)).await;
        for _ in 0..3 { if load_older(&f.timeline).await.unwrap_or(false) { break; } }
        wait_for(&f, |m| m.len() == 6).await;
        let items: Vec<_> = f.timeline.items().await.iter().cloned().collect();
        let hits = search_messages(&items, "@alice:hs", "LUNCH", 10);
        assert_eq!(hits.iter().map(|m| m.body.as_str()).collect::<Vec<_>>(), vec!["lunch is cancelled", "Lunch at noon"]);
        assert_eq!(search_messages(&items, "@alice:hs", "old 1", 10).len(), 1, "words match anywhere, several words must all match");
        assert_eq!(search_messages(&items, "@alice:hs", "bob", 10).len(), 3, "the sender counts");
        assert!(search_messages(&items, "@alice:hs", "  ", 10).is_empty());
    }

    #[tokio::test]
    async fn an_admin_can_moderate_and_edit_and_knows_what_they_may_do() {
        let f = fixture().await;
        let d = room_details(&f.client, ROOM).await.unwrap();
        assert!(d.can_edit && d.can_set_roles);
        let bob = d.members.iter().find(|m| m.name == "bob").unwrap();
        assert!(bob.can_kick && bob.can_ban);
        assert!(!d.members.iter().find(|m| m.name == "alice").unwrap().can_kick, "never offered on oneself");

        kick_member(&f.client, ROOM, "@bob:hs", "").await.unwrap();
        ban_member(&f.client, ROOM, "@bob:hs", "spam").await.unwrap();
        set_room_texts(&f.client, ROOM, "New name", "New topic").await.unwrap();
        set_member_role(&f.client, ROOM, "@bob:hs", "moderator").await.unwrap();
        assert!(set_member_role(&f.client, ROOM, "@bob:hs", "king").await.is_err());
        assert!(kick_member(&f.client, ROOM, "bob", "").await.is_err());
        let log = f.hs.log();
        for want in ["moderation kick @bob:hs ", "moderation ban @bob:hs spam", "state m.room.name {\"name\":\"New name\"}"] {
            assert!(log.iter().any(|l| l == want), "missing {want}: {:#?}", log.iter().filter(|l| l.starts_with("state") || l.starts_with("moderation") || l.contains("UNHANDLED")).collect::<Vec<_>>());
        }
        assert!(log.iter().any(|l| l.starts_with("state m.room.topic") && l.contains("\"topic\":\"New topic\"")), "{log:#?}");
        assert!(log.iter().any(|l| l.starts_with("state m.room.power_levels") && l.contains("@bob:hs") && l.contains("50")), "{:#?}", log.iter().filter(|l| l.starts_with("state")).collect::<Vec<_>>());
    }

    #[tokio::test]
    async fn a_thread_is_kept_out_of_the_main_timeline_and_has_its_own() {
        let f = fixture().await;
        let room = f.client.get_room(<&matrix_sdk::ruma::RoomId>::try_from(ROOM).unwrap()).unwrap();
        let main = Arc::new(open_timeline(&room).await.unwrap());
        let _ = main.subscribe().await;
        send_text(&main, "start of a thread", None).await.unwrap();
        let mut root = String::new();
        for _ in 0..120 {
            crate::sync_once(&f.client).await.unwrap();
            let m = ui_messages(&main.items().await.iter().cloned().collect::<Vec<_>>(), "@alice:hs");
            if m.len() == 1 && !m[0].pending { root = m[0].id.clone(); break; }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        assert!(!root.is_empty());
        let thread = Arc::new(open_thread(&room, &root).await.unwrap());
        let _ = thread.subscribe().await;
        send_text(&thread, "inside the thread", None).await.unwrap();
        let mut ok = false;
        let (mut in_main, mut in_thread) = (Vec::new(), Vec::new());
        for _ in 0..160 {
            crate::sync_once(&f.client).await.unwrap();
            in_main = ui_messages(&main.items().await.iter().cloned().collect::<Vec<_>>(), "@alice:hs");
            in_thread = ui_messages(&thread.items().await.iter().cloned().collect::<Vec<_>>(), "@alice:hs");
            if in_main.len() == 1 && in_main[0].thread_replies == 1 && in_thread.iter().any(|m| m.body == "inside the thread") { ok = true; break; }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        assert!(ok, "main: {in_main:#?}\nthread: {in_thread:#?}");
        assert_eq!(in_main[0].body, "start of a thread");
        assert!(in_thread.iter().any(|m| m.body == "start of a thread"), "the thread view starts with its root");
    }

    #[test]
    fn the_first_link_is_found_and_trimmed() {
        assert_eq!(first_url("see https://matrix.org, ok"), Some("https://matrix.org".into()));
        assert_eq!(first_url("(https://example.com/a)"), Some("https://example.com/a".into()));
        assert_eq!(first_url("https://en.wikipedia.org/wiki/Rust_(language)."), Some("https://en.wikipedia.org/wiki/Rust_(language)".into()));
        assert_eq!(first_url("a http://x.org/p?q=1 then https://y.org"), Some("http://x.org/p?q=1".into()));
        assert_eq!(first_url("no link, just https:// words"), None);
        assert_eq!(first_url("plain text"), None);
    }

    #[tokio::test]
    async fn a_link_preview_comes_from_the_homeserver() {
        let f = fixture().await;
        let dir = tempfile::tempdir().unwrap();
        let p = link_preview(&f.client, "https://example.org/page", dir.path()).await.unwrap_or_else(|| panic!("{:#?}", f.hs.log()));
        assert_eq!((p.title.as_str(), p.site.as_str()), ("Example page", "Example"), "{p:?}");
        assert!(p.description.contains("preview"), "{p:?}");
        assert!(f.hs.log().iter().any(|l| l.contains("preview_url")));
        assert!(link_preview(&f.client, "https://nothing.example/none", dir.path()).await.is_none(), "a page the server knows nothing about gives no card");
    }

    #[tokio::test]
    async fn a_message_from_someone_we_have_not_verified_carries_a_warning_and_ours_none() {
        let f = fixture().await;
        f.client.encryption().bootstrap_cross_signing(None).await.unwrap(); /* our own sessions are then vouched for by our identity */
        crate::sync_once(&f.client).await.unwrap();
        f.client.encryption().request_user_identity(f.client.user_id().unwrap()).await.unwrap();
        tokio::spawn(crate::testkit::bob_says(f.hs.clone(), vec!["from bob".to_string()]));
        send_text(&f.timeline, "from me", None).await.unwrap();
        let m = wait_for(&f, |m| m.len() == 2 && m.iter().all(|x| !x.pending)).await;
        let bob = m.iter().find(|x| x.body == "from bob").unwrap();
        let mine = m.iter().find(|x| x.body == "from me").unwrap();
        assert!(mine.shield.is_none(), "our own message needs no warning: {mine:?}");
        let s = bob.shield.as_ref().unwrap_or_else(|| panic!("bob is not verified: {bob:?}"));
        assert!(!s.text.is_empty() && (s.level == "grey" || s.level == "red"), "{s:?}");
    }

    #[tokio::test]
    async fn a_message_can_be_pinned_and_unpinned() {
        let f = fixture().await;
        send_text(&f.timeline, "keep this", None).await.unwrap();
        let id = wait_for(&f, |m| m.len() == 1 && !m[0].pending).await[0].id.clone();
        assert!(pinned_ids(&f.client, ROOM).is_empty());
        set_pinned(&f.client, ROOM, &id, true).await.unwrap();
        for _ in 0..40 { crate::sync_once(&f.client).await.unwrap(); if !pinned_ids(&f.client, ROOM).is_empty() { break; } tokio::time::sleep(std::time::Duration::from_millis(50)).await; }
        assert_eq!(pinned_ids(&f.client, ROOM), vec![id.clone()]);
        assert!(room_details(&f.client, ROOM).await.unwrap().can_pin);
        set_pinned(&f.client, ROOM, &id, false).await.unwrap();
        for _ in 0..40 { crate::sync_once(&f.client).await.unwrap(); if pinned_ids(&f.client, ROOM).is_empty() { break; } tokio::time::sleep(std::time::Duration::from_millis(50)).await; }
        assert!(pinned_ids(&f.client, ROOM).is_empty());
    }

    #[tokio::test]
    async fn a_poll_is_created_voted_on_counted_and_ended() {
        let f = fixture().await;
        assert!(create_poll(&f.timeline, "", &["a".into(), "b".into()], 1).await.is_err());
        assert!(create_poll(&f.timeline, "Lunch?", &["only one".into()], 1).await.is_err(), "needs two answers");
        create_poll(&f.timeline, "Lunch?", &["Pizza".into(), "Soup".into(), "  ".into()], 1).await.unwrap();
        let m = wait_for(&f, |m| m.len() == 1 && m[0].kind == "poll" && !m[0].pending).await;
        let id = m[0].id.clone();
        let p = m[0].poll.clone().unwrap();
        assert_eq!((p.question.as_str(), p.answers.len(), p.total_votes, p.ended), ("Lunch?", 2, 0, false), "{p:?}");
        let soup = p.answers.iter().find(|a| a.text == "Soup").unwrap().id.clone();
        vote_poll(&f.timeline, &id, vec![soup.clone()]).await.unwrap();
        let m = wait_for(&f, |m| m.iter().any(|x| x.poll.as_ref().map(|p| p.total_votes == 1).unwrap_or(false))).await;
        let p = m.iter().find_map(|x| x.poll.clone()).unwrap();
        assert!(p.answers.iter().any(|a| a.id == soup && a.votes == 1 && a.mine), "{p:?}");
        end_poll(&f.timeline, &id).await.unwrap();
        let m = wait_for(&f, |m| m.iter().any(|x| x.poll.as_ref().map(|p| p.ended).unwrap_or(false))).await;
        assert_eq!(m.iter().find_map(|x| x.poll.clone()).unwrap().total_votes, 1);
        let wire: Vec<String> = f.hs.room_events().iter().map(|e| e["type"].as_str().unwrap_or("").to_string()).collect();
        assert!(wire.iter().all(|t| t == "m.room.encrypted"), "polls travel encrypted: {wire:?}");
    }

    #[tokio::test]
    async fn people_can_be_invited_banned_listed_and_unbanned() {
        let f = fixture().await;
        invite_member(&f.client, ROOM, "@carol:hs").await.unwrap();
        assert!(invite_member(&f.client, ROOM, "carol").await.is_err());
        ban_member(&f.client, ROOM, "@bob:hs", "spam").await.unwrap();
        let mut d = room_details(&f.client, ROOM).await.unwrap();
        for _ in 0..40 {
            if d.banned.iter().any(|m| m.user_id == "@bob:hs") { break; }
            crate::sync_once(&f.client).await.unwrap();
            d = room_details(&f.client, ROOM).await.unwrap();
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        assert!(d.can_invite);
        assert_eq!(d.banned.iter().map(|m| m.user_id.as_str()).collect::<Vec<_>>(), vec!["@bob:hs"], "{d:?}");
        assert!(d.members.iter().all(|m| m.user_id != "@bob:hs"), "a banned person is no longer a member");
        unban_member(&f.client, ROOM, "@bob:hs").await.unwrap();
        for _ in 0..40 {
            crate::sync_once(&f.client).await.unwrap();
            if room_details(&f.client, ROOM).await.unwrap().banned.is_empty() { break; }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        assert!(room_details(&f.client, ROOM).await.unwrap().banned.is_empty());
        let log = f.hs.log();
        assert!(log.iter().any(|l| l == "invite @carol:hs") && log.iter().any(|l| l.starts_with("moderation unban @bob:hs")), "{log:#?}");
    }

    #[tokio::test]
    async fn rooms_of_a_space_are_listed_under_its_name_and_the_space_itself_is_not_a_room() {
        let f = fixture().await;
        let plans = create_room(&f.client, "Plans", "", false).await.unwrap();
        f.hs.add_space("Rust club", &[plans.as_str()]);
        for _ in 0..3 { crate::sync_once(&f.client).await.unwrap(); }
        let service = matrix_sdk_ui::spaces::SpaceService::new(f.client.clone()).await;
        let mut rooms = Vec::new();
        for _ in 0..40 {
            rooms = ui_rooms_with_spaces(&f.client, &service).await;
            if rooms.iter().any(|r| r.id == plans && r.section == "Rust club") { break; }
            crate::sync_once(&f.client).await.unwrap();
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
        let main = rooms.iter().find(|r| r.id == plans).unwrap();
        assert_eq!(main.section, "Rust club", "{rooms:#?}");
        assert_eq!(rooms.iter().find(|r| r.id == ROOM).unwrap().section, "Direct Messages", "direct chats stay where they are");
        assert!(rooms.iter().all(|r| r.title != "Rust club"), "the space is a section, not a room: {rooms:#?}");
    }

    #[tokio::test]
    async fn media_can_be_copied_for_a_player_but_other_files_are_not() {
        let f = fixture().await;
        let dir = tempfile::tempdir().unwrap();
        let (clip, doc) = (dir.path().join("my clip?.mp4"), dir.path().join("notes.bin"));
        std::fs::write(&clip, b"not really a video").unwrap();
        std::fs::write(&doc, b"data").unwrap();
        send_file(&f.timeline, &clip, None).await.unwrap();
        send_file(&f.timeline, &doc, None).await.unwrap();
        let m = wait_for(&f, |m| m.len() == 2 && m.iter().all(|x| !x.pending)).await;
        let video = m.iter().find(|x| x.kind == "video").unwrap_or_else(|| panic!("{m:#?}"));
        let out = dir.path().join("cache");
        let path = media_copy(&f.client, &f.timeline, &video.id, &out).await.unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"not really a video");
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        assert!(name.ends_with("my_clip_.mp4") && !name.contains(' ') && !name.contains('?'), "{name}");
        let file = m.iter().find(|x| x.kind == "file").unwrap();
        assert!(media_copy(&f.client, &f.timeline, &file.id, &out).await.is_err(), "plain files are never opened by the app");
    }

    #[tokio::test]
    async fn favourites_and_low_priority_rooms_get_their_own_sections_in_order() {
        let f = fixture().await;
        let plans = create_room(&f.client, "Plans", "", false).await.unwrap();
        let lounge = create_room(&f.client, "Lounge", "", false).await.unwrap();
        for _ in 0..2 { crate::sync_once(&f.client).await.unwrap(); }
        let service = matrix_sdk_ui::spaces::SpaceService::new(f.client.clone()).await;
        set_room_tag(&f.client, &plans, "favourite").await.unwrap();
        set_room_tag(&f.client, &lounge, "low_priority").await.unwrap();
        let mut rooms = Vec::new();
        for _ in 0..40 {
            crate::sync_once(&f.client).await.unwrap();
            rooms = ui_rooms_with_spaces(&f.client, &service).await;
            if rooms.iter().any(|r| r.id == plans && r.section == "Favourites") && rooms.iter().any(|r| r.id == lounge && r.section == "Low priority") { break; }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        let order: Vec<(&str, &str)> = rooms.iter().map(|r| (r.section.as_str(), r.title.as_str())).collect();
        assert_eq!(order, vec![("Favourites", "Plans"), ("Direct Messages", "bob"), ("Low priority", "Lounge")], "{order:?}");
        set_room_tag(&f.client, &plans, "none").await.unwrap();
        for _ in 0..40 {
            crate::sync_once(&f.client).await.unwrap();
            if !ui_rooms_with_spaces(&f.client, &service).await.iter().any(|r| r.section == "Favourites") { break; }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        assert!(ui_rooms_with_spaces(&f.client, &service).await.iter().all(|r| r.section != "Favourites"));
        assert!(set_room_tag(&f.client, &plans, "bogus").await.is_err());
    }

    #[tokio::test]
    async fn a_rooms_notification_level_can_be_set_and_read_back() {
        let f = fixture().await;
        let id = ROOM.to_string();
        assert_eq!(room_notification_level(&f.client, &id).await.unwrap(), "default");
        for level in ["mute", "mentions", "all", "default"] {
            set_room_notification_level(&f.client, &id, level).await.unwrap_or_else(|e| panic!("{level}: {e}; {:#?}", f.hs.log().iter().filter(|l| l.contains("UNHANDLED")).collect::<Vec<_>>()));
            let mut got = String::new();
            for _ in 0..40 { /* the rule set is read back through the sync, like the app does */
                crate::sync_once(&f.client).await.unwrap();
                got = room_notification_level(&f.client, &id).await.unwrap();
                if got == level { break; }
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            }
            assert_eq!(got, level);
        }
        assert!(set_room_notification_level(&f.client, &id, "loud").await.is_err());
    }

    #[tokio::test]
    async fn the_rooms_of_a_space_can_be_explored_and_an_unjoined_one_joined() {
        let f = fixture().await;
        let space = f.hs.add_space("Rust club", &[ROOM, "!hidden:hs"]);
        crate::sync_once(&f.client).await.unwrap();
        let rooms = space_rooms(&f.client, &space).await.unwrap_or_else(|e| panic!("{e}; {:#?}", f.hs.log().iter().filter(|l| l.contains("UNHANDLED")).collect::<Vec<_>>()));
        let names: Vec<(&str, bool)> = rooms.iter().map(|r| (r.room_id.as_str(), r.joined)).collect();
        assert_eq!(names, vec![("!hidden:hs", false), (ROOM, true)], "unjoined rooms first: {rooms:#?}");
        assert_eq!(rooms[0].name, "hidden", "{rooms:#?}");
        assert!(space_rooms(&f.client, "not an id").await.is_err());
        assert_eq!(join_via(&f.client, "!hidden:hs", &["hs".to_string()]).await.unwrap(), "!hidden:hs");
    }

    #[tokio::test]
    async fn custom_emoji_and_stickers_are_read_from_packs_sent_as_pictures_and_shown() {
        let f = fixture().await;
        let room = f.client.get_room(<&matrix_sdk::ruma::RoomId>::try_from(ROOM).unwrap()).unwrap();
        room.send_state_event_raw("im.ponies.room_emotes", "cats", serde_json::json!({"pack": {"display_name": "Cats"}, "images": {"cat": {"url": "mxc://hs/cat"}, "wave": {"url": "mxc://hs/wave", "usage": ["sticker"], "info": {"w": 64, "h": 48}}}})).await.unwrap();
        f.client.account().set_account_data_raw("im.ponies.user_emotes".into(), matrix_sdk::ruma::serde::Raw::new(&serde_json::json!({"images": {"mine": {"url": "mxc://hs/mine"}}})).unwrap().cast_unchecked()).await.unwrap();
        let mut packs = Vec::new();
        for _ in 0..40 { crate::sync_once(&f.client).await.unwrap(); packs = crate::emotes::emote_packs(&f.client, ROOM).await; if packs.len() == 2 { break; } tokio::time::sleep(std::time::Duration::from_millis(50)).await; }
        let names: Vec<(&str, &str, usize)> = packs.iter().map(|p| (p.source.as_str(), p.name.as_str(), p.emotes.len())).collect();
        assert_eq!(names, vec![("room", "Cats", 2), ("user", "My emoji", 1)], "{packs:#?}");
        let map = crate::emotes::emoji_map(&packs);
        assert_eq!(map.get("cat").map(String::as_str), Some("mxc://hs/cat"));
        assert!(!map.contains_key("wave"), "a sticker-only picture is not an emoji");

        send_text_with_emoji(&f.timeline, "hello :cat: and **bold** :mine:", None, &map).await.unwrap();
        send_sticker(&f.timeline, "mxc://hs/wave", "wave", 64, 48, "image/png").await.unwrap();
        assert!(send_sticker(&f.timeline, "not an mxc", "x", 0, 0, "").await.is_err());
        let msgs = wait_for(&f, |m| m.len() == 2 && m.iter().all(|x| !x.pending)).await;
        assert!(msgs[0].html.contains("src=\"mxc://hs/cat\"") && msgs[0].html.contains("<strong>bold</strong>") && msgs[0].html.contains("src=\"mxc://hs/mine\""), "{}", msgs[0].html);
        assert_eq!(msgs[0].emoji.iter().map(|e| e.mxc.as_str()).collect::<Vec<_>>(), vec!["mxc://hs/cat", "mxc://hs/mine"]);
        assert_eq!(msgs[0].body, "hello :cat: and **bold** :mine:", "the plain text keeps the shortcodes");
        assert_eq!((msgs[1].kind.as_str(), msgs[1].body.as_str(), msgs[1].image_w, msgs[1].image_h), ("sticker", "wave", 64, 48), "{:#?}", msgs[1]);
    }

    #[tokio::test]
    async fn online_status_of_people_can_be_read_and_set() {
        let f = fixture().await;
        let got = presence_of(&f.client, &["@bob:hs".to_string(), "@alice:hs".to_string(), "@nobody:hs".to_string(), "garbage".to_string()]).await;
        assert_eq!(got.get("@bob:hs").map(String::as_str), Some("online"), "{got:?}");
        assert!(!got.contains_key("@nobody:hs") && !got.contains_key("garbage"), "{got:?}");
        set_own_presence(&f.client, "unavailable").await.unwrap();
        assert_eq!(presence_of(&f.client, &["@alice:hs".to_string()]).await.get("@alice:hs").map(String::as_str), Some("unavailable"));
        assert!(set_own_presence(&f.client, "busy").await.is_err());
    }

    #[tokio::test]
    async fn people_and_public_rooms_can_be_searched_and_a_room_joined_by_address() {
        let f = fixture().await;
        let users = search_users(&f.client, "bo").await.unwrap();
        assert_eq!(users.iter().map(|u| u.user_id.as_str()).collect::<Vec<_>>(), vec!["@bob:hs"], "{users:?}");
        assert!(search_users(&f.client, "b").await.unwrap().is_empty(), "one letter is too little to ask the server");
        let rooms = public_directory(&f.client, "rust", "").await.unwrap();
        assert!(rooms.iter().any(|r| r.name == "Rust talk" && r.members == 42), "{rooms:?}");
        let id = join_by_address(&f.client, "!pub1:hs").await.unwrap_or_else(|e| panic!("{e}; {:#?}", f.hs.log().iter().filter(|l| l.contains("UNHANDLED")).collect::<Vec<_>>()));
        assert_eq!(id, "!pub1:hs");
        assert!(join_by_address(&f.client, "not an address").await.is_err());
    }

    #[tokio::test]
    async fn a_message_is_forwarded_and_an_edit_has_a_history() {
        let f = fixture().await;
        let other = create_room(&f.client, "Elsewhere", "", false).await.unwrap();
        crate::sync_once(&f.client).await.unwrap();
        send_text(&f.timeline, "first version", None).await.unwrap();
        let id = wait_for(&f, |m| m.len() == 1 && !m[0].pending).await[0].id.clone();
        assert_eq!(forward_message(&f.client, &f.timeline, &id, &[other.clone()]).await.unwrap(), 1);
        assert!(forward_message(&f.client, &f.timeline, "$nope", &[other]).await.is_err());
        edit_text(&f.timeline, &id, "second version").await.unwrap();
        let mut h = Vec::new();
        for _ in 0..60 { h = edit_history(&f.timeline, &id).await.unwrap(); if h.len() >= 2 { break; } crate::sync_once(&f.client).await.unwrap(); tokio::time::sleep(std::time::Duration::from_millis(100)).await; }
        assert_eq!(h.iter().map(|r| r.text.as_str()).collect::<Vec<_>>(), vec!["first version", "second version"], "{h:?}");
    }

    #[test]
    fn text_files_are_recognised_by_type_or_name() {
        assert!(text_like("text/plain", "notes") && text_like("application/json", "x") && text_like("", "main.rs") && text_like("", "README.md") && text_like("", "run.SH"));
        assert!(!text_like("application/pdf", "a.pdf") && !text_like("image/png", "a.png") && !text_like("", "archive.zip") && !text_like("", "txt"), "a name that is only an extension is not one");
    }

    #[tokio::test]
    async fn a_text_file_can_be_read_in_the_app_but_binary_data_and_huge_text_are_handled() {
        let f = fixture().await;
        let dir = tempfile::tempdir().unwrap();
        let (txt, bin, big) = (dir.path().join("notes.txt"), dir.path().join("blob.bin"), dir.path().join("big.log"));
        std::fs::write(&txt, "line one\nline two \u{e4}\u{f6}\u{fc}\n").unwrap();
        std::fs::write(&bin, [0u8, 159, 146, 150, 0, 1, 2]).unwrap();
        std::fs::write(&big, "x".repeat(5000)).unwrap();
        for p in [&txt, &bin, &big] { send_file(&f.timeline, p, None).await.unwrap(); }
        let m = wait_for(&f, |m| m.len() == 3 && m.iter().all(|x| !x.pending)).await;
        let by = |n: &str| m.iter().find(|x| x.file_name == n).unwrap().clone();
        assert!(by("notes.txt").text_file && by("big.log").text_file && !by("blob.bin").text_file, "{m:#?}");
        let t = read_text_file(&f.client, &f.timeline, &by("notes.txt").id, 1000).await.unwrap();
        assert_eq!((t.text.as_str(), t.truncated, t.name.as_str()), ("line one\nline two \u{e4}\u{f6}\u{fc}\n", false, "notes.txt"));
        assert!(read_text_file(&f.client, &f.timeline, &by("blob.bin").id, 1000).await.unwrap_err().contains("binary"));
        let b = read_text_file(&f.client, &f.timeline, &by("big.log").id, 100).await.unwrap();
        assert!(b.truncated && b.text.chars().count() == 100 && b.size == 5000, "{b:?}");
        assert!(read_text_file(&f.client, &f.timeline, "$nope", 100).await.is_err());
    }
}
