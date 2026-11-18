//! The packets of the camera.
//!
//! A picture of the camera is an H.264 access unit, with start codes. It is
//! cut into pieces that each fit one UDP packet, and every piece is sealed on
//! its own with AES-128-GCM, under a key the host made for this camera and
//! told over the API. A packet is a header, the sealed piece, and its seal:
//!
//! | Offset | Size | Field |
//! |---|---|---|
//! | 0 | 1 | flags: bit 0 says the picture is a key frame |
//! | 1 | 1 | packet type, `0x62` |
//! | 2 | 2 | number of the packet, which wraps |
//! | 4 | 4 | when the picture was taken, on a clock of 90 kHz |
//! | 8 | 4 | number of the picture, from 0, which never wraps |
//! | 12 | 2 | number of the piece in its picture, from 0 |
//! | 14 | 2 | how many pieces the picture has |
//! | 16 | n | the piece, sealed |
//! | 16 + n | 16 | the seal |
//!
//! Numbers are little endian. The nonce of a seal is the number of the key,
//! of the picture and of the piece, then two zero bytes: no two packets of a
//! camera share one, because a picture number is never used twice. The
//! header is sealed with the piece without being hidden, so nobody on the
//! way can change what a packet says about itself.
//!
//! Nothing is sent twice and nothing is repaired: a picture that lost a
//! piece is dropped, and so are the ones after it until a key frame, since
//! each builds on the one before.

use std::fmt;

use aes_gcm::{
    Aes128Gcm, Nonce, Tag,
    aead::{AeadInPlace, KeyInit},
};

/// What marks a packet as one of the camera's.
pub const PACKET_TYPE: u8 = 0x62;

/// The largest packet, header and seal included: what crosses an ordinary
/// network in one piece.
pub const MAX_PACKET: usize = 1400;

const HEADER: usize = 16;
const SEAL: usize = 16;

/// The most of a picture that one packet carries.
pub const MAX_PIECE: usize = MAX_PACKET - HEADER - SEAL;

/// The most pieces a picture is cut into, which allows key frames of more
/// than five megabytes: far beyond what a camera makes.
pub const MAX_PIECES: usize = 4096;

/// How many times a second the clock of the pictures ticks.
pub const CLOCK: u32 = 90_000;

/// The flag of a picture that decodes on its own.
const KEY_FRAME: u8 = 1;

/// Pictures that may be in pieces at once. Packets of two pictures cross on
/// the way now and then, not of more.
const IN_PIECES: usize = 4;

/// The key of one camera, from the moment a host agrees to take it until
/// the client stops sending.
#[derive(Clone, PartialEq, Eq)]
pub struct Key {
    pub bytes: [u8; 16],
    /// The number the host gave the key. It is part of every nonce.
    pub id: u32,
}

impl fmt::Debug for Key {
    /// A key is not for logs.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Key({:#010x})", self.id)
    }
}

/// What a packet says about itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Header {
    key_frame: bool,
    sequence: u16,
    timestamp: u32,
    picture: u32,
    piece: u16,
    pieces: u16,
}

impl Header {
    fn write(&self) -> [u8; HEADER] {
        let mut bytes = [0u8; HEADER];
        bytes[0] = if self.key_frame { KEY_FRAME } else { 0 };
        bytes[1] = PACKET_TYPE;
        bytes[2..4].copy_from_slice(&self.sequence.to_le_bytes());
        bytes[4..8].copy_from_slice(&self.timestamp.to_le_bytes());
        bytes[8..12].copy_from_slice(&self.picture.to_le_bytes());
        bytes[12..14].copy_from_slice(&self.piece.to_le_bytes());
        bytes[14..16].copy_from_slice(&self.pieces.to_le_bytes());
        bytes
    }

    /// Reads a header that makes sense, or nothing.
    fn read(bytes: &[u8]) -> Option<Self> {
        let bytes: &[u8; HEADER] = bytes.try_into().ok()?;
        let [flags, kind, s0, s1, t0, t1, t2, t3, n0, n1, n2, n3, p0, p1, c0, c1] = *bytes;
        let header = Self {
            key_frame: flags & KEY_FRAME != 0,
            sequence: u16::from_le_bytes([s0, s1]),
            timestamp: u32::from_le_bytes([t0, t1, t2, t3]),
            picture: u32::from_le_bytes([n0, n1, n2, n3]),
            piece: u16::from_le_bytes([p0, p1]),
            pieces: u16::from_le_bytes([c0, c1]),
        };
        let sound = kind == PACKET_TYPE
            && header.pieces != 0
            && usize::from(header.pieces) <= MAX_PIECES
            && header.piece < header.pieces;
        sound.then_some(header)
    }
}

