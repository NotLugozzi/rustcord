use aes::cipher::{generic_array::GenericArray, BlockEncrypt, KeyInit};
use aes::Aes128;
use ghash::universal_hash::UniversalHash;
use ghash::{GHash, Key as GHashKey};
use hmac::{Hmac, Mac};
use sha2::Sha256;
use std::collections::HashMap;
use std::time::{Duration, Instant};
use subtle::ConstantTimeEq;

const MARKER: [u8; 2] = [0xfa, 0xfa];
const TAG_LEN: usize = 8;
const MAX_CACHED_GENERATIONS: usize = 8;

fn expand_label(secret: &[u8], label: &str, len: usize) -> Option<Vec<u8>> {
    if len > 255 * 32 { return None; }
    let label = format!("MLS 1.0 {label}");
    let mut info = Vec::with_capacity(4 + label.len());
    info.extend_from_slice(&(len as u16).to_be_bytes());
    info.push(u8::try_from(label.len()).ok()?);
    info.extend_from_slice(label.as_bytes());
    info.push(0);
    let mut output = Vec::with_capacity(len);
    let mut previous = Vec::new();
    for counter in 1..=((len + 31) / 32) {
        let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(secret).ok()?;
        mac.update(&previous);
        mac.update(&info);
        mac.update(&[u8::try_from(counter).ok()?]);
        previous = mac.finalize().into_bytes().to_vec();
        let remaining = len - output.len();
        output.extend_from_slice(&previous[..remaining.min(previous.len())]);
    }
    Some(output)
}

#[derive(Clone)]
struct GenerationKey {
    secret: Vec<u8>,
    key: [u8; 16],
}

impl GenerationKey {
    fn derive(secret: Vec<u8>) -> Option<Self> {
        let bytes = expand_label(&secret, "key", 16)?;
        let mut key = [0; 16];
        key.copy_from_slice(&bytes);
        Some(Self { secret, key })
    }

    fn next(&self) -> Option<Self> {
        Self::derive(expand_label(&self.secret, "secret", 32)?)
    }
}

struct SenderRatchet {
    keys: HashMap<u64, GenerationKey>,
    highest: u64,
}

impl SenderRatchet {
    fn new(base_secret: [u8; 16]) -> Option<Self> {
        let mut keys = HashMap::new();
        keys.insert(0, GenerationKey::derive(base_secret.to_vec())?);
        Some(Self { keys, highest: 0 })
    }

    fn key(&mut self, generation: u64) -> Option<[u8; 16]> {
        if generation <= self.highest {
            return self.keys.get(&generation).map(|state| state.key);
        }
        let mut next = self.keys.get(&self.highest)?.clone();
        for index in self.highest.checked_add(1)?..=generation {
            next = next.next()?;
            self.keys.insert(index, next.clone());
            if self.keys.len() > MAX_CACHED_GENERATIONS {
                let oldest = *self.keys.keys().min()?;
                self.keys.remove(&oldest);
            }
        }
        self.highest = generation;
        self.keys.get(&generation).map(|state| state.key)
    }

    fn unwrap_generation(&self, nonce: u32) -> Option<u64> {
        let generation_byte = (nonce >> 24) as u64;
        let highest = self.highest;
        let cycle = highest / 256;
        let mut generation = cycle.checked_mul(256)?.checked_add(generation_byte)?;
        if generation.saturating_add(128) < highest {
            generation = generation.checked_add(256)?;
        } else if generation > highest.saturating_add(128) {
            generation = generation.checked_sub(256)?;
        }
        Some(generation)
    }
}

pub struct FrameCrypto {
    ratchets: HashMap<u64, SenderRatchet>,
    pending_ratchets: HashMap<u64, SenderRatchet>,
    previous_epochs: Vec<(Instant, HashMap<u64, SenderRatchet>)>,
    local_sender_id: u64,
    next_nonce: u64,
}

impl FrameCrypto {
    pub fn new(local_sender_id: u64) -> Self {
        Self { ratchets: HashMap::new(), pending_ratchets: HashMap::new(), previous_epochs: Vec::new(), local_sender_id, next_nonce: 0 }
    }

