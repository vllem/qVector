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
pub struct UiRoom { pub id: String, pub title: String, pub section: String, pub unread: u32, pub highlight: bool, pub invite: bool, pub favourite: bool, pub low_priority: bool, pub avatar_mxc: String, pub avatar_path: String }

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct UiReaction { pub key: String, pub count: u32, pub mine: bool }

/// The message a reply answers.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct UiReply { pub event_id: String, pub sender: String, pub preview: String }

/// One message of a timeline as the UI draws it.
#[derive(Debug, Clone, PartialEq, Serialize, Default)]
pub struct UiMessage {
    pub id: String,
    pub sender: String,
    pub sender_id: String,
    /// plain text (also what a UI without rich text shows)
    pub body: String,
    /// sanitized HTML of a formatted message, empty for plain text
    pub html: String,
    /// "text", "emote", "notice", "image", "file", "video", "audio", "location", "undecryptable", "other"
    pub kind: String,
    pub time: String,
    /// milliseconds since the epoch (sorting; `time` is only the clock for display)
    pub ts: u64,
    pub own: bool,
    pub edited: bool,
    pub pending: bool,
    pub reply: Option<UiReply>,
    pub reactions: Vec<UiReaction>,
    /// a picture: where the cached file is once it has been downloaded (empty before), and its size
    pub image_path: String,
    pub image_w: u32,
    pub image_h: u32,
    pub file_name: String,
    /// a file whose contents can be shown as text (by its type or name): the UI offers "Open file"
    pub text_file: bool,
    pub size: u64,
    /// display names of other people whose newest read receipt is on this message
    pub seen_by: Vec<String>,
    /// replies in the thread that starts at this message (0: no thread)
    pub thread_replies: u32,
    /// the card of the first link (filled in by the app when previews are on)
    pub preview: Option<UiPreview>,
    /// who-wrote-this warning for encrypted messages
    pub shield: Option<UiShield>,
    /// pinned in the room (set by the app from the room's state)
    pub pinned: bool,
    pub poll: Option<UiPoll>,
    /// a gallery (several pictures or files sent as one message, MSC4274): its items in order; `body` is the caption
    pub gallery: Vec<UiGalleryItem>,
    /// the sender's picture: a file the app downloaded (set by the app, empty until then)
    pub avatar_path: String,
}

