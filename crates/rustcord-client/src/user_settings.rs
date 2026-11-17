use base64::engine::general_purpose::STANDARD;
use base64::Engine;

const GUILD_FOLDERS: u64 = 14;
const FOLDERS: u64 = 1;
const GUILD_POSITIONS: u64 = 2;
const FOLDER_GUILDS: u64 = 1;
const FOLDER_ID: u64 = 2;
const FOLDER_NAME: u64 = 3;
const FOLDER_COLOR: u64 = 4;

const VARINT: u64 = 0;
const FIXED64: u64 = 1;
const BYTES: u64 = 2;
const FIXED32: u64 = 5;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RailEntry {
    Guild(String),
    Folder(Folder),
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Folder {
    pub id: u64,
    pub name: Option<String>,
    pub color: Option<u32>,
    pub guild_ids: Vec<String>,
}

enum Field<'a> {
    Varint(u64),
    Fixed64(u64),
    Bytes(&'a [u8]),
    Fixed32,
}

fn read_varint(data: &[u8], at: &mut usize) -> Option<u64> {
    let mut value = 0u64;
    for shift in (0..64).step_by(7) {
        let byte = *data.get(*at)?;
        *at += 1;
        value |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Some(value);
        }
    }
    None
}

fn fields(data: &[u8]) -> impl Iterator<Item = (u64, Field<'_>)> {
    let mut at = 0usize;
    std::iter::from_fn(move || {
        if at >= data.len() {
            return None;
        }
        let key = read_varint(data, &mut at)?;
        let field = match key & 7 {
            VARINT => Field::Varint(read_varint(data, &mut at)?),
            FIXED64 => {
                let bytes: [u8; 8] = data.get(at..at + 8)?.try_into().ok()?;
                at += 8;
                Field::Fixed64(u64::from_le_bytes(bytes))
            }
            BYTES => {
                let len = read_varint(data, &mut at)? as usize;
                let bytes = data.get(at..at.checked_add(len)?)?;
                at += len;
                Field::Bytes(bytes)
            }
            FIXED32 => {
                data.get(at..at + 4)?;
                at += 4;
                Field::Fixed32
            }
            _ => return None,
        };
        Some((key >> 3, field))
    })
}

fn fixed64s(field: Field<'_>) -> Vec<u64> {
    match field {
        Field::Fixed64(value) => vec![value],
        Field::Bytes(packed) => packed.chunks_exact(8).map(|chunk| u64::from_le_bytes(chunk.try_into().expect("eight bytes"))).collect(),
        _ => Vec::new(),
    }
}

fn wrapped_varint(wrapper: &[u8]) -> u64 {
    fields(wrapper)
        .find_map(|(number, field)| match field {
            Field::Varint(value) if number == 1 => Some(value),
            _ => None,
        })
        .unwrap_or_default()
}

fn wrapped_string(wrapper: &[u8]) -> Option<String> {
    fields(wrapper).find_map(|(number, field)| match field {
        Field::Bytes(text) if number == 1 => Some(String::from_utf8_lossy(text).into_owned()),
        _ => None,
    })
}

pub fn server_rail(encoded: &str) -> Option<Vec<RailEntry>> {
    let data = STANDARD.decode(encoded.trim()).ok()?;
    let folders = fields(&data).find_map(|(number, field)| match field {
        Field::Bytes(bytes) if number == GUILD_FOLDERS => Some(bytes),
        _ => None,
    })?;
    let mut entries = Vec::new();
    let mut positions = Vec::new();
    for (number, field) in fields(folders) {
        match (number, field) {
            (FOLDERS, Field::Bytes(entry)) => {
                let mut folder = Folder::default();
                let mut has_id = false;
                for (number, field) in fields(entry) {
                    match (number, field) {
                        (FOLDER_GUILDS, field) => folder.guild_ids.extend(fixed64s(field).into_iter().map(|id| id.to_string())),
                        (FOLDER_ID, Field::Bytes(wrapper)) => {
                            folder.id = wrapped_varint(wrapper);
                            has_id = true;
                        }
                        (FOLDER_NAME, Field::Bytes(wrapper)) => folder.name = wrapped_string(wrapper).filter(|name| !name.trim().is_empty()),
                        (FOLDER_COLOR, Field::Bytes(wrapper)) => folder.color = u32::try_from(wrapped_varint(wrapper)).ok(),
                        _ => {}
                    }
                }
                match (has_id, folder.guild_ids.as_slice()) {
                    (_, []) => {}
                    (false, ids) => entries.extend(ids.iter().cloned().map(RailEntry::Guild)),
                    (true, _) => entries.push(RailEntry::Folder(folder)),
                }
            }
            (GUILD_POSITIONS, field) => positions.extend(fixed64s(field)),
            _ => {}
        }
    }
    if entries.is_empty() {
        entries = positions.into_iter().map(|id| RailEntry::Guild(id.to_string())).collect();
    }
    Some(entries)
}

