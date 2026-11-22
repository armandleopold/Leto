//! The packets of the microphone, when it is asked for on Leto's API.
//!
//! A host that speaks the microphone protocol of the Apollo fork is sent
//! the microphone inside the stream, by the protocol library. A Leto host
//! beside an Apollo that has no such protocol takes it here instead: Opus
//! frames of twenty milliseconds, one channel at 48 kHz, each in a packet
//! of its own, sealed with AES-128-GCM under a key the host made for this
//! microphone and told over the API. A packet is a header, the sealed frame,
//! and its seal:
//!
//! | Offset | Size | Field |
//! |---|---|---|
//! | 0 | 1 | flags, 0 |
//! | 1 | 1 | packet type, `0x61` |
//! | 2 | 2 | number of the packet, which wraps |
//! | 4 | 4 | when the frame was captured, on a clock of 48 kHz |
//! | 8 | 4 | number of the frame, from 0, which never wraps |
//! | 12 | n | the frame, sealed |
//! | 12 + n | 16 | the seal |
//!
//! Numbers are little endian. The start of the header is the one of the
//! fork's packets; the frame number stands where they have a constant. The
//! nonce of a seal is the number of the key and of the frame, then four
//! zero bytes: no two packets of a microphone share one. The header is
//! sealed with the frame without being hidden.
//!
//! Nothing is sent twice. A frame that arrives after a later one is left
//! out: a voice is worth nothing late, and a packet heard twice is one
//! somebody recorded.

use aes_gcm::{
    Aes128Gcm, Nonce, Tag,
    aead::{AeadInPlace, KeyInit},
};

use crate::camera::Key;

/// What marks a packet as one of the microphone's.
pub const PACKET_TYPE: u8 = 0x61;

/// How many times a second the clock of the frames ticks.
pub const CLOCK: u32 = 48_000;

const HEADER: usize = 12;
const SEAL: usize = 16;

/// The largest frame a packet carries. Twenty milliseconds of Opus are a
/// few hundred bytes at most.
pub const MAX_FRAME: usize = 1400 - HEADER - SEAL;

fn nonce(key_id: u32, frame: u32) -> [u8; 12] {
    let mut nonce = [0u8; 12];
    nonce[0..4].copy_from_slice(&key_id.to_le_bytes());
    nonce[4..8].copy_from_slice(&frame.to_le_bytes());
    nonce
}

/// Whether what arrived on a port says of itself that it is a packet of a
/// microphone. Only its seal tells whether it is one.
pub fn is_one(packet: &[u8]) -> bool {
    packet.get(1) == Some(&PACKET_TYPE)
}

/// Makes the packets of a microphone, on the client.
pub struct Sealer {
    cipher: Aes128Gcm,
    key_id: u32,
    sequence: u16,
    /// The number of the next frame, or `None` once every number was used:
    /// after two and a half years of talking.
    next: Option<u32>,
    packet: Vec<u8>,
}

impl Sealer {
    pub fn new(key: &Key) -> Self {
        Self {
            cipher: Aes128Gcm::new(&key.bytes.into()),
            key_id: key.id,
            sequence: 0,
            next: Some(0),
            packet: Vec::with_capacity(HEADER + 512 + SEAL),
        }
    }

    /// The packet of one Opus `frame` captured at `timestamp`, in ticks of
    /// [`CLOCK`]. `None` for a frame that is empty or too large for one
    /// packet, and once every frame number was used.
    pub fn seal(&mut self, timestamp: u32, frame: &[u8]) -> Option<&[u8]> {
        if frame.is_empty() || frame.len() > MAX_FRAME {
            return None;
        }
        let number = self.next?;
        // A number used twice would be a nonce used twice.
        self.next = number.checked_add(1);

        self.packet.clear();
        self.packet.extend_from_slice(&[0, PACKET_TYPE]);
        self.packet.extend_from_slice(&self.sequence.to_le_bytes());
        self.packet.extend_from_slice(&timestamp.to_le_bytes());
        self.packet.extend_from_slice(&number.to_le_bytes());
        self.packet.extend_from_slice(frame);
        self.sequence = self.sequence.wrapping_add(1);

        let (head, body) = self.packet.split_at_mut(HEADER);
        let nonce = nonce(self.key_id, number);
        let seal =
            self.cipher.encrypt_in_place_detached(Nonce::from_slice(&nonce), head, body).ok()?;
        self.packet.extend_from_slice(&seal);
        Some(&self.packet)
    }
}