/// The joined rooms, grouped the way the sidebar shows them.
pub async fn ui_rooms(client: &Client) -> Vec<UiRoom> {
    let mut out = Vec::new();
    for r in client.joined_rooms() {
        if r.is_space() { continue; } /* spaces are sections, not conversations */
        let title = r.display_name().await.map(|n| n.to_string()).unwrap_or_else(|_| r.room_id().to_string());
        let direct = r.is_direct().await.unwrap_or(false);
        /* a direct chat shows the other person's picture when it has none of its own */
        let avatar = match r.avatar_url() { Some(a) => Some(a), None if direct => r.heroes().await.into_iter().next().and_then(|h| h.avatar_url), None => None };
        out.push(UiRoom {
            id: r.room_id().to_string(),
            title,
            section: if direct { "Direct Messages" } else { "Rooms" }.into(),
            unread: r.num_unread_notifications() as u32,
            highlight: r.num_unread_mentions() > 0,
            invite: false,
            favourite: r.is_favourite(),
            low_priority: r.is_low_priority(),
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

/// What the room details panel shows.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct UiRoomDetails { pub id: String, pub name: String, pub topic: String, pub encrypted: bool, pub can_edit: bool, pub can_set_roles: bool, pub can_pin: bool, pub can_invite: bool, pub members: Vec<UiMember>, pub banned: Vec<UiMember> }

/// Name, topic, encryption and the joined members (fetched in full: the sync only brings the ones it needs) of a room.
pub async fn room_details(client: &Client, room_id: &str) -> Result<UiRoomDetails, String> {
    use matrix_sdk::RoomMemberships;
    let rid = <&matrix_sdk::ruma::RoomId>::try_from(room_id).map_err(|e| e.to_string())?;
    let room = client.get_room(rid).ok_or("unknown room")?;
    let name = room.display_name().await.map(|n| n.to_string()).unwrap_or_else(|_| room_id.to_string());
    let encrypted = room.latest_encryption_state().await.map(|s| s.is_encrypted()).unwrap_or(false);
    let me = client.user_id().ok_or("not signed in")?.to_owned();
    let levels = room.power_levels().await.ok();
    let allowed = |f: &dyn Fn(&matrix_sdk::ruma::events::room::power_levels::RoomPowerLevels) -> bool| levels.as_ref().map(f).unwrap_or(false);
    let mut members: Vec<UiMember> = room.members(RoomMemberships::JOIN).await.map_err(|e| e.to_string())?.iter().map(|m| UiMember {
        user_id: m.user_id().to_string(),
        name: m.display_name().map(String::from).unwrap_or_else(|| m.user_id().localpart().to_string()),
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
    let banned: Vec<UiMember> = room.members(RoomMemberships::BAN).await.map(|v| v.iter().map(|m| UiMember {
        user_id: m.user_id().to_string(),
        name: m.display_name().map(String::from).unwrap_or_else(|| m.user_id().localpart().to_string()),
        role: String::new(), can_kick: false, can_ban: allowed(&|l| l.user_can_ban(&me)), verified: false, avatar_mxc: m.avatar_url().map(|u| u.to_string()).unwrap_or_default(), avatar_path: String::new(),
    }).collect()).unwrap_or_default();
    Ok(UiRoomDetails { id: room_id.into(), name, topic: room.topic().unwrap_or_default(), encrypted, can_edit, can_set_roles, can_pin, can_invite, members, banned })
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

/// Join a room by id or address (#room:server).
pub async fn join_by_address(client: &Client, address: &str) -> Result<String, String> {
    let id = <&matrix_sdk::ruma::RoomOrAliasId>::try_from(address.trim()).map_err(|e| format!("not a room id or address: {e}"))?;
    client.join_room_by_id_or_alias(id, &[]).await.map(|r| r.room_id().to_string()).map_err(|e| e.to_string())
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
        if let Some(parent) = spaces.joined_parents_of_child(rid).await.into_iter().next() { r.section = parent.display_name; }
    }
    for r in rooms.iter_mut().filter(|r| !r.invite) {
        if r.favourite { r.section = "Favourites".into(); } else if r.low_priority { r.section = "Low priority".into(); }
    }
    let rank = |r: &UiRoom| match r.section.as_str() { "Invites" => 0, "Favourites" => 1, "Direct Messages" => 2, "Rooms" => 4, "Low priority" => 5, _ => 3 };
    rooms.sort_by(|a, b| (rank(a), a.section.to_lowercase(), a.title.to_lowercase()).cmp(&(rank(b), b.section.to_lowercase(), b.title.to_lowercase())));
    rooms
}

fn clock(ms: u64) -> String { let s = ms / 1000; format!("{:02}:{:02}", (s / 3600) % 24, (s / 60) % 60) }

fn name_of(profile: &TimelineDetails<matrix_sdk_ui::timeline::Profile>, id: &str) -> String {
    match profile { TimelineDetails::Ready(p) => p.display_name.clone(), _ => None }
        .unwrap_or_else(|| id.trim_start_matches('@').split(':').next().unwrap_or(id).to_string())
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
    if c.is_sticker() { return ("other".into(), "[sticker]".into(), String::new()); }
    ("other".into(), String::new(), String::new())
}

fn preview_of(c: &TimelineItemContent) -> String {
    let (_, body, _) = describe(c);
    let first = body.lines().next().unwrap_or("");
    if first.chars().count() > 120 { first.chars().take(117).collect::<String>() + "..." } else { first.to_string() }
}

fn message_row(ev: &EventTimelineItem, me: &str, images: &HashMap<String, PathBuf>) -> Option<UiMessage> {
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
            row.image_path = images.get(&id).map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
        }
    }
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

fn gallery_item(index: usize, it: &matrix_sdk::ruma::events::room::message::GalleryItemType, image: Option<&PathBuf>) -> UiGalleryItem {
    use matrix_sdk::ruma::events::room::message::GalleryItemType as G;
    let uint = |v: Option<matrix_sdk::ruma::UInt>| v.map(u64::from).unwrap_or(0);
    let mut item = UiGalleryItem { index: index as u32, name: it.body().to_string(), ..Default::default() };
    match it {
        G::Image(i) => {
            item.kind = "image".into(); item.name = i.filename().to_string();
            if let Some(info) = &i.info { item.image_w = uint(info.width) as u32; item.image_h = uint(info.height) as u32; item.size = uint(info.size); }
            item.image_path = image.map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
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
pub fn ui_messages_with(items: &[Arc<TimelineItem>], me: &str, images: &HashMap<String, PathBuf>) -> Vec<UiMessage> {
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
        if let Ok(bytes) = client.media().get_media_content(&req, true).await {
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

/// Download (and decrypt) the pictures of these items that are not cached yet into `cache`; returns what was added.
pub async fn fetch_images(client: &Client, items: &[Arc<TimelineItem>], cache: &Path, have: &HashMap<String, PathBuf>) -> HashMap<String, PathBuf> {
    let mut added = HashMap::new();
    let _ = std::fs::create_dir_all(cache);
    for item in items {
        let Some(ev) = item.as_event() else { continue };
        let Some(id) = ev.event_id().map(|e| e.to_string()) else { continue };
        let Some(m) = ev.content().as_message() else { continue };
        /* (key, source, file name) of every picture of the message; a gallery's pictures are keyed "<event id>#<index>" */
        let wanted: Vec<(String, matrix_sdk::ruma::events::room::MediaSource, String)> = match m.msgtype() {
            MessageType::Image(img) => vec![(id.clone(), img.source.clone(), img.filename().to_string())],
            MessageType::Gallery(g) => g.itemtypes.iter().enumerate().filter_map(|(n, it)| match it {
                matrix_sdk::ruma::events::room::message::GalleryItemType::Image(img) => Some((format!("{id}#{n}"), img.source.clone(), img.filename().to_string())),
                _ => None,
            }).collect(),
            _ => continue,
        };
        for (key, source, name) in wanted {
            if have.contains_key(&key) || added.contains_key(&key) { continue; }
            let req = MediaRequestParameters { source, format: MediaFormat::File };
            if let Ok(bytes) = client.media().get_media_content(&req, true).await {
                let ext = name.rsplit('.').next().filter(|e| e.len() <= 5 && !e.contains('/')).unwrap_or("img");
                let safe: String = key.chars().filter(|c| c.is_ascii_alphanumeric()).collect();
                let path = cache.join(format!("{safe}.{ext}"));
                if std::fs::write(&path, bytes).is_ok() { added.insert(key, path); }
            }
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
    let bytes = client.media().get_media_content(&MediaRequestParameters { source, format: MediaFormat::File }, true).await.map_err(|e| e.to_string())?;
    std::fs::write(dest, &bytes).map_err(|e| format!("cannot write {}: {e}", dest.display()))?;
    Ok(bytes.len() as u64)
}

/// Download a picture, video or audio message into `dir` (under a safe version of its file name) so a player can open it; returns the path.
/// Other kinds of file are refused: they are saved by the user, never opened by the app.
pub async fn media_copy(client: &Client, timeline: &Timeline, event_id: &str, dir: &Path) -> Result<PathBuf, String> {
    let items: Vec<_> = timeline.items().await.iter().cloned().collect();
    let (base, item) = match event_id.split_once('#') { Some((e, n)) => (e, n.parse::<usize>().ok()), None => (event_id, None) };
    let mut m = ui_messages(&items, "").into_iter().find(|m| m.id == base).ok_or("that message is not in the timeline")?;
    if let Some(g) = item.and_then(|n| m.gallery.get(n)) { m.kind = g.kind.clone(); m.file_name = g.name.clone(); }
    if !matches!(m.kind.as_str(), "image" | "video" | "audio") { return Err("only pictures, videos and audio are opened from here; save other files instead".into()); }
    let stem: String = m.file_name.rsplit('/').next().unwrap_or("").chars().map(|c| if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') { c } else { '_' }).collect();
    let stem = stem.trim_start_matches('.');
    let id: String = event_id.chars().filter(|c| c.is_ascii_alphanumeric()).take(12).collect::<String>() + item.map(|n| format!("n{n}")).unwrap_or_default().as_str();
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let dest = dir.join(format!("{id}-{}", if stem.is_empty() { "media" } else { stem }));
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
pub async fn send_text(timeline: &Timeline, text: &str, reply_to: Option<&str>) -> Result<(), String> {
    let content = RoomMessageEventContentWithoutRelation::new(RoomMessageEventContent::text_markdown(text).msgtype);
    match reply_to.filter(|r| !r.is_empty()) {
        Some(id) => {
            let id: OwnedEventId = id.try_into().map_err(|e| format!("bad event id: {e}"))?;
            timeline.send_reply(content, id).await.map_err(|e| e.to_string())?;
        }
        None => { timeline.send(RoomMessageEventContent::new(content.msgtype).into()).await.map_err(|e| e.to_string())?; }
    }
    Ok(())
}

/// Send a file (a picture when it looks like one) into the timeline's room; encrypted rooms get an encrypted upload.
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

/// Tell the room we have read up to its newest message (so the server's unread counts and other people's receipts follow). True if a receipt was sent.
pub async fn mark_read(timeline: &Timeline) -> Result<bool, String> {
    timeline.mark_as_read(matrix_sdk::ruma::api::client::receipt::create_receipt::v3::ReceiptType::Read).await.map_err(|e| e.to_string())
}

/// Say that we are (or are no longer) typing in a room.
pub async fn set_typing(client: &Client, room_id: &str, typing: bool) -> Result<(), String> {
    room_of(client, room_id)?.typing_notice(typing).await.map_err(|e| e.to_string())
}

/// Replace the text of one of our messages (Markdown becomes formatted text).
pub async fn edit_text(timeline: &Timeline, event_id: &str, text: &str) -> Result<(), String> {
    let id: OwnedEventId = event_id.try_into().map_err(|e| format!("bad event id: {e}"))?;
    let content = RoomMessageEventContentWithoutRelation::new(RoomMessageEventContent::text_markdown(text).msgtype);
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
        let got = fetch_images(&f.client, &items, &cache, &HashMap::new()).await;
        assert_eq!(got.len(), 1);
        assert_eq!(std::fs::read(got.values().next().unwrap()).unwrap(), PNG, "downloaded and decrypted to the original bytes");
        let again = ui_messages_with(&items, "@alice:hs", &got);
        assert!(again[0].image_path.ends_with(".png"));
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
        let got = fetch_images(&f.client, &items, &dir.path().join("cache"), &HashMap::new()).await;
        assert_eq!(got.len(), 3);
        let id = msgs[0].id.clone();
        for (n, p) in files.iter().enumerate() {
            assert_eq!(std::fs::read(&got[&format!("{id}#{n}")]).unwrap(), std::fs::read(p).unwrap(), "item {n}");
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
