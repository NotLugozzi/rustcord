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
    pub times: Times,
}

pub struct ChannelRows {
    pub rows: Vec<MessageRow>,
    pub today: NaiveDate,
    pub format: TimeFormat,
}

impl ChannelRows {
    pub fn build<Tz: TimeZone>(messages: &[Arc<Message>], tz: &Tz, today: NaiveDate, format: TimeFormat) -> Self
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
                times: Times::of(msg, stamp, tz, today, format),
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
        ChannelRows::build(&messages, tz, today(), US)
    }

    fn starts(messages: Vec<Message>, tz: &FixedOffset) -> Vec<bool> {
        build(messages, tz).rows.iter().map(|r| r.starts_group).collect()
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
        let ends: Vec<bool> = ChannelRows::build(&messages, &utc(), today(), US).rows.iter().map(|r| r.ends_group).collect();
        assert_eq!(ends, [false, true, true]);
    }
}