/// One moment of a microphone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    /// Counts up by one per frame the microphone sent.
    pub number: u32,
    /// When it was captured, in ticks of [`CLOCK`].
    pub timestamp: u32,
    /// One Opus frame.
    pub data: Vec<u8>,
}

/// Opens the packets of a microphone, on the host.
pub struct Opener {
    cipher: Aes128Gcm,
    key_id: u32,
    /// The newest frame that was handed over.
    newest: Option<u32>,
}

impl Opener {
    pub fn new(key: &Key) -> Self {
        Self { cipher: Aes128Gcm::new(&key.bytes.into()), key_id: key.id, newest: None }
    }

    /// The frame in one packet as it arrived. `None` for what is not a
    /// packet of this microphone, was changed on the way, or comes after a
    /// later frame.
    pub fn open(&mut self, packet: &[u8]) -> Option<Frame> {
        let (head, rest) = packet.split_at_checked(HEADER)?;
        let (sealed, seal) = rest.split_at_checked(rest.len().checked_sub(SEAL)?)?;
        if head[1] != PACKET_TYPE || sealed.is_empty() {
            return None;
        }
        let timestamp = u32::from_le_bytes([head[4], head[5], head[6], head[7]]);
        let number = u32::from_le_bytes([head[8], head[9], head[10], head[11]]);
        if self.newest.is_some_and(|newest| number <= newest) {
            return None;
        }
        let mut data = sealed.to_vec();
        let nonce = nonce(self.key_id, number);
        self.cipher
            .decrypt_in_place_detached(
                Nonce::from_slice(&nonce),
                head,
                &mut data,
                Tag::from_slice(seal),
            )
            .ok()?;
        self.newest = Some(number);
        Some(Frame { number, timestamp, data })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: Key = Key { bytes: *b"0123456789abcdef", id: 7 };

    #[test]
    fn a_frame_comes_out_as_it_went_in() {
        let (mut sealer, mut opener) = (Sealer::new(&KEY), Opener::new(&KEY));
        let packet = sealer.seal(960, b"a moment of voice").map(<[u8]>::to_vec);
        let packet = packet.expect("a frame this small is sealed");
        assert!(is_one(&packet));
        assert_eq!(packet.len(), HEADER + 17 + SEAL);
        // Nobody on the way reads the frame.
        assert!(!packet.windows(6).any(|window| window == b"moment"));
        assert_eq!(
            opener.open(&packet),
            Some(Frame { number: 0, timestamp: 960, data: b"a moment of voice".to_vec() })
        );
        let next = sealer.seal(1920, b"more").map(<[u8]>::to_vec).unwrap_or_default();
        assert_eq!(opener.open(&next).map(|frame| frame.number), Some(1));
    }

    #[test]
    fn what_was_changed_recorded_or_sealed_by_another_is_left_out() {
        let (mut sealer, mut opener) = (Sealer::new(&KEY), Opener::new(&KEY));
        let first = sealer.seal(0, b"one").map(<[u8]>::to_vec).unwrap_or_default();
        let second = sealer.seal(960, b"two").map(<[u8]>::to_vec).unwrap_or_default();

        // Changed on the way: the frame, then what the packet says of itself.
        for at in [HEADER, 4, 8] {
            let mut changed = second.clone();
            changed[at] ^= 1;
            assert_eq!(opener.open(&changed), None, "byte {at}");
        }
        assert!(opener.open(&second).is_some());
        // Heard a second time, and the one before it, too late.
        assert_eq!(opener.open(&second), None);
        assert_eq!(opener.open(&first), None);

        // Under another key, or not a packet at all.
        let other = Key { bytes: *b"fedcba9876543210", id: 7 };
        let foreign = Sealer::new(&other).seal(0, b"one").map(<[u8]>::to_vec).unwrap_or_default();
        assert_eq!(Opener::new(&KEY).open(&foreign), None);
        assert_eq!(Opener::new(&KEY).open(&[0]), None);
        assert_eq!(Opener::new(&KEY).open(&first[..HEADER + SEAL]), None);
    }

    #[test]
    fn a_frame_that_fits_no_packet_is_not_sealed() {
        let mut sealer = Sealer::new(&KEY);
        assert!(sealer.seal(0, &[]).is_none());
        assert!(sealer.seal(0, &vec![0; MAX_FRAME + 1]).is_none());
        assert_eq!(sealer.seal(0, &vec![0; MAX_FRAME]).map(<[u8]>::len), Some(1400));
    }
}