/// The nonce of one piece: no two pieces of a camera have the same.
fn nonce(key_id: u32, picture: u32, piece: u16) -> [u8; 12] {
    let mut nonce = [0u8; 12];
    nonce[0..4].copy_from_slice(&key_id.to_le_bytes());
    nonce[4..8].copy_from_slice(&picture.to_le_bytes());
    nonce[8..10].copy_from_slice(&piece.to_le_bytes());
    nonce
}

fn cipher(key: &Key) -> Aes128Gcm {
    Aes128Gcm::new(&key.bytes.into())
}

/// Why a picture was not made into packets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refused {
    /// There was nothing in it.
    Empty,
    /// It is larger than [`MAX_PIECES`] packets carry.
    TooLarge,
    /// Every picture number was used. A new key starts again from zero.
    Exhausted,
}

impl fmt::Display for Refused {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Empty => "a picture with nothing in it cannot be sent",
            Self::TooLarge => "the picture is too large to send",
            Self::Exhausted => "this camera has sent as many pictures as one key allows",
        })
    }
}

impl std::error::Error for Refused {}

/// Makes the packets of a camera, on the client.
pub struct Packer {
    cipher: Aes128Gcm,
    key_id: u32,
    sequence: u16,
    /// The number of the next picture, or `None` once every number was used.
    next: Option<u32>,
    /// The packet being made.
    packet: Vec<u8>,
}

impl Packer {
    pub fn new(key: &Key) -> Self {
        Self {
            cipher: cipher(key),
            key_id: key.id,
            sequence: 0,
            next: Some(0),
            packet: Vec::with_capacity(MAX_PACKET),
        }
    }

    /// Cuts one `picture` into packets and hands each one to `send`, in
    /// order. `timestamp` is when the picture was taken, in ticks of
    /// [`CLOCK`], and `key_frame` whether it decodes on its own. Returns how
    /// many packets it made.
    pub fn pack(
        &mut self,
        timestamp: u32,
        key_frame: bool,
        picture: &[u8],
        mut send: impl FnMut(&[u8]),
    ) -> Result<usize, Refused> {
        if picture.is_empty() {
            return Err(Refused::Empty);
        }
        let pieces = picture.len().div_ceil(MAX_PIECE);
        if pieces > MAX_PIECES {
            return Err(Refused::TooLarge);
        }
        let number = self.next.ok_or(Refused::Exhausted)?;
        // A number used twice would be a nonce used twice.
        self.next = number.checked_add(1);

        for (index, piece) in picture.chunks(MAX_PIECE).enumerate() {
            // Both fit: there are at most `MAX_PIECES` of them.
            let header = Header {
                key_frame,
                sequence: self.sequence,
                timestamp,
                picture: number,
                piece: index as u16,
                pieces: pieces as u16,
            };
            self.sequence = self.sequence.wrapping_add(1);

            self.packet.clear();
            self.packet.extend_from_slice(&header.write());
            self.packet.extend_from_slice(piece);
            let (head, body) = self.packet.split_at_mut(HEADER);
            let nonce = nonce(self.key_id, number, header.piece);
            let seal = self
                .cipher
                .encrypt_in_place_detached(Nonce::from_slice(&nonce), head, body)
                .map_err(|_| Refused::TooLarge)?;
            self.packet.extend_from_slice(&seal);
            send(&self.packet);
        }
        Ok(pieces)
    }
}

/// One picture of a camera, back in one piece.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Picture {
    /// Counts up by one per picture the camera sent.
    pub number: u32,
    /// When it was taken, in ticks of [`CLOCK`].
    pub timestamp: u32,
    /// It decodes on its own.
    pub key_frame: bool,
    /// An H.264 access unit with start codes.
    pub data: Vec<u8>,
}

/// What an [`Assembler`] has seen so far.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Tally {
    /// Everything that reached it.
    pub packets: u64,
    /// What was not a packet of this camera: something else on the port, a
    /// packet under another key, or one that was changed on the way.
    pub refused: u64,
    /// Pictures handed over.
    pub pictures: u64,
    /// Pictures that lost a piece, or that built on one that did.
    pub lost: u64,
}

/// A picture that still misses pieces.
struct InPieces {
    number: u32,
    timestamp: u32,
    key_frame: bool,
    pieces: Vec<Option<Vec<u8>>>,
    missing: usize,
}