fn put_varint(out: &mut Vec<u8>, mut value: u64) {
    loop {
        let byte = (value & 0x7f) as u8;
        value >>= 7;
        if value == 0 {
            out.push(byte);
            return;
        }
        out.push(byte | 0x80);
    }
}

fn put_bytes(out: &mut Vec<u8>, number: u64, bytes: &[u8]) {
    put_varint(out, number << 3 | BYTES);
    put_varint(out, bytes.len() as u64);
    out.extend_from_slice(bytes);
}

fn wrapped(value: u64) -> Vec<u8> {
    let mut wrapper = Vec::new();
    put_varint(&mut wrapper, 1 << 3 | VARINT);
    put_varint(&mut wrapper, value);
    wrapper
}

pub fn encode_server_rail(entries: &[RailEntry]) -> String {
    let mut message = Vec::new();
    for entry in entries {
        let mut folder = Vec::new();
        let (ids, details): (Vec<&String>, Option<&Folder>) = match entry {
            RailEntry::Guild(id) => (vec![id], None),
            RailEntry::Folder(folder) => (folder.guild_ids.iter().collect(), Some(folder)),
        };
        let packed: Vec<u8> = ids.iter().filter_map(|id| id.parse::<u64>().ok()).flat_map(u64::to_le_bytes).collect();
        if packed.is_empty() {
            continue;
        }
        put_bytes(&mut folder, FOLDER_GUILDS, &packed);
        if let Some(details) = details {
            put_bytes(&mut folder, FOLDER_ID, &wrapped(details.id));
            if let Some(name) = &details.name {
                let mut wrapper = Vec::new();
                put_bytes(&mut wrapper, 1, name.as_bytes());
                put_bytes(&mut folder, FOLDER_NAME, &wrapper);
            }
            if let Some(color) = details.color {
                put_bytes(&mut folder, FOLDER_COLOR, &wrapped(u64::from(color)));
            }
        }
        put_bytes(&mut message, FOLDERS, &folder);
    }
    let mut settings = Vec::new();
    put_bytes(&mut settings, GUILD_FOLDERS, &message);
    STANDARD.encode(settings)
}

pub fn new_folder_id() -> u64 {
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    ((nanos as u64) ^ (u64::from(std::process::id()) << 32)) & ((1 << 31) - 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Vec<RailEntry> {
        vec![
            RailEntry::Guild("1001".into()),
            RailEntry::Folder(Folder { id: 77, name: Some("Games".into()), color: Some(0x5865f2), guild_ids: vec!["2002".into(), "3003".into()] }),
            RailEntry::Folder(Folder { id: 78, name: None, color: None, guild_ids: vec!["4004".into()] }),
            RailEntry::Guild("5005".into()),
        ]
    }

    #[test]
    fn a_rail_round_trips_through_discords_settings() {
        let encoded = encode_server_rail(&sample());
        assert_eq!(server_rail(&encoded), Some(sample()));
    }

    #[test]
    fn settings_without_folders_leave_the_rail_alone() {
        let mut other = Vec::new();
        put_bytes(&mut other, 11, &[1 << 3 | VARINT as u8, 1]);
        assert_eq!(server_rail(&STANDARD.encode(other)), None);
        assert_eq!(server_rail("not base64!"), None);
    }

    #[test]
    fn unrelated_fields_around_the_folders_are_skipped() {
        let mut settings = Vec::new();
        put_varint(&mut settings, 1 << 3 | FIXED64);
        settings.extend_from_slice(&7u64.to_le_bytes());
        put_bytes(&mut settings, 5, b"voice");
        let folders = STANDARD.decode(encode_server_rail(&sample())).unwrap();
        settings.extend_from_slice(&folders);
        put_varint(&mut settings, 20 << 3 | FIXED32);
        settings.extend_from_slice(&[0, 0, 0, 0]);
        assert_eq!(server_rail(&STANDARD.encode(settings)), Some(sample()));
    }

    #[test]
    fn the_legacy_order_is_used_when_there_are_no_folders() {
        let mut folders = Vec::new();
        put_bytes(&mut folders, GUILD_POSITIONS, &[9u64.to_le_bytes(), 8u64.to_le_bytes()].concat());
        let mut settings = Vec::new();
        put_bytes(&mut settings, GUILD_FOLDERS, &folders);
        assert_eq!(server_rail(&STANDARD.encode(settings)), Some(vec![RailEntry::Guild("9".into()), RailEntry::Guild("8".into())]));
    }

    #[test]
    fn new_folder_ids_fit_discords_range() {
        let id = new_folder_id();
        assert!(id < (1 << 31));
    }
}
