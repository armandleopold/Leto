//! Android's own Opus decoder, through the NDK's MediaCodec: libopus, built
//! for the phone's processor. Leto's decoder, written in Rust, takes about
//! a whole core of a Fire TV Stick's 32-bit processor and still gives only
//! half of the sound in time.
//!
//! Packets go in one at a time and their samples come out, a little later
//! at times: a packet's samples may come out with the next one's.

use std::{ffi::CString, ptr, time::Duration};

use ndk_sys as sys;

/// How long the samples of a packet are waited for, once it went in. They
/// come with the next packet's otherwise.
const OUT_PATIENCE: Duration = Duration::from_millis(2);
/// How long a free input buffer is waited for.
const IN_PATIENCE: Duration = Duration::from_millis(20);
/// How long a packet lasts, for its time: the hosts send five milliseconds.
const PACKET_MICROS: u64 = 5_000;
/// What the decoder needs to decode from the middle of a stream, in
/// nanoseconds: the 80 ms the Opus specification gives.
const SEEK_PREROLL: u64 = 80_000_000;

/// `AudioFormat.ENCODING_PCM_16BIT`.
const PCM_16: i32 = 2;
/// `AudioFormat.ENCODING_PCM_FLOAT`.
const PCM_FLOAT: i32 = 4;

/// One Opus decoder of the phone's, for one stream of plain Opus.
pub struct OpusDecoder {
    codec: *mut sys::AMediaCodec,
    /// Counts the packets that went in, for their times.
    count: u64,
    /// How the samples come out: `PCM_16` or `PCM_FLOAT`.
    encoding: i32,
    /// The samples of what came out, interleaved, between -1 and 1.
    samples: Vec<f32>,
}

// SAFETY: a MediaCodec may be used from any thread, one at a time, which
// `&mut self` ensures.
unsafe impl Send for OpusDecoder {}

impl OpusDecoder {
    /// A decoder for Opus at `sample_rate` with `channels` channels (one or
    /// two), as one stream without a channel mapping.
    pub fn new(sample_rate: u32, channels: u8) -> Result<Self, String> {
        let mime = c"audio/opus";
        // SAFETY: a C string.
        let codec = unsafe { sys::AMediaCodec_createDecoderByType(mime.as_ptr()) };
        if codec.is_null() {
            return Err("this device has no Opus decoder".into());
        }
        let mut decoder = Self { codec, count: 0, encoding: PCM_16, samples: Vec::new() };

        // SAFETY: a new format, deleted below.
        let format = unsafe { sys::AMediaFormat_new() };
        let int = |key: &str, value: i32| {
            let key = CString::new(key).unwrap_or_default();
            // SAFETY: a live format and a C string.
            unsafe { sys::AMediaFormat_setInt32(format, key.as_ptr(), value) };
        };
        let buffer = |key: &str, value: &[u8]| {
            let key = CString::new(key).unwrap_or_default();
            // SAFETY: a live format, a C string and `value.len()` bytes,
            // which the format copies.
            unsafe {
                sys::AMediaFormat_setBuffer(
                    format,
                    key.as_ptr(),
                    value.as_ptr().cast(),
                    value.len(),
                )
            };
        };
        // SAFETY: a live format and C strings.
        unsafe { sys::AMediaFormat_setString(format, c"mime".as_ptr(), mime.as_ptr()) };
        int("sample-rate", i32::try_from(sample_rate).unwrap_or(48_000));
        int("channel-count", i32::from(channels));
        // Asked for; a decoder that cannot gives 16 bits, and says so.
        int("pcm-encoding", PCM_FLOAT);
        // The identification header of an Ogg Opus stream (RFC 7845), as
        // Android wants it, and nothing to skip at the start: the hosts'
        // packets carry no Ogg pages, and the first sound is wanted too.
        buffer("csd-0", &head(sample_rate, channels));
        buffer("csd-1", &0_u64.to_le_bytes());
        buffer("csd-2", &SEEK_PREROLL.to_le_bytes());

        // SAFETY: a live codec and format, no surface, no crypto.
        let configured = unsafe {
            sys::AMediaCodec_configure(codec, format, ptr::null_mut(), ptr::null_mut(), 0)
        };
        // SAFETY: deleted once, after use.
        unsafe { sys::AMediaFormat_delete(format) };
        status(configured, "setting up the Opus decoder")?;
        // SAFETY: a configured codec.
        status(unsafe { sys::AMediaCodec_start(codec) }, "starting the Opus decoder")?;
        decoder.read_format();
        Ok(decoder)
    }

    /// Decodes one packet. The samples that came out, of this packet or of
    /// earlier ones, are interleaved and between -1 and 1: none, at times.
    pub fn decode(&mut self, packet: &[u8]) -> Result<&[f32], String> {
        self.samples.clear();
        self.put(packet)?;
        self.take()?;
        Ok(&self.samples)
    }

