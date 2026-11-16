use std::collections::BTreeMap;
use std::time::{Duration, Instant};

pub(crate) const FRAME_TICKS: u64 = 960;
const START_FRAMES: usize = 3;
const START_DELAY: Duration = Duration::from_millis(60);
const STOP_AFTER_EMPTY_TICKS: u32 = 5;
const CUSHION_FRAMES: usize = 2;
const FORCED_SKIP_SURPLUS: usize = 4;
const SURPLUS_TICKS_BEFORE_SKIP: u32 = 50;
const RESYNC_FRAMES: u64 = 10;
const MAX_BUFFERED: usize = 50;

struct Packet {
    payload: Vec<u8>,
    gain: f32,
    arrived: Instant,
}

#[derive(Debug, PartialEq)]
pub(crate) enum Playout {
    Idle,
    Decode { payload: Vec<u8>, gain: f32 },
    Recover { next_payload: Vec<u8>, gain: f32 },
    Conceal { gain: f32 },
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Skip {
    No,
    IfQuiet,
    Always,
}

#[derive(Default)]
pub(crate) struct JitterBuffer {
    packets: BTreeMap<u64, Packet>,
    last_unwrapped: Option<u64>,
    next: Option<u64>,
    empty_ticks: u32,
    surplus_ticks: u32,
    gain: f32,
}

impl JitterBuffer {
    fn unwrap_timestamp(&mut self, timestamp: u32) -> u64 {
        let unwrapped = match self.last_unwrapped {
            None => (1u64 << 32) + u64::from(timestamp),
            Some(last) => {
                let delta = timestamp.wrapping_sub(last as u32) as i32;
                last.wrapping_add_signed(i64::from(delta))
            }
        };
        if self.last_unwrapped.is_none_or(|last| unwrapped > last) {
            self.last_unwrapped = Some(unwrapped);
        }
        unwrapped
    }

    pub(crate) fn push(&mut self, timestamp: u32, payload: Vec<u8>, gain: f32, now: Instant) {
        let key = self.unwrap_timestamp(timestamp);
        if self.next.is_some_and(|next| key < next) {
            return;
        }
        if self.packets.len() >= MAX_BUFFERED {
            self.packets.pop_first();
        }
        self.packets.insert(key, Packet { payload, gain, arrived: now });
    }

    pub(crate) fn reset(&mut self) {
        self.packets.clear();
        self.next = None;
        self.empty_ticks = 0;
        self.surplus_ticks = 0;
    }

    pub(crate) fn tick(&mut self, now: Instant) -> Playout {
        let next = match self.next {
            Some(next) => next,
            None => {
                let Some((&first, packet)) = self.packets.first_key_value() else {
                    return Playout::Idle;
                };
                let ready = self.packets.len() >= START_FRAMES || now.duration_since(packet.arrived) >= START_DELAY;
                if !ready {
                    return Playout::Idle;
                }
                first
            }
        };
        self.packets.retain(|&key, _| key >= next);
        self.track_surplus();

        if let Some(packet) = self.packets.remove(&next) {
            return self.play(next, packet);
        }

        let Some(&earliest) = self.packets.keys().next() else {
            self.empty_ticks += 1;
            if self.empty_ticks > STOP_AFTER_EMPTY_TICKS {
                self.reset();
                return Playout::Idle;
            }
            self.next = Some(next);
            return Playout::Conceal { gain: self.gain };
        };

        if earliest - next > RESYNC_FRAMES * FRAME_TICKS {
            let packet = self.packets.remove(&earliest).expect("earliest key is present");
            return self.play(earliest, packet);
        }

        self.empty_ticks = 0;
        let following = next + FRAME_TICKS;
        self.next = Some(following);
        match self.packets.get(&following) {
            Some(packet) => Playout::Recover { next_payload: packet.payload.clone(), gain: packet.gain },
            None => Playout::Conceal { gain: self.gain },
        }
    }

    fn play(&mut self, key: u64, packet: Packet) -> Playout {
        self.next = Some(key + FRAME_TICKS);
        self.empty_ticks = 0;
        self.gain = packet.gain;
        Playout::Decode { payload: packet.payload, gain: packet.gain }
    }

    fn buffered_ahead(&self) -> usize {
        let Some(next) = self.next else { return 0 };
        self.packets.range(next..).count()
    }

    fn track_surplus(&mut self) {
        if self.buffered_ahead() > CUSHION_FRAMES {
            self.surplus_ticks = self.surplus_ticks.saturating_add(1);
        } else {
            self.surplus_ticks = 0;
        }
    }

    pub(crate) fn skip(&self) -> Skip {
        let ahead = self.buffered_ahead();
        if ahead >= CUSHION_FRAMES + FORCED_SKIP_SURPLUS {
            Skip::Always
        } else if ahead > CUSHION_FRAMES && self.surplus_ticks >= SURPLUS_TICKS_BEFORE_SKIP {
            Skip::IfQuiet
        } else {
            Skip::No
        }
    }

