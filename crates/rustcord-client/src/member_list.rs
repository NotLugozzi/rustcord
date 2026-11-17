use crate::models::{GuildMember, Presence};
use serde_json::{json, Value};

pub const ROWS: usize = 100;

#[derive(Debug, Clone)]
pub enum ListItem {
    Group { id: String, count: Option<u64> },
    Member(Box<ListedMember>),
    Unknown,
}

#[derive(Debug, Clone)]
pub struct ListedMember {
    pub user_id: String,
    pub member: Option<GuildMember>,
    pub presence: Option<Presence>,
}

#[derive(Debug, Clone)]
pub enum ListOp {
    Sync { start: usize, end: usize, items: Vec<ListItem> },
    Insert { index: usize, item: ListItem },
    Update { index: usize, item: ListItem },
    Delete { index: usize },
    Invalidate,
}

#[derive(Debug, Clone)]
pub struct MemberListUpdate {
    pub guild_id: String,
    pub list_id: String,
    pub member_count: Option<u64>,
    pub online_count: Option<u64>,
    pub groups: Vec<(String, u64)>,
    pub ops: Vec<ListOp>,
}

fn parse_item(item: &Value) -> ListItem {
    if let Some(id) = item["group"]["id"].as_str() {
        return ListItem::Group { id: id.to_string(), count: item["group"]["count"].as_u64() };
    }
    let member = &item["member"];
    let Some(user_id) = member["user"]["id"].as_str() else { return ListItem::Unknown };
    let mut presence = member["presence"].clone();
    if let Some(obj) = presence.as_object_mut() {
        obj.entry("user").or_insert_with(|| json!({ "id": user_id }));
    }
    ListItem::Member(Box::new(ListedMember {
        user_id: user_id.to_string(),
        member: serde_json::from_value(member.clone()).ok(),
        presence: serde_json::from_value(presence).ok(),
    }))
}

impl MemberListUpdate {
    pub fn parse(data: &Value) -> Option<Self> {
        let ops = data["ops"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|op| {
                let range = op["range"].as_array().and_then(|r| Some((r.first()?.as_u64()? as usize, r.get(1)?.as_u64()? as usize)));
                let index = op["index"].as_u64().map(|i| i as usize);
                match (op["op"].as_str()?, index, range) {
                    ("SYNC", _, Some((start, end))) => Some(ListOp::Sync {
                        start,
                        end,
                        items: op["items"].as_array().into_iter().flatten().map(parse_item).collect(),
                    }),
                    ("INSERT", Some(index), _) => Some(ListOp::Insert { index, item: parse_item(&op["item"]) }),
                    ("UPDATE", Some(index), _) => Some(ListOp::Update { index, item: parse_item(&op["item"]) }),
                    ("DELETE", Some(index), _) => Some(ListOp::Delete { index }),
                    ("INVALIDATE", _, _) => Some(ListOp::Invalidate),
                    _ => None,
                }
            })
            .collect();
        Some(Self {
            guild_id: data["guild_id"].as_str()?.to_string(),
            list_id: data["id"].as_str().unwrap_or("everyone").to_string(),
            member_count: data["member_count"].as_u64(),
            online_count: data["online_count"].as_u64(),
            groups: data["groups"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|g| Some((g["id"].as_str()?.to_string(), g["count"].as_u64()?)))
                .collect(),
            ops,
        })
    }