    pub fn set_sender_base_secret(&mut self, sender_id: u64, base_secret: [u8; 16]) -> bool {
        let Some(ratchet) = SenderRatchet::new(base_secret) else { return false };
        self.ratchets.insert(sender_id, ratchet);
        true
    }

    pub fn begin_epoch(&mut self) {
        self.pending_ratchets.clear();
    }

    pub fn prepare_sender_base_secret(&mut self, sender_id: u64, base_secret: [u8; 16]) -> bool {
        let Some(ratchet) = SenderRatchet::new(base_secret) else { return false };
        self.pending_ratchets.insert(sender_id, ratchet);
        true
    }

    pub fn execute_epoch(&mut self) {
        if !self.ratchets.is_empty() {
            self.previous_epochs.push((Instant::now() + Duration::from_secs(10), std::mem::take(&mut self.ratchets)));
        }
        self.ratchets = std::mem::take(&mut self.pending_ratchets);
        self.next_nonce = 0;
    }

    pub fn reset_epoch(&mut self) {
        self.begin_epoch();
        self.execute_epoch();
    }

    pub fn encrypt(&mut self, opus_frame: &[u8]) -> Option<Vec<u8>> {
        if opus_frame.is_empty() { return None; }
        let full_nonce = self.next_nonce.checked_add(1)?;
        let nonce_value = full_nonce as u32;
        let ratchet = self.ratchets.get_mut(&self.local_sender_id)?;
        let generation = full_nonce >> 24;
        let key = ratchet.key(generation)?;
        let mut nonce = [0u8; 12];
        nonce[8..].copy_from_slice(&nonce_value.to_le_bytes());
        let mut ciphertext = opus_frame.to_vec();
        let tag = encrypt_gcm(&key, &nonce, &mut ciphertext)?;
        let mut output = Vec::with_capacity(opus_frame.len() + 16);
        output.extend_from_slice(&ciphertext);
        output.extend_from_slice(&tag[..TAG_LEN]);
        put_uleb128(nonce_value, &mut output);
        let supplemental_size = TAG_LEN + output.len() - opus_frame.len() - TAG_LEN + 3;
        output.push(u8::try_from(supplemental_size).ok()?);
        output.extend_from_slice(&MARKER);
        self.next_nonce = full_nonce;
        Some(output)
    }

    pub fn decrypt(&mut self, sender_id: u64, frame: &[u8]) -> Option<Vec<u8>> {
        if frame.len() < TAG_LEN + 1 + 1 + 2 || frame.get(frame.len() - 2..)? != MARKER { return None; }
        let marker_offset = frame.len() - 2;
        let supplemental_size = *frame.get(marker_offset.checked_sub(1)?)? as usize;
        if supplemental_size < TAG_LEN + 1 + 2 || supplemental_size > marker_offset { return None; }
        let supplemental_start = marker_offset.checked_sub(supplemental_size.checked_sub(2)?)?;
        if supplemental_start < TAG_LEN { return None; }
        let tag_start = supplemental_start;
        let mut cursor = tag_start.checked_add(TAG_LEN)?;
        let (nonce_value, nonce_len) = read_uleb128(&frame[cursor..marker_offset - 1])?;
        cursor += nonce_len;
        if cursor != marker_offset - 1 { return None; }
        let ciphertext = &frame[..tag_start];
        let tag_end = tag_start.checked_add(TAG_LEN)?;
        let truncated_tag = frame.get(tag_start..tag_end)?;
        let now = Instant::now();
        self.previous_epochs.retain(|(expiry, _)| *expiry > now);
        if let Some(plaintext) = try_decrypt(self.ratchets.get_mut(&sender_id), nonce_value, ciphertext, truncated_tag) {
            return Some(plaintext);
        }
        if let Some(plaintext) = try_decrypt(self.pending_ratchets.get_mut(&sender_id), nonce_value, ciphertext, truncated_tag) {
            return Some(plaintext);
        }
        for (_, epoch) in self.previous_epochs.iter_mut().rev() {
            if let Some(plaintext) = try_decrypt(epoch.get_mut(&sender_id), nonce_value, ciphertext, truncated_tag) {
                return Some(plaintext);
            }
        }
        None
    }
}

