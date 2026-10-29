use crate::models::{OverwriteKind, PermissionOverwrite, Role, Snowflake};

pub const VIEW_CHANNEL: u64 = 1 << 10;
pub const KICK_MEMBERS: u64 = 1 << 1;
pub const BAN_MEMBERS: u64 = 1 << 2;
pub const ADMINISTRATOR: u64 = 1 << 3;

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