    pub(crate) fn skipped(&mut self) {
        self.surplus_ticks = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TS: u32 = FRAME_TICKS as u32;

    fn payload(n: u8) -> Vec<u8> {
        vec![n]
    }

    fn decoded(playout: Playout) -> Option<u8> {
        match playout {
            Playout::Decode { payload, .. } => Some(payload[0]),
            _ => None,
        }
    }

    fn primed(start: u32, frames: u8) -> (JitterBuffer, Instant) {
        let now = Instant::now();
        let mut buffer = JitterBuffer::default();
        for n in 0..frames {
            buffer.push(start.wrapping_add(u32::from(n) * TS), payload(n), 1.0, now);
        }
        (buffer, now)
    }

    #[test]
    fn waits_for_sixty_milliseconds_before_playing() {
        let now = Instant::now();
        let mut buffer = JitterBuffer::default();
        buffer.push(0, payload(0), 1.0, now);
        buffer.push(TS, payload(1), 1.0, now);
        assert_eq!(buffer.tick(now), Playout::Idle);
        assert_eq!(decoded(buffer.tick(now + START_DELAY)), Some(0));
    }

    #[test]
    fn plays_packets_in_timestamp_order() {
        let now = Instant::now();
        let mut buffer = JitterBuffer::default();
        for n in [2u8, 0, 1] {
            buffer.push(u32::from(n) * TS, payload(n), 1.0, now);
        }
        let order: Vec<_> = (0..3).map(|_| decoded(buffer.tick(now))).collect();
        assert_eq!(order, [Some(0), Some(1), Some(2)]);
    }

    #[test]
    fn a_lost_packet_is_recovered_from_the_next_ones_fec() {
        let now = Instant::now();
        let mut buffer = JitterBuffer::default();
        for n in [0u8, 2, 3] {
            buffer.push(u32::from(n) * TS, payload(n), 1.0, now);
        }
        assert_eq!(decoded(buffer.tick(now)), Some(0));
        assert_eq!(buffer.tick(now), Playout::Recover { next_payload: payload(2), gain: 1.0 });
        assert_eq!(decoded(buffer.tick(now)), Some(2));
        assert_eq!(decoded(buffer.tick(now)), Some(3));
    }

    #[test]
    fn a_late_packet_is_concealed_then_still_played() {
        let (mut buffer, now) = primed(0, 3);
        for _ in 0..3 {
            assert!(decoded(buffer.tick(now)).is_some());
        }
        assert_eq!(buffer.tick(now), Playout::Conceal { gain: 1.0 });
        buffer.push(3 * TS, payload(3), 1.0, now);
        assert_eq!(decoded(buffer.tick(now)), Some(3));
    }

    #[test]
    fn packets_older_than_the_playhead_are_dropped() {
        let (mut buffer, now) = primed(0, 3);
        assert_eq!(decoded(buffer.tick(now)), Some(0));
        buffer.push(0, payload(9), 1.0, now);
        assert_eq!(decoded(buffer.tick(now)), Some(1));
    }

    #[test]
    fn a_silent_speaker_drops_out_after_a_hundred_milliseconds() {
        let (mut buffer, now) = primed(0, 3);
        for _ in 0..3 {
            buffer.tick(now);
        }
        for _ in 0..STOP_AFTER_EMPTY_TICKS {
            assert!(matches!(buffer.tick(now), Playout::Conceal { .. }));
        }
        assert_eq!(buffer.tick(now), Playout::Idle);
    }

    #[test]
    fn speech_after_a_pause_starts_from_its_own_timestamp() {
        let (mut buffer, now) = primed(0, 3);
        for _ in 0..(3 + STOP_AFTER_EMPTY_TICKS + 1) {
            buffer.tick(now);
        }
        for n in 0..3u8 {
            buffer.push(500 * TS + u32::from(n) * TS, payload(10 + n), 1.0, now);
        }
        assert_eq!(decoded(buffer.tick(now)), Some(10));
    }

    #[test]
    fn a_large_timestamp_jump_resyncs_instead_of_concealing_for_ages() {
        let (mut buffer, now) = primed(0, 3);
        for _ in 0..3 {
            buffer.tick(now);
        }
        buffer.push(1000 * TS, payload(7), 1.0, now);
        assert_eq!(decoded(buffer.tick(now)), Some(7));
    }

    #[test]
    fn timestamps_wrap_around_without_reordering() {
        let (mut buffer, now) = primed(u32::MAX - TS + 1, 3);
        let order: Vec<_> = (0..3).map(|_| decoded(buffer.tick(now))).collect();
        assert_eq!(order, [Some(0), Some(1), Some(2)]);
    }

    #[test]
    fn a_sustained_surplus_asks_to_skip_a_quiet_frame() {
        let (mut buffer, now) = primed(0, 60);
        assert_ne!(buffer.tick(now), Playout::Idle);
        assert_eq!(buffer.skip(), Skip::Always);
        let (mut buffer, now) = primed(0, 4);
        buffer.tick(now);
        assert_eq!(buffer.skip(), Skip::No);
        for n in 4..200u32 {
            buffer.push(n * TS, payload(0), 1.0, now);
            buffer.tick(now);
            if buffer.skip() == Skip::IfQuiet {
                buffer.skipped();
                assert_eq!(buffer.skip(), Skip::No);
                return;
            }
        }
        panic!("a buffer three frames deep for a second should ask to skip");
    }
}