fn try_decrypt(
    ratchet: Option<&mut SenderRatchet>,
    nonce_value: u32,
    ciphertext: &[u8],
    truncated_tag: &[u8],
) -> Option<Vec<u8>> {
    let ratchet = ratchet?;
    let generation = ratchet.unwrap_generation(nonce_value)?;
    let key = ratchet.key(generation)?;
    let mut nonce = [0u8; 12];
    nonce[8..].copy_from_slice(&nonce_value.to_le_bytes());
    let expected_tag = calculate_tag(&key, &nonce, ciphertext)?;
    if !bool::from(expected_tag[..TAG_LEN].ct_eq(truncated_tag)) { return None; }
    let mut plaintext = ciphertext.to_vec();
    apply_ctr(&key, &nonce, &mut plaintext)?;
    Some(plaintext)
}

fn encrypt_gcm(key: &[u8; 16], nonce: &[u8; 12], buffer: &mut [u8]) -> Option<[u8; 16]> {
    apply_ctr(key, nonce, buffer)?;
    calculate_tag(key, nonce, buffer)
}

fn apply_ctr(key: &[u8; 16], nonce: &[u8; 12], buffer: &mut [u8]) -> Option<()> {
    let cipher = Aes128::new_from_slice(key).ok()?;
    let mut counter = [0u8; 16];
    counter[..12].copy_from_slice(nonce);
    counter[15] = 1;
    for chunk in buffer.chunks_mut(16) {
        let value = u32::from_be_bytes(counter[12..16].try_into().ok()?).wrapping_add(1);
        counter[12..16].copy_from_slice(&value.to_be_bytes());
        let mut stream = GenericArray::clone_from_slice(&counter);
        cipher.encrypt_block(&mut stream);
        for (dst, mask) in chunk.iter_mut().zip(stream.iter()) { *dst ^= mask; }
    }
    Some(())
}

fn calculate_tag(key: &[u8; 16], nonce: &[u8; 12], ciphertext: &[u8]) -> Option<[u8; 16]> {
    let cipher = Aes128::new_from_slice(key).ok()?;
    let mut zero = GenericArray::default();
    cipher.encrypt_block(&mut zero);
    let ghash_key = GHashKey::clone_from_slice(&zero);
    let mut ghash = GHash::new(&ghash_key);
    ghash.update_padded(ciphertext);
    let mut lengths = ghash::Block::default();
    lengths[..8].copy_from_slice(&0u64.to_be_bytes());
    lengths[8..].copy_from_slice(&(u64::try_from(ciphertext.len()).ok()?.checked_mul(8)?).to_be_bytes());
    ghash.update(&[lengths]);
    let mut tag = ghash.finalize();
    let mut j0 = [0u8; 16];
    j0[..12].copy_from_slice(nonce);
    j0[15] = 1;
    let mut mask = GenericArray::clone_from_slice(&j0);
    cipher.encrypt_block(&mut mask);
    for (out, value) in tag.iter_mut().zip(mask.iter()) { *out ^= value; }
    let mut result = [0; 16];
    result.copy_from_slice(&tag);
    Some(result)
}

fn put_uleb128(mut value: u32, output: &mut Vec<u8>) {
    loop {
        let mut byte = (value & 0x7f) as u8;
        value >>= 7;
        if value != 0 { byte |= 0x80; }
        output.push(byte);
        if value == 0 { break; }
    }
}

fn read_uleb128(bytes: &[u8]) -> Option<(u32, usize)> {
    let mut value = 0u32;
    for (index, byte) in bytes.iter().copied().enumerate() {
        let shift = index.checked_mul(7)?;
        if shift >= 32 || (shift == 28 && byte & 0x70 != 0) { return None; }
        value |= ((byte & 0x7f) as u32) << shift;
        if byte & 0x80 == 0 { return Some((value, index + 1)); }
    }
    None
}
