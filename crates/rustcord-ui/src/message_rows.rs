use crate::icons::Icon;
use crate::timefmt::TimeFormat;
use chrono::{DateTime, NaiveDate, TimeZone, Utc};
use rustcord_client::models::Message;
use std::sync::Arc;

const GROUP_WINDOW_SECS: i64 = 7 * 60;

pub fn is_system(msg: &Message) -> bool {
    !matches!(msg.kind, 0 | 19 | 20 | 23)
}

pub fn is_reply(msg: &Message) -> bool {
    msg.kind == 19 || msg.referenced_message.is_some()
}

pub struct Stamp {
    pub utc: Option<DateTime<Utc>>,
    pub local_date: Option<NaiveDate>,
}

impl Stamp {
    pub fn of<Tz: TimeZone>(iso: &str, tz: &Tz) -> Self {
        let utc = DateTime::parse_from_rfc3339(iso).ok().map(|dt| dt.with_timezone(&Utc));
        let local_date = utc.map(|dt| dt.with_timezone(tz).date_naive());
        Self { utc, local_date }
    }
}

fn webhook_identity_differs(prev: &Message, msg: &Message) -> bool {
    msg.webhook_id.is_some() && (prev.author.username != msg.author.username || prev.author.avatar != msg.author.avatar)
}