/// Puts the pictures of a camera back together, on the host.
pub struct Assembler {
    cipher: Aes128Gcm,
    key_id: u32,
    /// The newest picture that was finished or given up on. What is not
    /// newer comes too late, or a second time.
    newest: Option<u32>,
    /// The one picture that can follow the last one handed over. Without
    /// it, only a key frame can be shown next.
    follows: Option<u32>,
    in_pieces: Vec<InPieces>,
    tally: Tally,
}

impl Assembler {
    pub fn new(key: &Key) -> Self {
        Self {
            cipher: cipher(key),
            key_id: key.id,
            newest: None,
            follows: None,
            in_pieces: Vec::new(),
            tally: Tally::default(),
        }
    }

    pub fn tally(&self) -> Tally {
        self.tally
    }

    /// Takes one packet as it arrived, and returns the picture it completes
    /// when that picture can be shown.
    pub fn push(&mut self, packet: &[u8]) -> Option<Picture> {
        self.tally.packets += 1;
        let Some((header, piece)) = self.open(packet) else {
            self.tally.refused += 1;
            return None;
        };
        if self.newest.is_some_and(|newest| header.picture <= newest) {
            return None;
        }

        let at = match self.in_pieces.iter().position(|picture| picture.number == header.picture) {
            Some(at) => at,
            None => {
                if self.in_pieces.len() == IN_PIECES {
                    self.give_up_on_the_oldest();
                }
                self.in_pieces.push(InPieces {
                    number: header.picture,
                    timestamp: header.timestamp,
                    key_frame: header.key_frame,
                    pieces: vec![None; usize::from(header.pieces)],
                    missing: usize::from(header.pieces),
                });
                self.in_pieces.len() - 1
            }
        };
        let picture = &mut self.in_pieces[at];
        // Every packet of a picture says the same about the picture.
        if picture.pieces.len() != usize::from(header.pieces)
            || picture.timestamp != header.timestamp
            || picture.key_frame != header.key_frame
        {
            self.tally.refused += 1;
            return None;
        }
        let place = &mut picture.pieces[usize::from(header.piece)];
        if place.is_none() {
            *place = Some(piece);
            picture.missing -= 1;
        }
        if picture.missing > 0 {
            return None;
        }

        let finished = self.in_pieces.swap_remove(at);
        // What was still in pieces before it will never be shown.
        let before = self.in_pieces.len();
        self.in_pieces.retain(|picture| picture.number > finished.number);
        self.tally.lost += (before - self.in_pieces.len()) as u64;
        self.newest = Some(finished.number);

        if !finished.key_frame && self.follows != Some(finished.number) {
            // It builds on a picture that never came.
            self.follows = None;
            self.tally.lost += 1;
            return None;
        }
        self.follows = finished.number.checked_add(1);
        self.tally.pictures += 1;
        Some(Picture {
            number: finished.number,
            timestamp: finished.timestamp,
            key_frame: finished.key_frame,
            data: finished.pieces.into_iter().flatten().flatten().collect(),
        })
    }

    /// Opens the seal of a packet: what it says about itself, and its piece.
    fn open(&self, packet: &[u8]) -> Option<(Header, Vec<u8>)> {
        if packet.len() > MAX_PACKET {
            return None;
        }
        let (head, rest) = packet.split_at_checked(HEADER)?;
        let header = Header::read(head)?;
        let (sealed, seal) = rest.split_at_checked(rest.len().checked_sub(SEAL)?)?;
        if sealed.is_empty() {
            return None;
        }
        let mut piece = sealed.to_vec();
        let nonce = nonce(self.key_id, header.picture, header.piece);
        self.cipher
            .decrypt_in_place_detached(
                Nonce::from_slice(&nonce),
                head,
                &mut piece,
                Tag::from_slice(seal),
            )
            .ok()?;
        Some((header, piece))
    }

