use crate::models::{OverwriteKind, PermissionOverwrite, Role, Snowflake};

pub const VIEW_CHANNEL: u64 = 1 << 10;
pub const KICK_MEMBERS: u64 = 1 << 1;
pub const BAN_MEMBERS: u64 = 1 << 2;
pub const ADMINISTRATOR: u64 = 1 << 3;
pub const CREATE_INSTANT_INVITE: u64 = 1 << 0;
pub const MANAGE_CHANNELS: u64 = 1 << 4;
pub const MANAGE_GUILD: u64 = 1 << 5;
pub const MANAGE_ROLES: u64 = 1 << 28;
pub const MODERATE_MEMBERS: u64 = 1 << 40;

pub const PERMISSION_GROUPS: &[(&str, &[(u64, &str)])] = &[
    (
        "General Server",
        &[
            (VIEW_CHANNEL, "View Channels"),
            (MANAGE_CHANNELS, "Manage Channels"),
            (MANAGE_ROLES, "Manage Roles"),
            (1 << 43, "Create Expressions"),
            (1 << 30, "Manage Expressions"),
            (1 << 7, "View Audit Log"),
            (1 << 19, "View Server Insights"),
            (1 << 29, "Manage Webhooks"),
            (MANAGE_GUILD, "Manage Server"),
        ],
    ),
    (
        "Membership",
        &[
            (CREATE_INSTANT_INVITE, "Create Invite"),
            (1 << 26, "Change Nickname"),
            (1 << 27, "Manage Nicknames"),
            (KICK_MEMBERS, "Kick Members"),
            (BAN_MEMBERS, "Ban Members"),
            (MODERATE_MEMBERS, "Timeout Members"),
        ],
    ),
    (
        "Text",
        &[
            (1 << 11, "Send Messages"),
            (1 << 38, "Send Messages in Threads"),
            (1 << 35, "Create Public Threads"),
            (1 << 36, "Create Private Threads"),
            (1 << 14, "Embed Links"),
            (1 << 15, "Attach Files"),
            (1 << 6, "Add Reactions"),
            (1 << 18, "Use External Emoji"),
            (1 << 37, "Use External Stickers"),
            (1 << 17, "Mention @everyone, @here, and All Roles"),
            (1 << 13, "Manage Messages"),
            (1 << 34, "Manage Threads"),
            (1 << 16, "Read Message History"),
            (1 << 12, "Send Text-to-Speech Messages"),
            (1 << 46, "Send Voice Messages"),
            (1 << 49, "Create Polls"),
        ],
    ),
    (
        "Voice",
        &[
            (1 << 20, "Connect"),
            (1 << 21, "Speak"),
            (1 << 9, "Video"),
            (1 << 39, "Use Activities"),
            (1 << 42, "Use Soundboard"),
            (1 << 45, "Use External Sounds"),
            (1 << 25, "Use Voice Activity"),
            (1 << 8, "Priority Speaker"),
            (1 << 22, "Mute Members"),
            (1 << 23, "Deafen Members"),
            (1 << 24, "Move Members"),
        ],
    ),
    (
        "Apps and Events",
        &[
            (1 << 31, "Use Application Commands"),
            (1 << 50, "Use External Apps"),
            (1 << 44, "Create Events"),
            (1 << 33, "Manage Events"),
        ],
    ),
    ("Advanced", &[(ADMINISTRATOR, "Administrator")]),
];

pub fn compute_base_permissions(member_role_ids: &[Snowflake], everyone_role_id: &str, roles: &[Role]) -> u64 {
    let mut permissions: u64 = 0;
    if let Some(everyone) = roles.iter().find(|r| r.id == everyone_role_id) {
        permissions |= everyone.permissions;
    }
    for role in roles {
        if member_role_ids.contains(&role.id) {
            permissions |= role.permissions;
        }
    }
    permissions
}

pub fn compute_channel_permissions(
    base_permissions: u64,
    member_id: &str,
    member_role_ids: &[Snowflake],
    everyone_role_id: &str,
    overwrites: &[PermissionOverwrite],
) -> u64 {
    if base_permissions & ADMINISTRATOR != 0 {
        return u64::MAX;
    }

    let mut permissions = base_permissions;

    if let Some(everyone_overwrite) = overwrites.iter().find(|o| o.id == everyone_role_id) {
        permissions &= !everyone_overwrite.deny;
        permissions |= everyone_overwrite.allow;
    }

    let mut role_allow = 0u64;
    let mut role_deny = 0u64;
    for overwrite in overwrites {
        if overwrite.kind == OverwriteKind::Role && member_role_ids.contains(&overwrite.id) {
            role_allow |= overwrite.allow;
            role_deny |= overwrite.deny;
        }
    }
    permissions &= !role_deny;
    permissions |= role_allow;

    if let Some(member_overwrite) = overwrites.iter().find(|o| o.kind == OverwriteKind::Member && o.id == member_id) {
        permissions &= !member_overwrite.deny;
        permissions |= member_overwrite.allow;
    }

    permissions
}

pub fn has_permission(permissions: u64, bit: u64) -> bool {
    permissions & ADMINISTRATOR != 0 || permissions & bit != 0
}

#[cfg(test)]
mod tests {
    use super::PERMISSION_GROUPS;

    #[test]
    fn permission_groups_list_each_bit_once() {
        let bits: Vec<u64> = PERMISSION_GROUPS.iter().flat_map(|(_, entries)| entries.iter().map(|(bit, _)| *bit)).collect();
        let unique: std::collections::HashSet<u64> = bits.iter().copied().collect();
        assert_eq!(bits.len(), unique.len());
        assert!(bits.iter().all(|bit| bit.is_power_of_two()));
    }
}