    pub fn members(&self) -> impl Iterator<Item = &ListedMember> {
        self.ops
            .iter()
            .flat_map(|op| match op {
                ListOp::Sync { items, .. } => items.iter().collect::<Vec<_>>(),
                ListOp::Insert { item, .. } | ListOp::Update { item, .. } => vec![item],
                _ => Vec::new(),
            })
            .filter_map(|item| match item {
                ListItem::Member(member) => Some(member.as_ref()),
                _ => None,
            })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Row {
    Group { id: String },
    Member { user_id: String },
    Unknown,
}

impl From<&ListItem> for Row {
    fn from(item: &ListItem) -> Self {
        match item {
            ListItem::Group { id, .. } => Row::Group { id: id.clone() },
            ListItem::Member(member) => Row::Member { user_id: member.user_id.clone() },
            ListItem::Unknown => Row::Unknown,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct MemberList {
    pub channel_id: Option<String>,
    pub list_id: Option<String>,
    pub rows: Vec<Row>,
    pub group_counts: Vec<(String, u64)>,
    pub member_count: Option<u64>,
    pub online_count: Option<u64>,
    pub synced: bool,
}

pub fn subscription_frame(guild_id: &str, channel_id: Option<&str>) -> String {
    let mut subscription = json!({ "typing": true, "threads": true, "activities": true });
    if let Some(channel_id) = channel_id {
        let mut channels = serde_json::Map::new();
        channels.insert(channel_id.to_string(), json!([[0, ROWS - 1]]));
        subscription["channels"] = Value::Object(channels);
    }
    let mut subscriptions = serde_json::Map::new();
    subscriptions.insert(guild_id.to_string(), subscription);
    json!({ "op": 37, "d": { "subscriptions": subscriptions } }).to_string()
}

impl MemberList {
    pub fn for_channel(channel_id: &str) -> Self {
        Self { channel_id: Some(channel_id.to_string()), ..Default::default() }
    }

    pub fn group_count(&self, group_id: &str) -> Option<u64> {
        self.group_counts.iter().find(|(id, _)| id == group_id).map(|(_, count)| *count)
    }

    pub fn apply(&mut self, update: &MemberListUpdate) -> bool {
        let syncs = update.ops.iter().any(|op| matches!(op, ListOp::Sync { .. }));
        match &self.list_id {
            Some(list_id) if *list_id != update.list_id => return false,
            None if !syncs => return false,
            None => self.list_id = Some(update.list_id.clone()),
            Some(_) => {}
        }
        self.synced |= syncs;
        if update.member_count.is_some() {
            self.member_count = update.member_count;
        }
        if update.online_count.is_some() {
            self.online_count = update.online_count;
        }
        if !update.groups.is_empty() {
            self.group_counts = update.groups.clone();
        }
        for op in &update.ops {
            match op {
                ListOp::Sync { start, end, items } => {
                    let rows: Vec<Row> = items.iter().map(Row::from).collect();
                    let ended = rows.len() <= end.saturating_sub(*start);
                    if self.rows.len() < *start {
                        self.rows.resize(*start, Row::Unknown);
                    }
                    let after = start + rows.len();
                    let tail = if ended || self.rows.len() <= after { Vec::new() } else { self.rows.split_off(after) };
                    self.rows.truncate(*start);
                    self.rows.extend(rows);
                    self.rows.extend(tail);
                }
                ListOp::Insert { index, item } => {
                    let index = (*index).min(self.rows.len());
                    self.rows.insert(index, Row::from(item));
                }
                ListOp::Update { index, item } => {
                    if let Some(slot) = self.rows.get_mut(*index) {
                        *slot = Row::from(item);
                    }
                }
                ListOp::Delete { index } => {
                    if *index < self.rows.len() {
                        self.rows.remove(*index);
                    }
                }
                ListOp::Invalidate => {}
            }
        }
        self.rows.truncate(ROWS);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn member(id: &str, status: &str) -> Value {
        json!({ "member": { "user": { "id": id, "username": id, "discriminator": "0" }, "roles": [], "joined_at": "2026-01-01T00:00:00+00:00", "nick": null, "presence": { "status": status, "activities": [] } } })
    }

    fn update(ops: Value) -> MemberListUpdate {
        MemberListUpdate::parse(&json!({
            "guild_id": "g", "id": "everyone", "member_count": 3, "online_count": 2,
            "groups": [ { "id": "online", "count": 2 }, { "id": "offline", "count": 1 } ],
            "ops": ops
        }))
        .unwrap()
    }

    fn ids(list: &MemberList) -> Vec<String> {
        list.rows
            .iter()
            .map(|row| match row {
                Row::Group { id } => format!("[{id}]"),
                Row::Member { user_id } => user_id.clone(),
                Row::Unknown => "?".into(),
            })
            .collect()
    }

    #[test]
    fn a_sync_fills_the_list_with_groups_and_members() {
        let mut list = MemberList::for_channel("c");
        let sync = update(json!([{ "op": "SYNC", "range": [0, 99], "items": [ { "group": { "id": "online", "count": 2 } }, member("a", "online"), member("b", "idle"), { "group": { "id": "offline", "count": 1 } }, member("c", "offline") ] }]));
        assert!(list.apply(&sync));
        assert_eq!(ids(&list), ["[online]", "a", "b", "[offline]", "c"]);
        assert_eq!(list.group_count("online"), Some(2));
        assert_eq!(list.online_count, Some(2));
        let people: Vec<_> = sync.members().map(|m| m.user_id.as_str()).collect();
        assert_eq!(people, ["a", "b", "c"]);
        assert!(sync.members().all(|m| m.member.is_some() && m.presence.is_some()));
    }

    #[test]
    fn inserts_updates_and_deletes_move_rows() {
        let mut list = MemberList::for_channel("c");
        list.apply(&update(json!([{ "op": "SYNC", "range": [0, 99], "items": [ { "group": { "id": "online" } }, member("a", "online"), member("b", "online") ] }])));
        list.apply(&update(json!([{ "op": "INSERT", "index": 1, "item": member("z", "online") }])));
        list.apply(&update(json!([{ "op": "DELETE", "index": 3 }])));
        list.apply(&update(json!([{ "op": "UPDATE", "index": 2, "item": member("y", "dnd") }])));
        assert_eq!(ids(&list), ["[online]", "z", "y"]);
    }

    #[test]
    fn updates_for_another_list_or_before_a_sync_are_ignored() {
        let mut list = MemberList::for_channel("c");
        assert!(!list.apply(&update(json!([{ "op": "INSERT", "index": 0, "item": member("a", "online") }]))));
        list.apply(&update(json!([{ "op": "SYNC", "range": [0, 99], "items": [ member("a", "online") ] }])));
        let mut other = update(json!([{ "op": "SYNC", "range": [0, 99], "items": [ member("x", "online") ] }]));
        other.list_id = "123456".into();
        assert!(!list.apply(&other));
        assert_eq!(ids(&list), ["a"]);
    }

    #[test]
    fn the_subscription_asks_for_the_channels_first_rows() {
        let frame: Value = serde_json::from_str(&subscription_frame("g", Some("c"))).unwrap();
        assert_eq!(frame["op"], 37);
        assert_eq!(frame["d"]["subscriptions"]["g"]["channels"]["c"], json!([[0, 99]]));
        let bare: Value = serde_json::from_str(&subscription_frame("g", None)).unwrap();
        assert!(bare["d"]["subscriptions"]["g"].get("channels").is_none());
    }
}