pub fn starts_group(prev: Option<(&Message, &Stamp)>, msg: &Message, stamp: &Stamp) -> bool {
    let Some((prev, prev_stamp)) = prev else { return true };
    if prev.author.id != msg.author.id || is_system(prev) || is_system(msg) || is_reply(msg) {
        return true;
    }
    let within_window = match (prev_stamp.utc, stamp.utc) {
        (Some(a), Some(b)) => (b - a).num_seconds().abs() <= GROUP_WINDOW_SECS,
        _ => false,
    };
    if !within_window {
        return true;
    }
    if prev_stamp.local_date.is_none() || prev_stamp.local_date != stamp.local_date {
        return true;
    }
    prev.webhook_id.is_some() || webhook_identity_differs(prev, msg)
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tone {
    Muted,
    CallActive,
    CallMissed,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum SystemAction {
    JumpTo { channel_id: String, message_id: String },
    JoinCall,
}

pub struct SystemRow {
    pub icon: Icon,
    pub tone: Tone,
    pub parts: Vec<(String, bool)>,
    pub action: Option<SystemAction>,
    pub tooltip: Option<String>,
}

impl SystemRow {
    pub fn plain_text(&self) -> String {
        self.parts.iter().map(|(text, _)| text.as_str()).collect()
    }
}

fn display_name(user: &rustcord_client::models::User) -> String {
    user.global_name.clone().filter(|n| !n.is_empty()).unwrap_or_else(|| user.username.clone())
}

pub fn humanize_duration(secs: i64) -> String {
    let secs = secs.max(0);
    let minutes = (secs as f64 / 60.0).round() as i64;
    let hours = (minutes as f64 / 60.0).round() as i64;
    match secs {
        ..45 => "a few seconds".into(),
        45..90 => "a minute".into(),
        _ if minutes < 45 => format!("{minutes} minutes"),
        _ if minutes < 90 => "an hour".into(),
        _ => format!("{hours} hours"),
    }
}

fn call_duration(msg: &Message) -> Option<String> {
    let end = msg.call.as_ref()?.ended_timestamp.as_deref()?;
    let start = DateTime::parse_from_rfc3339(&msg.timestamp).ok()?;
    let end = DateTime::parse_from_rfc3339(end).ok()?;
    Some(humanize_duration((end - start).num_seconds()))
}

fn log_unsupported_once(kind: u8) {
    static LOGGED: std::sync::Mutex<Vec<u8>> = std::sync::Mutex::new(Vec::new());
    let Ok(mut logged) = LOGGED.lock() else { return };
    if !logged.contains(&kind) {
        logged.push(kind);
        tracing::info!("unsupported system message type {kind}");
    }
}

pub fn system_row(msg: &Message, me: &str) -> SystemRow {
    let author = display_name(&msg.author);
    let target = msg.mentions.first();
    let bold = |text: String| (text, true);
    let plain = |text: &str| (text.to_string(), false);
    let row = |icon, parts| SystemRow { icon, tone: Tone::Muted, parts, action: None, tooltip: None };
    match msg.kind {
        1 => {
            let target = target.map(display_name).unwrap_or_else(|| "someone".into());
            row(Icon::UserPlus, vec![bold(author), plain(" added "), bold(target), plain(" to the group.")])
        }
        2 => match target.filter(|t| t.id != msg.author.id) {
            Some(target) => row(Icon::UserMinus, vec![bold(author), plain(" removed "), bold(display_name(target)), plain(" from the group.")]),
            None => row(Icon::UserMinus, vec![bold(author), plain(" left the group.")]),
        },
        3 => {
            let ongoing = msg.call.as_ref().is_some_and(|c| c.ended_timestamp.is_none());
            let participated = msg.call.as_ref().is_some_and(|c| c.participants.iter().any(|p| p == me));
            let missed = !participated && msg.author.id != me;
            let lasted = call_duration(msg).map(|d| format!(" that lasted {d}.")).unwrap_or_else(|| ".".into());
            if ongoing {
                SystemRow {
                    icon: Icon::PhoneCall,
                    tone: Tone::CallActive,
                    parts: vec![bold(author), plain(" started a call.")],
                    action: Some(SystemAction::JoinCall),
                    tooltip: None,
                }
            } else if missed {
                SystemRow {
                    icon: Icon::PhoneMissed,
                    tone: Tone::CallMissed,
                    parts: vec![plain("You missed a call from "), bold(author), (lasted, false)],
                    action: None,
                    tooltip: None,
                }
            } else {
                SystemRow {
                    icon: Icon::Phone,
                    tone: Tone::CallActive,
                    parts: vec![bold(author), (format!(" started a call{lasted}"), false)],
                    action: None,
                    tooltip: None,
                }
            }
        }
        4 => row(Icon::Pencil, vec![bold(author), plain(" changed the channel name: "), bold(msg.content.clone()), plain(".")]),
        5 => row(Icon::Image, vec![bold(author), plain(" changed the channel icon.")]),
        6 => {
            let action = msg.message_reference.as_ref().and_then(|r| {
                Some(SystemAction::JumpTo {
                    channel_id: r.channel_id.clone().unwrap_or_else(|| msg.channel_id.clone()),
                    message_id: r.message_id.clone()?,
                })
            });
            SystemRow {
                icon: Icon::Pin,
                tone: Tone::Muted,
                parts: vec![bold(author), plain(" pinned a message to this channel.")],
                action,
                tooltip: None,
            }
        }
        7 => row(Icon::UserPlus, vec![bold(author), plain(" joined the server.")]),
        kind => {
            log_unsupported_once(kind);
            SystemRow {
                icon: Icon::CircleHelp,
                tone: Tone::Muted,
                parts: vec![bold(author), plain(" performed an unsupported action.")],
                action: None,
                tooltip: Some(format!("Unsupported message type {kind}")),
            }
        }
    }
}

pub struct Times {
    pub header: String,
    pub short: String,
    pub inline: String,
    pub full: String,
    pub edited_full: Option<String>,
}

impl Times {
    fn of<Tz: TimeZone>(msg: &Message, stamp: &Stamp, tz: &Tz, today: NaiveDate, fmt: TimeFormat) -> Self
    where
        Tz::Offset: std::fmt::Display,
    {
        let edited_full = msg
            .edited_timestamp
            .as_deref()
            .and_then(|iso| DateTime::parse_from_rfc3339(iso).ok())
            .map(|dt| fmt.full(&dt.with_timezone(tz)));
        match stamp.utc.map(|dt| dt.with_timezone(tz)) {
            Some(local) => Self {
                header: fmt.header(&local, today),
                short: fmt.time(&local),
                inline: fmt.short_datetime(&local),
                full: fmt.full(&local),
                edited_full,
            },
            None => Self {
                header: "--:--".into(),
                short: "--:--".into(),
                inline: "--:--".into(),
                full: format!("Unknown time ({})", msg.timestamp),
                edited_full,
            },
        }
    }
}

pub struct MessageRow {
    pub message: Arc<Message>,
    pub divider: Option<String>,
    pub starts_group: bool,
    pub ends_group: bool,
    pub followed_by_system: bool,
    pub times: Times,
    pub system: Option<SystemRow>,
}

pub struct ChannelRows {
    pub rows: Vec<MessageRow>,
    pub today: NaiveDate,
    pub format: TimeFormat,
}

impl ChannelRows {
    pub fn build<Tz: TimeZone>(messages: &[Arc<Message>], tz: &Tz, today: NaiveDate, format: TimeFormat, me: &str) -> Self
    where
        Tz::Offset: std::fmt::Display,
    {
        let stamps: Vec<Stamp> = messages.iter().map(|m| Stamp::of(&m.timestamp, tz)).collect();
        let mut rows: Vec<MessageRow> = Vec::with_capacity(messages.len());
        for (idx, msg) in messages.iter().enumerate() {
            let stamp = &stamps[idx];
            let prev = idx.checked_sub(1).map(|p| (&*messages[p], &stamps[p]));
            let starts = starts_group(prev, msg, stamp);
            if let Some(last) = rows.last_mut() {
                last.ends_group = starts;
                last.followed_by_system = is_system(msg) && stamp.local_date == prev.and_then(|(_, s)| s.local_date);
            }
            let prev_date = prev.and_then(|(_, s)| s.local_date);
            let divider = match stamp.local_date {
                Some(date) if prev.is_none() || prev_date != Some(date) => Some(format.long_date(date)),
                _ => None,
            };
            rows.push(MessageRow {
                message: msg.clone(),
                divider,
                starts_group: starts,
                ends_group: true,
                followed_by_system: false,
                times: Times::of(msg, stamp, tz, today, format),
                system: is_system(msg).then(|| system_row(msg, me)),
            });
        }
        Self { rows, today, format }
    }

    pub fn is_stale(&self, today: NaiveDate, format: TimeFormat) -> bool {
        self.today != today || self.format != format
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::FixedOffset;

    fn msg(id: &str, author: &str, timestamp: &str) -> Message {
        serde_json::from_value(serde_json::json!({
            "id": id,
            "channel_id": "c",
            "author": { "id": author, "username": author, "discriminator": "0" },
            "content": "hi",
            "timestamp": timestamp,
            "edited_timestamp": null,
        }))
        .unwrap()
    }

    fn with_kind(mut m: Message, kind: u8) -> Message {
        m.kind = kind;
        m
    }

    fn utc() -> FixedOffset {
        FixedOffset::east_opt(0).unwrap()
    }

    const US: TimeFormat = TimeFormat { clock_24h: false, month_first: true };

    fn today() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 11, 6).unwrap()
    }

    fn build(messages: Vec<Message>, tz: &FixedOffset) -> ChannelRows {
        let messages: Vec<Arc<Message>> = messages.into_iter().map(Arc::new).collect();
        ChannelRows::build(&messages, tz, today(), US, "me")
    }

    fn starts(messages: Vec<Message>, tz: &FixedOffset) -> Vec<bool> {
        build(messages, tz).rows.iter().map(|r| r.starts_group).collect()
    }

    fn call(author: &str, start: &str, end: Option<&str>, participants: &[&str]) -> Message {
        let mut m = with_kind(msg("1", author, start), 3);
        m.call = Some(rustcord_client::models::CallInfo {
            participants: participants.iter().map(|p| p.to_string()).collect(),
            ended_timestamp: end.map(str::to_string),
        });
        m
    }

    #[test]
    fn duration_buckets() {
        assert_eq!(humanize_duration(10), "a few seconds");
        assert_eq!(humanize_duration(44), "a few seconds");
        assert_eq!(humanize_duration(60), "a minute");
        assert_eq!(humanize_duration(120), "2 minutes");
        assert_eq!(humanize_duration(44 * 60), "44 minutes");
        assert_eq!(humanize_duration(60 * 60), "an hour");
        assert_eq!(humanize_duration(3 * 3600), "3 hours");
    }

    #[test]
    fn completed_missed_and_ongoing_calls() {
        let done = system_row(&call("me", "2026-10-28T12:13:00+00:00", Some("2026-10-28T12:14:00+00:00"), &["me"]), "me");
        assert_eq!(done.plain_text(), "me started a call that lasted a minute.");
        assert_eq!((done.icon, done.tone), (Icon::Phone, Tone::CallActive));

        let missed = system_row(&call("them", "2026-10-28T12:14:00+00:00", Some("2026-10-28T12:16:00+00:00"), &["them"]), "me");
        assert_eq!(missed.plain_text(), "You missed a call from them that lasted 2 minutes.");
        assert_eq!((missed.icon, missed.tone), (Icon::PhoneMissed, Tone::CallMissed));

        let ongoing = system_row(&call("them", "2026-10-28T12:14:00+00:00", None, &["them"]), "me");
        assert_eq!(ongoing.plain_text(), "them started a call.");
        assert_eq!(ongoing.action, Some(SystemAction::JoinCall));
    }

    #[test]
    fn recipient_changes_and_channel_edits() {
        let mut added = with_kind(msg("1", "a", "2026-11-06T10:00:00+00:00"), 1);
        added.mentions = vec![added.author.clone()];
        added.mentions[0].id = "b".into();
        added.mentions[0].username = "b".into();
        assert_eq!(system_row(&added, "me").plain_text(), "a added b to the group.");

        let mut left = with_kind(msg("1", "a", "2026-11-06T10:00:00+00:00"), 2);
        left.mentions = vec![left.author.clone()];
        assert_eq!(system_row(&left, "me").plain_text(), "a left the group.");

        let mut renamed = with_kind(msg("1", "a", "2026-11-06T10:00:00+00:00"), 4);
        renamed.content = "new name".into();
        assert_eq!(system_row(&renamed, "me").plain_text(), "a changed the channel name: new name.");
    }

    #[test]
    fn unknown_types_fall_back_to_readable_text() {
        let row = system_row(&with_kind(msg("1", "a", "2026-11-06T10:00:00+00:00"), 250), "me");
        assert_eq!(row.plain_text(), "a performed an unsupported action.");
        assert_eq!(row.tooltip.as_deref(), Some("Unsupported message type 250"));
    }

    #[test]
    fn every_system_row_has_text() {
        for kind in 1..=60u8 {
            let m = with_kind(msg("1", "a", "2026-11-06T10:00:00+00:00"), kind);
            if is_system(&m) {
                assert!(!system_row(&m, "me").plain_text().trim().is_empty(), "type {kind}");
            }
        }
    }

    #[test]
    fn consecutive_system_rows_stay_compact() {
        let rows = build(
            vec![
                with_kind(msg("1", "a", "2026-11-06T10:00:00+00:00"), 3),
                with_kind(msg("2", "a", "2026-11-06T10:01:00+00:00"), 3),
                msg("3", "a", "2026-11-06T10:02:00+00:00"),
            ],
            &utc(),
        );
        let compact: Vec<bool> = rows.rows.iter().map(|r| r.followed_by_system).collect();
        assert_eq!(compact, [true, false, false]);
    }

    #[test]
    fn one_divider_per_local_date_change() {
        let rows = build(
            vec![
                msg("1", "a", "2026-11-04T10:00:00+00:00"),
                msg("2", "b", "2026-11-04T11:00:00+00:00"),
                msg("3", "a", "2026-11-05T10:00:00+00:00"),
                msg("4", "a", "2026-11-06T10:00:00+00:00"),
                msg("5", "b", "2026-11-06T10:01:00+00:00"),
            ],
            &utc(),
        );
        let dividers: Vec<Option<&str>> = rows.rows.iter().map(|r| r.divider.as_deref()).collect();
        assert_eq!(
            dividers,
            [Some("November 4, 2026"), None, Some("November 5, 2026"), Some("November 6, 2026"), None]
        );
    }

    #[test]
    fn header_times_use_relative_days() {
        let rows = build(vec![msg("1", "a", "2026-11-05T16:29:00+00:00"), msg("2", "b", "2026-11-06T16:29:00+00:00")], &utc());
        assert_eq!(rows.rows[0].times.header, "Yesterday at 4:29 PM");
        assert_eq!(rows.rows[1].times.header, "Today at 4:29 PM");
        assert_eq!(rows.rows[1].times.full, "Friday, November 6, 2026 at 4:29 PM");
    }

    #[test]
    fn rows_go_stale_when_the_date_rolls_over() {
        let rows = build(vec![msg("1", "a", "2026-11-06T23:58:00+00:00")], &utc());
        assert!(!rows.is_stale(today(), US));
        assert!(rows.is_stale(today().succ_opt().unwrap(), US));
        assert!(rows.is_stale(today(), TimeFormat { clock_24h: true, ..US }));
    }

    #[test]
    fn an_edited_message_carries_its_edit_time() {
        let mut m = msg("1", "a", "2026-11-06T10:00:00+00:00");
        m.edited_timestamp = Some("2026-11-06T10:05:00+00:00".into());
        let rows = build(vec![m], &utc());
        assert_eq!(rows.rows[0].times.edited_full.as_deref(), Some("Friday, November 6, 2026 at 10:05 AM"));
    }

    #[test]
    fn six_minutes_apart_share_a_header() {
        let s = starts(vec![msg("1", "a", "2026-11-06T10:00:00+00:00"), msg("2", "a", "2026-11-06T10:06:00+00:00")], &utc());
        assert_eq!(s, [true, false]);
    }

    #[test]
    fn eight_minutes_apart_do_not() {
        let s = starts(vec![msg("1", "a", "2026-11-06T10:00:00+00:00"), msg("2", "a", "2026-11-06T10:08:00+00:00")], &utc());
        assert_eq!(s, [true, true]);
    }

    #[test]
    fn a_different_author_starts_a_group() {
        let s = starts(vec![msg("1", "a", "2026-11-06T10:00:00+00:00"), msg("2", "b", "2026-11-06T10:00:30+00:00")], &utc());
        assert_eq!(s, [true, true]);
    }

    #[test]
    fn a_system_message_is_always_standalone() {
        let s = starts(
            vec![
                msg("1", "a", "2026-11-06T10:00:00+00:00"),
                with_kind(msg("2", "a", "2026-11-06T10:00:10+00:00"), 3),
                msg("3", "a", "2026-11-06T10:00:20+00:00"),
            ],
            &utc(),
        );
        assert_eq!(s, [true, true, true]);
    }

    #[test]
    fn a_reply_starts_a_group() {
        let s = starts(
            vec![msg("1", "a", "2026-11-06T10:00:00+00:00"), with_kind(msg("2", "a", "2026-11-06T10:00:10+00:00"), 19)],
            &utc(),
        );
        assert_eq!(s, [true, true]);
    }

    #[test]
    fn slash_command_responses_are_not_treated_as_system_messages() {
        assert!(!is_system(&with_kind(msg("1", "a", "2026-11-06T10:00:00+00:00"), 20)));
    }

    #[test]
    fn crossing_local_midnight_splits_the_group() {
        let tz = FixedOffset::east_opt(2 * 3600).unwrap();
        let s = starts(vec![msg("1", "a", "2026-11-06T21:58:00+00:00"), msg("2", "a", "2026-11-06T22:01:00+00:00")], &tz);
        assert_eq!(s, [true, true]);
        let s = starts(vec![msg("1", "a", "2026-11-06T21:58:00+00:00"), msg("2", "a", "2026-11-06T22:01:00+00:00")], &utc());
        assert_eq!(s, [true, false]);
    }

    #[test]
    fn a_webhook_message_never_continues_a_group() {
        let mut first = msg("1", "hook", "2026-11-06T10:00:00+00:00");
        first.webhook_id = Some("hook".into());
        let mut second = msg("2", "hook", "2026-11-06T10:00:10+00:00");
        second.webhook_id = Some("hook".into());
        assert_eq!(starts(vec![first, second], &utc()), [true, true]);
    }

    #[test]
    fn an_unparseable_timestamp_starts_a_group_instead_of_panicking() {
        let s = starts(vec![msg("1", "a", "2026-11-06T10:00:00+00:00"), msg("2", "a", "garbage")], &utc());
        assert_eq!(s, [true, true]);
    }

    #[test]
    fn deleting_a_groups_first_message_promotes_the_next() {
        let all = vec![
            msg("1", "a", "2026-11-06T10:00:00+00:00"),
            msg("2", "a", "2026-11-06T10:01:00+00:00"),
            msg("3", "a", "2026-11-06T10:02:00+00:00"),
        ];
        assert_eq!(starts(all.clone(), &utc()), [true, false, false]);
        assert_eq!(starts(all[1..].to_vec(), &utc()), [true, false]);
    }

    #[test]
    fn the_last_row_of_a_group_is_marked() {
        let messages: Vec<Arc<Message>> = vec![
            msg("1", "a", "2026-11-06T10:00:00+00:00"),
            msg("2", "a", "2026-11-06T10:01:00+00:00"),
            msg("3", "b", "2026-11-06T10:02:00+00:00"),
        ]
        .into_iter()
        .map(Arc::new)
        .collect();
        let ends: Vec<bool> = ChannelRows::build(&messages, &utc(), today(), US, "me").rows.iter().map(|r| r.ends_group).collect();
        assert_eq!(ends, [false, true, true]);
    }
}