    fn put(&mut self, packet: &[u8]) -> Result<(), String> {
        // SAFETY: a started codec.
        let index = unsafe { sys::AMediaCodec_dequeueInputBuffer(self.codec, micros(IN_PATIENCE)) };
        let Ok(index) = usize::try_from(index) else {
            return Err("the Opus decoder takes no more packets".into());
        };
        let mut size = 0;
        // SAFETY: an index the codec gave; `size` is written.
        let buffer = unsafe { sys::AMediaCodec_getInputBuffer(self.codec, index, &mut size) };
        if buffer.is_null() || size < packet.len() {
            // SAFETY: the buffer goes back empty.
            unsafe { sys::AMediaCodec_queueInputBuffer(self.codec, index, 0, 0, 0, 0) };
            return Err(format!("a packet of {} bytes is too large", packet.len()));
        }
        // SAFETY: the buffer holds `size` bytes, at least `packet.len()`.
        unsafe { ptr::copy_nonoverlapping(packet.as_ptr(), buffer, packet.len()) };
        let time = self.count * PACKET_MICROS;
        self.count += 1;
        // SAFETY: the buffer holds `packet.len()` bytes now.
        let queued = unsafe {
            sys::AMediaCodec_queueInputBuffer(self.codec, index, 0, packet.len(), time, 0)
        };
        status(queued, "giving the Opus decoder a packet")
    }

    fn take(&mut self) -> Result<(), String> {
        let mut patience = OUT_PATIENCE;
        loop {
            let mut info =
                sys::AMediaCodecBufferInfo { offset: 0, size: 0, presentationTimeUs: 0, flags: 0 };
            // SAFETY: a started codec; `info` is written.
            let index = unsafe {
                sys::AMediaCodec_dequeueOutputBuffer(self.codec, &mut info, micros(patience))
            };
            if let Ok(index) = usize::try_from(index) {
                self.read(index, &info);
                // SAFETY: an index the codec gave, given back once.
                unsafe { sys::AMediaCodec_releaseOutputBuffer(self.codec, index, false) };
                // The next ones only if they are there already.
                patience = Duration::ZERO;
            } else if index == sys::AMEDIACODEC_INFO_OUTPUT_FORMAT_CHANGED as isize {
                self.read_format();
            } else if index == sys::AMEDIACODEC_INFO_OUTPUT_BUFFERS_CHANGED as isize {
                continue;
            } else if index == sys::AMEDIACODEC_INFO_TRY_AGAIN_LATER as isize {
                return Ok(());
            } else {
                return Err(format!("the Opus decoder failed ({index})"));
            }
        }
    }

    /// Adds the samples of output buffer `index` to `samples`.
    fn read(&mut self, index: usize, info: &sys::AMediaCodecBufferInfo) {
        if info.flags & sys::AMEDIACODEC_BUFFER_FLAG_CODEC_CONFIG != 0 {
            return;
        }
        let mut size = 0;
        // SAFETY: an index the codec gave; `size` is written.
        let buffer = unsafe { sys::AMediaCodec_getOutputBuffer(self.codec, index, &mut size) };
        let (Ok(offset), Ok(length)) = (usize::try_from(info.offset), usize::try_from(info.size))
        else {
            return;
        };
        if buffer.is_null() || offset + length > size {
            return;
        }
        // SAFETY: `size` bytes from `buffer`, read before it is given back.
        let bytes = unsafe { std::slice::from_raw_parts(buffer.add(offset), length) };
        if self.encoding == PCM_FLOAT {
            self.samples.extend(
                bytes.chunks_exact(4).map(|b| f32::from_ne_bytes([b[0], b[1], b[2], b[3]])),
            );
        } else {
            self.samples.extend(
                bytes
                    .chunks_exact(2)
                    .map(|b| f32::from(i16::from_ne_bytes([b[0], b[1]])) / 32_768.0),
            );
        }
    }

    /// Reads how the samples come out.
    fn read_format(&mut self) {
        // SAFETY: a started codec; the format is deleted below.
        let format = unsafe { sys::AMediaCodec_getOutputFormat(self.codec) };
        if format.is_null() {
            return;
        }
        let mut encoding = PCM_16;
        // SAFETY: a live format and a C string; `encoding` is written.
        let said =
            unsafe { sys::AMediaFormat_getInt32(format, c"pcm-encoding".as_ptr(), &mut encoding) };
        // SAFETY: deleted once.
        unsafe { sys::AMediaFormat_delete(format) };
        self.encoding = if said && encoding == PCM_FLOAT { PCM_FLOAT } else { PCM_16 };
    }
}

impl Drop for OpusDecoder {
    fn drop(&mut self) {
        // SAFETY: a codec this decoder made, stopped and deleted once.
        unsafe {
            sys::AMediaCodec_stop(self.codec);
            sys::AMediaCodec_delete(self.codec);
        }
    }
}

/// The 19 bytes of an `OpusHead` for one stream of `channels` channels.
fn head(sample_rate: u32, channels: u8) -> Vec<u8> {
    let mut head = b"OpusHead".to_vec();
    head.push(1); // version
    head.push(channels);
    head.extend(0_u16.to_le_bytes()); // samples to skip at the start
    head.extend(sample_rate.to_le_bytes()); // the rate the sound was made at
    head.extend(0_i16.to_le_bytes()); // gain
    head.push(0); // mapping family 0: one stream, left then right
    head
}

fn status(status: sys::media_status_t, what: &str) -> Result<(), String> {
    if status == sys::media_status_t::AMEDIA_OK {
        Ok(())
    } else {
        Err(format!("{what} failed ({status:?})"))
    }
}

fn micros(duration: Duration) -> i64 {
    i64::try_from(duration.as_micros()).unwrap_or(i64::MAX)
}