    fn give_up_on_the_oldest(&mut self) {
        let oldest = self.in_pieces.iter().enumerate().min_by_key(|(_, picture)| picture.number);
        let Some((at, _)) = oldest else { return };
        let gone = self.in_pieces.swap_remove(at);
        // Its last pieces are of no use any more if they still come.
        self.newest = Some(self.newest.map_or(gone.number, |newest| newest.max(gone.number)));
        self.follows = None;
        self.tally.lost += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key() -> Key {
        Key { bytes: *b"sixteen byte key", id: 0x1234_5678 }
    }

    /// A picture of `length` bytes that is not the same everywhere.
    fn picture(length: usize, seed: u8) -> Vec<u8> {
        (0..length).map(|at| (at as u8).wrapping_mul(31).wrapping_add(seed)).collect()
    }

    /// The packets of one picture.
    fn packets(packer: &mut Packer, timestamp: u32, key_frame: bool, data: &[u8]) -> Vec<Vec<u8>> {
        let mut packets = Vec::new();
        let made = packer
            .pack(timestamp, key_frame, data, |packet| packets.push(packet.to_vec()))
            .expect("the picture can be sent");
        assert_eq!(made, packets.len());
        packets
    }

    #[test]
    fn a_picture_comes_out_as_it_went_in_whatever_its_size() {
        let mut packer = Packer::new(&key());
        let mut assembler = Assembler::new(&key());
        let sizes = [1, MAX_PIECE - 1, MAX_PIECE, MAX_PIECE + 1, 60_000];
        for (number, size) in sizes.into_iter().enumerate() {
            let sent = picture(size, number as u8);
            let timestamp = number as u32 * 3000;
            let packets = packets(&mut packer, timestamp, number == 0, &sent);
            assert_eq!(packets.len(), size.div_ceil(MAX_PIECE));
            assert!(packets.iter().all(|packet| packet.len() <= MAX_PACKET));

            let (last, first) = packets.split_last().expect("a picture has a packet");
            for packet in first {
                assert_eq!(assembler.push(packet), None);
            }
            let received = assembler.push(last).expect("the last piece completes the picture");
            assert_eq!(received.number, number as u32);
            assert_eq!(received.timestamp, timestamp);
            assert_eq!(received.key_frame, number == 0);
            assert_eq!(received.data, sent);
        }
        assert_eq!(assembler.tally().pictures, sizes.len() as u64);
        assert_eq!(assembler.tally().refused, 0);
        assert_eq!(assembler.tally().lost, 0);
    }

    #[test]
    fn pieces_may_arrive_in_any_order_and_twice() {
        let mut packer = Packer::new(&key());
        let mut assembler = Assembler::new(&key());
        let sent = picture(MAX_PIECE * 5 + 100, 7);
        let mut packets = packets(&mut packer, 0, true, &sent);
        packets.reverse();
        let (last, first) = packets.split_last().expect("a picture has a packet");
        for packet in first {
            assert_eq!(assembler.push(packet), None);
            assert_eq!(assembler.push(packet), None);
        }
        assert_eq!(assembler.push(last).map(|picture| picture.data), Some(sent));
    }

    #[test]
    fn nothing_of_a_picture_travels_in_clear() {
        let mut packer = Packer::new(&key());
        let sent = vec![0xABu8; 4000];
        for packet in packets(&mut packer, 0, true, &sent) {
            assert!(!packet[HEADER..].windows(8).any(|window| window == [0xAB; 8]));
        }
    }

    #[test]
    fn a_packet_that_was_changed_or_sealed_with_another_key_is_refused() {
        let mut packer = Packer::new(&key());
        let sent = picture(3000, 1);
        let packets = packets(&mut packer, 90_000, true, &sent);

        // One bit, anywhere: in what the packet says about itself, in the
        // piece or in the seal.
        for at in [0, 3, 5, 9, 12, 14, HEADER, HEADER + 700, MAX_PACKET - 1] {
            let mut assembler = Assembler::new(&key());
            let mut changed = packets[0].clone();
            changed[at] ^= 1;
            assert_eq!(assembler.push(&changed), None);
            assert_eq!(assembler.tally().refused, 1, "byte {at}");
        }

        for other in [Key { bytes: *b"another key 0123", id: key().id }, Key { id: 1, ..key() }] {
            let mut assembler = Assembler::new(&other);
            for packet in &packets {
                assert_eq!(assembler.push(packet), None);
            }
            assert_eq!(assembler.tally().refused, packets.len() as u64);
        }

        // And what is no packet of a camera at all.
        let mut assembler = Assembler::new(&key());
        let too_long = vec![0x62u8; 2000];
        let strays: [&[u8]; 5] = [b"", b"hello", &[0u8; HEADER], &[0u8; 64], &too_long];
        for stray in strays {
            assert_eq!(assembler.push(stray), None);
        }
        assert_eq!(assembler.tally().refused, 5);
    }

    #[test]
    fn a_picture_that_lost_a_piece_is_dropped_with_those_that_build_on_it() {
        let mut packer = Packer::new(&key());
        let mut assembler = Assembler::new(&key());
        let mut shown = Vec::new();
        // Pictures 0 and 6 are key frames. Picture 2 loses a piece.
        for number in 0..8u32 {
            let key_frame = number % 6 == 0;
            let sent = picture(MAX_PIECE * 2 + 10, number as u8);
            for (piece, packet) in
                packets(&mut packer, number * 3000, key_frame, &sent).iter().enumerate()
            {
                if number == 2 && piece == 1 {
                    continue;
                }
                shown.extend(assembler.push(packet).map(|picture| picture.number));
            }
        }
        assert_eq!(shown, [0, 1, 6, 7]);
        // The one in pieces, and the three that came whole after it.
        assert_eq!(assembler.tally().lost, 4);
        assert_eq!(assembler.tally().pictures, 4);
    }

    #[test]
    fn what_comes_too_late_or_a_second_time_is_not_shown_again() {
        let mut packer = Packer::new(&key());
        let mut assembler = Assembler::new(&key());
        let first = packets(&mut packer, 0, true, &picture(100, 1));
        let second = packets(&mut packer, 3000, false, &picture(100, 2));

        assert!(assembler.push(&first[0]).is_some());
        assert!(assembler.push(&second[0]).is_some());
        // Someone who recorded the packets plays them again.
        assert_eq!(assembler.push(&first[0]), None);
        assert_eq!(assembler.push(&second[0]), None);
        assert_eq!(assembler.tally().pictures, 2);
        assert_eq!(assembler.tally().refused, 0);
    }

    #[test]
    fn pictures_that_never_finish_do_not_pile_up() {
        let mut packer = Packer::new(&key());
        let mut assembler = Assembler::new(&key());
        // Twenty pictures that all lose their second piece.
        for number in 0..20u32 {
            let sent = picture(MAX_PIECE + 10, number as u8);
            let packets = packets(&mut packer, number * 3000, number == 0, &sent);
            assert_eq!(assembler.push(&packets[0]), None);
            assert!(assembler.in_pieces.len() <= IN_PIECES);
        }
        assert_eq!(assembler.tally().lost, 20 - IN_PIECES as u64);

        // A key frame that arrives whole is shown all the same.
        let sent = picture(500, 99);
        let packets = packets(&mut packer, 60_000, true, &sent);
        assert_eq!(assembler.push(&packets[0]).map(|picture| picture.data), Some(sent));
        assert!(assembler.in_pieces.is_empty());
    }

    #[test]
    fn a_packer_refuses_what_it_cannot_send() {
        let mut packer = Packer::new(&key());
        let mut sent = 0;
        assert_eq!(packer.pack(0, true, &[], |_| sent += 1), Err(Refused::Empty));
        let huge = vec![0u8; MAX_PIECE * MAX_PIECES + 1];
        assert_eq!(packer.pack(0, true, &huge, |_| sent += 1), Err(Refused::TooLarge));
        assert_eq!(sent, 0);
        // Neither took a picture number.
        assert_eq!(packer.next, Some(0));

        // The last number can be used, and no number comes after it.
        packer.next = Some(u32::MAX);
        assert_eq!(packer.pack(0, false, b"last", |_| sent += 1), Ok(1));
        assert_eq!(packer.pack(0, false, b"one more", |_| sent += 1), Err(Refused::Exhausted));
        assert_eq!(sent, 1);
    }

    #[test]
    fn a_header_reads_back_and_one_that_makes_no_sense_does_not() {
        let header = Header {
            key_frame: true,
            sequence: 0xBEEF,
            timestamp: 0x0102_0304,
            picture: 0x0A0B_0C0D,
            piece: 2,
            pieces: 3,
        };
        let bytes = header.write();
        assert_eq!(bytes, [1, 0x62, 0xEF, 0xBE, 4, 3, 2, 1, 0x0D, 0x0C, 0x0B, 0x0A, 2, 0, 3, 0]);
        assert_eq!(Header::read(&bytes), Some(header));

        let with = |change: fn(&mut Header)| {
            let mut other = header;
            change(&mut other);
            Header::read(&other.write())
        };
        assert_eq!(with(|header| header.pieces = 0), None);
        assert_eq!(with(|header| header.piece = 3), None);
        assert_eq!(with(|header| header.pieces = MAX_PIECES as u16 + 1), None);
        // Another kind of packet, or one cut short.
        let mut microphone = bytes;
        microphone[1] = 0x61;
        assert_eq!(Header::read(&microphone), None);
        assert_eq!(Header::read(&bytes[..15]), None);
    }

    #[test]
    fn a_key_does_not_show_in_logs() {
        assert_eq!(format!("{:?}", key()), "Key(0x12345678)");
    }
}
