//! The video decoders of an Android phone, through the NDK's MediaCodec:
//! on the phone's own video hardware, where a picture of the whole screen
//! decodes in a few milliseconds, which the processor alone cannot do.
//!
//! A picture comes out in the memory of the decoder, in NV12 or I420 as the
//! decoder chose, and is read there before the next one is asked for.

use std::{
    ffi::{CStr, CString, c_char, c_void},
    ptr,
    sync::OnceLock,
    time::{Duration, Instant},
};

use ndk_sys as sys;

/// How much of the time between two pictures a picture is waited for, once
/// its data went in. A decoder that keeps it longer gives it out with the
/// next one, and only the newer of the two is shown: a Fire TV Stick takes
/// about 8 ms, and waiting 4 ms lost 6 to 9 pictures of 60 each second.
/// Three quarters still leave the rest of the time for the next picture's
/// data to arrive.
const OUT_PATIENCE: f32 = 0.75;
/// How often the pictures given out together are told in the log.
const TELL_EVERY: Duration = Duration::from_secs(5);
/// How long a free input buffer is waited for.
const IN_PATIENCE: Duration = Duration::from_millis(20);

/// `MediaCodecInfo.CodecCapabilities.COLOR_FormatYUV420Planar`: I420.
const I420: i32 = 19;
/// `COLOR_FormatYUV420SemiPlanar`: NV12.
const NV12: i32 = 21;

/// How the planes of a picture lie in the decoder's memory.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layout {
    /// Luminance, then U and V interleaved.
    Nv12,
    /// Luminance, then U, then V, each at half the stride.
    I420,
}

/// A decoded picture, in the decoder's memory.
pub struct Picture<'a> {
    pub width: usize,
    pub height: usize,
    pub layout: Layout,
    /// Luminance, from the picture's first pixel, rows `stride` apart.
    pub luma: &'a [u8],
    pub stride: usize,
    /// The colour, from the picture's first pixel, rows `chroma_stride`
    /// apart: U and V interleaved in `u` for NV12, `u` then `v` for I420.
    pub u: &'a [u8],
    pub v: &'a [u8],
    pub chroma_stride: usize,
}

/// One decoder of the phone's.
pub struct Decoder {
    codec: *mut sys::AMediaCodec,
    name: String,
    /// Counts the pictures that went in, for their times.
    count: u64,
    /// The decoder's layout, once it said.
    shape: Option<Shape>,
    /// How long a picture is waited for: see `OUT_PATIENCE`.
    patience: Duration,
    /// Pictures given out behind a newer one and so never shown, since the
    /// log was last told, and when that was.
    passed_over: (u64, Instant),
    /// The pictures go onto a [`Reader`]'s, on the graphics card, instead of
    /// into the decoder's memory.
    onto: bool,
}

#[derive(Clone, Copy, Debug)]
struct Shape {
    width: usize,
    height: usize,
    stride: usize,
    rows: usize,
    layout: Layout,
    /// Where the picture starts in the decoder's, which may be larger.
    left: usize,
    top: usize,
}

// SAFETY: a MediaCodec may be used from any thread, one at a time, which
// `&mut self` ensures.
unsafe impl Send for Decoder {}

impl Decoder {
    /// A decoder for `mime` ("video/avc", "video/hevc", "video/av01"), for
    /// pictures `width` by `height` at `fps`.
    ///
    /// With `onto`, the pictures go onto that reader's, on the graphics card,
    /// and are never in memory: see [`Self::decode_onto`].
    pub fn new(
        mime: &str,
        width: u32,
        height: u32,
        fps: u32,
        onto: Option<&Reader>,
    ) -> Result<Self, String> {
        let mime_c = CString::new(mime).map_err(|error| error.to_string())?;
        // SAFETY: a valid C string; the codec is freed by `Drop`.
        let codec = unsafe { sys::AMediaCodec_createDecoderByType(mime_c.as_ptr()) };
        if codec.is_null() {
            return Err(format!("this phone has no {mime} decoder"));
        }
        let patience = Duration::from_secs_f32(OUT_PATIENCE / fps.max(1) as f32);
        let decoder = Self {
            codec,
            name: name(codec),
            count: 0,
            shape: None,
            patience,
            passed_over: (0, Instant::now()),
            onto: onto.is_some(),
        };
        // SAFETY: a new format, freed below.
        let format = unsafe { sys::AMediaFormat_new() };
        let int = |key: &CStr, value: i32| {
            // SAFETY: `format` lives until it is freed below; C strings.
            unsafe { sys::AMediaFormat_setInt32(format, key.as_ptr(), value) };
        };
        // SAFETY: as above.
        unsafe { sys::AMediaFormat_setString(format, c"mime".as_ptr(), mime_c.as_ptr()) };
        int(c"width", i32::try_from(width).unwrap_or(i32::MAX));
        int(c"height", i32::try_from(height).unwrap_or(i32::MAX));
        int(c"frame-rate", i32::try_from(fps).unwrap_or(60));
        // Out in NV12, which most decoders give anyway.
        int(c"color-format", NV12);
        // As soon as possible, and as fast as it can: what Moonlight asks,
        // the standard key and each maker's.
        int(c"low-latency", 1);
        int(c"priority", 0);
        int(c"operating-rate", i32::from(i16::MAX));
        int(c"vendor.qti-ext-dec-low-latency.enable", 1);
        int(c"vendor.qti-ext-dec-picture-order.enable", 1);
        int(c"vendor.rtc-ext-dec-low-latency.enable", 1);
        int(c"vendor.low-latency.enable", 1);
        int(c"vendor.hisi-ext-low-latency-video-dec.video-scene-for-low-latency-req", 1);
        int(c"vendor.hisi-ext-low-latency-video-dec.video-scene-for-low-latency-rdy", -1);
        let surface = onto.map_or(ptr::null_mut(), |reader| reader.window);
        // SAFETY: the codec and format are valid; a reader's window, which
        // outlives the decoder as its owner sees to, or none; no crypto.
        let configured =
            unsafe { sys::AMediaCodec_configure(codec, format, surface, ptr::null_mut(), 0) };
        // SAFETY: the format is no longer used.
        unsafe { sys::AMediaFormat_delete(format) };
        status(configured, "setting up the decoder")?;
        // SAFETY: a configured codec.
        status(unsafe { sys::AMediaCodec_start(codec) }, "starting the decoder")?;
        Ok(decoder)
    }

    /// The decoder's name, as the system knows it.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Whether the decoder is the phone's video hardware: Android's own
    /// decoders on the processor have names that say so.
    pub fn hardware(&self) -> bool {
        hardware(&self.name)
    }

    /// Gives the decoder one picture's data, and `show` the newest picture
    /// that came out meanwhile, if any. Returns whether one came out.
    pub fn decode(
        &mut self,
        data: &[u8],
        show: Option<&mut dyn FnMut(Picture<'_>)>,
    ) -> Result<bool, String> {
        self.put(data)?;
        self.take(show)
    }

    /// Decodes one picture onto the [`Reader`] the decoder was made with,
    /// when `shown`: it comes out there a moment later. A picture nobody
    /// will see is only decoded. Returns whether a picture came out.
    pub fn decode_onto(&mut self, data: &[u8], shown: bool) -> Result<bool, String> {
        self.put(data)?;
        let Some(index) = self.newest()? else { return Ok(false) };
        // SAFETY: an index the codec gave, given back once: onto the
        // reader's window when shown.
        unsafe { sys::AMediaCodec_releaseOutputBuffer(self.codec, index, shown && self.onto) };
        Ok(true)
    }

    fn put(&mut self, data: &[u8]) -> Result<(), String> {
        // SAFETY: a started codec.
        let index = unsafe { sys::AMediaCodec_dequeueInputBuffer(self.codec, micros(IN_PATIENCE)) };
        let Ok(index) = usize::try_from(index) else {
            return Err("the decoder takes no more data".into());
        };
        let mut size = 0;
        // SAFETY: an index the codec gave; `size` is written.
        let buffer = unsafe { sys::AMediaCodec_getInputBuffer(self.codec, index, &mut size) };
        if buffer.is_null() || size < data.len() {
            // SAFETY: the buffer goes back empty.
            unsafe { sys::AMediaCodec_queueInputBuffer(self.codec, index, 0, 0, 0, 0) };
            return Err(format!(
                "a picture of {} bytes is larger than the decoder takes",
                data.len()
            ));
        }
        // SAFETY: the buffer holds `size` bytes, at least `data.len()`.
        unsafe { ptr::copy_nonoverlapping(data.as_ptr(), buffer, data.len()) };
        self.count += 1;
        // Times only need to grow: a picture each sixtieth of a second.
        let time = self.count * 16_667;
        // SAFETY: the buffer holds `data.len()` bytes now.
        let queued =
            unsafe { sys::AMediaCodec_queueInputBuffer(self.codec, index, 0, data.len(), time, 0) };
        status(queued, "giving the decoder a picture")
    }

    fn take(&mut self, show: Option<&mut dyn FnMut(Picture<'_>)>) -> Result<bool, String> {
        let Some((index, info)) = self.newest_with_info()? else { return Ok(false) };
        let read = match show {
            Some(show) if !self.onto => self.read(index, &info, show),
            _ => Ok(()),
        };
        // SAFETY: an index the codec gave, given back once.
        unsafe { sys::AMediaCodec_releaseOutputBuffer(self.codec, index, false) };
        read.map(|()| true)
    }

    /// The newest picture out of the decoder, if one came in time.
    fn newest(&mut self) -> Result<Option<usize>, String> {
        Ok(self.newest_with_info()?.map(|(index, _)| index))
    }

    /// The newest picture out of the decoder and what it says of it, if one
    /// came in time. Older ones are given back: they are only late.
    fn newest_with_info(&mut self) -> Result<Option<(usize, sys::AMediaCodecBufferInfo)>, String> {
        let mut newest: Option<(usize, sys::AMediaCodecBufferInfo)> = None;
        let mut patience = self.patience;
        loop {
            let mut info =
                sys::AMediaCodecBufferInfo { offset: 0, size: 0, presentationTimeUs: 0, flags: 0 };
            // SAFETY: a started codec; `info` is written.
            let index = unsafe {
                sys::AMediaCodec_dequeueOutputBuffer(self.codec, &mut info, micros(patience))
            };
            if let Ok(index) = usize::try_from(index) {
                if let Some((older, _)) = newest.replace((index, info)) {
                    // SAFETY: an index the codec gave, given back once.
                    unsafe { sys::AMediaCodec_releaseOutputBuffer(self.codec, older, false) };
                    self.passed_over.0 += 1;
                }
                // The next one only if it is there already.
                patience = Duration::ZERO;
            } else if index == sys::AMEDIACODEC_INFO_OUTPUT_FORMAT_CHANGED as isize {
                // Onto a reader, the layout is the graphics card's business.
                if !self.onto {
                    self.shape = Some(self.shape()?);
                }
            } else if index != sys::AMEDIACODEC_INFO_OUTPUT_BUFFERS_CHANGED as isize {
                break;
            }
        }
        self.tell();
        Ok(newest)
    }

    /// Tells the log, every few seconds, of pictures that were never shown
    /// because the decoder gave them out together with newer ones.
    fn tell(&mut self) {
        let (count, since) = self.passed_over;
        if since.elapsed() >= TELL_EVERY {
            if count > 0 {
                log::info!(
                    "the decoder gave out {count} pictures behind newer ones in {} s, not shown",
                    TELL_EVERY.as_secs()
                );
            }
            self.passed_over = (0, Instant::now());
        }
    }

    fn read(
        &mut self,
        index: usize,
        info: &sys::AMediaCodecBufferInfo,
        show: &mut dyn FnMut(Picture<'_>),
    ) -> Result<(), String> {
        let shape = match self.shape {
            Some(shape) => shape,
            None => *self.shape.insert(self.shape()?),
        };
        let mut size = 0;
        // SAFETY: an index the codec gave; `size` is written.
        let buffer = unsafe { sys::AMediaCodec_getOutputBuffer(self.codec, index, &mut size) };
        if buffer.is_null() {
            return Err("the decoder gave a picture without memory".into());
        }
        let (Ok(offset), Ok(length)) = (usize::try_from(info.offset), usize::try_from(info.size))
        else {
            return Err("the decoder gave a picture of no size".into());
        };
        let length = length.min(size.saturating_sub(offset));
        // SAFETY: the buffer holds `size` bytes until it is given back,
        // after `show` returns.
        let data = unsafe { std::slice::from_raw_parts(buffer.add(offset), length) };
        // A picture cut out of a larger one starts further in.
        let luma_at = shape.top * shape.stride + shape.left;
        let colour = shape.rows * shape.stride;
        let (chroma_stride, u_at, v_at) = match shape.layout {
            Layout::Nv12 => {
                let at = colour + shape.top / 2 * shape.stride + shape.left / 2 * 2;
                (shape.stride, at, at + 1)
            }
            Layout::I420 => {
                let half = shape.stride / 2;
                let corner = shape.top / 2 * half + shape.left / 2;
                (half, colour + corner, colour + half * shape.rows.div_ceil(2) + corner)
            }
        };
        let from = |at: usize| data.get(at..).unwrap_or_default();
        show(Picture {
            width: shape.width,
            height: shape.height,
            layout: shape.layout,
            luma: from(luma_at),
            stride: shape.stride,
            u: from(u_at),
            v: from(v_at),
            chroma_stride,
        });
        Ok(())
    }

    /// How the decoder lays its pictures out, as it says.
    fn shape(&self) -> Result<Shape, String> {
        // SAFETY: a started codec; the format is freed below.
        let format = unsafe { sys::AMediaCodec_getOutputFormat(self.codec) };
        if format.is_null() {
            return Err("the decoder does not say how its pictures are laid out".into());
        }
        let int = |key: &CStr| {
            let mut value = 0;
            // SAFETY: a valid format and C string; `value` is written.
            unsafe { sys::AMediaFormat_getInt32(format, key.as_ptr(), &mut value) }.then_some(value)
        };
        let width = int(c"width").unwrap_or(0);
        let height = int(c"height").unwrap_or(0);
        let colour = int(c"color-format").unwrap_or(NV12);
        let stride = int(c"stride").filter(|&stride| stride > 0).unwrap_or(width);
        let rows = int(c"slice-height").filter(|&rows| rows > 0).unwrap_or(height);
        let crop = [int(c"crop-left"), int(c"crop-top"), int(c"crop-right"), int(c"crop-bottom")];
        // SAFETY: the format is no longer used.
        unsafe { sys::AMediaFormat_delete(format) };
        let (left, top, width, height) = match crop {
            [Some(left), Some(top), Some(right), Some(bottom)] => {
                (left, top, right - left + 1, bottom - top + 1)
            }
            _ => (0, 0, width, height),
        };
        let layout = match colour {
            I420 => Layout::I420,
            NV12 => Layout::Nv12,
            other => {
                return Err(format!(
                    "the decoder gives pictures in a layout Leto does not read ({other:#x})"
                ));
            }
        };
        let size = |value: i32| usize::try_from(value).unwrap_or(0);
        let shape = Shape {
            width: size(width),
            height: size(height),
            stride: size(stride),
            rows: size(rows),
            layout,
            left: size(left),
            top: size(top),
        };
        log::info!("the {} decoder gives {shape:?}", self.name);
        if shape.width == 0
            || shape.stride < shape.left + shape.width
            || shape.rows < shape.top + shape.height
        {
            return Err(format!("the decoder gives pictures Leto does not understand: {shape:?}"));
        }
        Ok(shape)
    }
}

impl Drop for Decoder {
    fn drop(&mut self) {
        // SAFETY: the codec was made in `new` and is freed once.
        unsafe {
            sys::AMediaCodec_stop(self.codec);
            sys::AMediaCodec_delete(self.codec);
        }
    }
}

/// How many pictures a [`Reader`] holds: the one being drawn, the next one
/// waiting, the one being taken from the decoder and one to spare.
const READER_HOLDS: i32 = 4;

/// Where a decoder leaves its pictures on the graphics card, for the window
/// to draw them from there (`AImageReader`). Nothing of a picture is ever in
/// memory: a Fire TV Stick spent 5 ms copying each one out of its decoder.
pub struct Reader {
    reader: *mut sys::AImageReader,
    window: *mut sys::ANativeWindow,
}

// SAFETY: an image reader may be used from any thread; its pictures are
// taken on the decoder's and given back on the window's, which it allows.
unsafe impl Send for Reader {}
// SAFETY: as above: what is shared is only read, or locked by the reader.
unsafe impl Sync for Reader {}

impl Reader {
    /// A reader for pictures `width` by `height` that the graphics card
    /// draws from.
    pub fn new(width: u32, height: u32) -> Result<Self, String> {
        let side = |value: u32| i32::try_from(value).map_err(|error| error.to_string());
        let mut reader = ptr::null_mut();
        // SAFETY: `reader` is written.
        let made = unsafe {
            sys::AImageReader_newWithUsage(
                side(width)?,
                side(height)?,
                sys::AIMAGE_FORMATS::AIMAGE_FORMAT_PRIVATE.0 as i32,
                sys::AHardwareBuffer_UsageFlags::AHARDWAREBUFFER_USAGE_GPU_SAMPLED_IMAGE.0,
                READER_HOLDS,
                &mut reader,
            )
        };
        status(made, "making a place for pictures on the graphics card")?;
        let mut window = ptr::null_mut();
        // SAFETY: a reader just made; `window` is written, and the reader's.
        let found = unsafe { sys::AImageReader_getWindow(reader, &mut window) };
        if let Err(error) = status(found, "finding where the decoder puts its pictures") {
            // SAFETY: made above, deleted once.
            unsafe { sys::AImageReader_delete(reader) };
            return Err(error);
        }
        Ok(Self { reader, window })
    }

    /// The newest picture the decoder left, older ones given back, or none
    /// if none came since the last.
    pub fn latest(&self) -> Option<Image> {
        let mut image = ptr::null_mut();
        // SAFETY: a live reader; `image` is written.
        let taken = unsafe { sys::AImageReader_acquireLatestImage(self.reader, &mut image) };
        (taken == sys::media_status_t::AMEDIA_OK && !image.is_null()).then_some(Image { image })
    }
}

impl Drop for Reader {
    fn drop(&mut self) {
        // SAFETY: made in `new`, deleted once, after its pictures: whoever
        // holds one holds what owns the reader.
        unsafe { sys::AImageReader_delete(self.reader) };
    }
}

/// A picture on the graphics card, in a [`Reader`]. It goes back to the
/// reader when dropped, so it must be dropped before the reader is.
pub struct Image {
    image: *mut sys::AImage,
}

// SAFETY: a picture of a reader may be given back from any thread.
unsafe impl Send for Image {}

impl Image {
    /// The picture's buffer on the card (an `AHardwareBuffer`), as long as
    /// the picture lives.
    pub fn buffer(&self) -> *mut c_void {
        let mut buffer = ptr::null_mut();
        // SAFETY: a live picture; `buffer` is written.
        unsafe { sys::AImage_getHardwareBuffer(self.image, &mut buffer) };
        buffer.cast()
    }

    /// Where the picture is in its buffer, which may be larger, as parts of
    /// the buffer's width and height: left, top, width, height.
    ///
    /// A decoder's buffer is often taller than the picture: 1088 lines for
    /// 1080, the last 8 green. The picture's own width and height say the
    /// picture's, not the buffer's, so the buffer is asked.
    pub fn place(&self) -> [f32; 4] {
        let (mut width, mut height) = (0, 0);
        let mut crop = sys::AImageCropRect { left: 0, top: 0, right: 0, bottom: 0 };
        // SAFETY: a live picture; each is written.
        let said = unsafe {
            sys::AImage_getWidth(self.image, &mut width) == sys::media_status_t::AMEDIA_OK
                && sys::AImage_getHeight(self.image, &mut height) == sys::media_status_t::AMEDIA_OK
        };
        if !said {
            return [0.0, 0.0, 1.0, 1.0];
        }
        // SAFETY: as above.
        let cropped = unsafe {
            sys::AImage_getCropRect(self.image, &mut crop) == sys::media_status_t::AMEDIA_OK
        };
        let crop = if cropped { [crop.left, crop.top, crop.right, crop.bottom] } else { [0; 4] };
        let mut whole = (width, height);
        let buffer = self.buffer();
        if !buffer.is_null() {
            // SAFETY: the picture's live buffer; the description is written.
            let described = unsafe {
                let mut description = std::mem::zeroed::<sys::AHardwareBuffer_Desc>();
                sys::AHardwareBuffer_describe(buffer.cast(), &mut description);
                description
            };
            if let (Ok(across), Ok(down)) =
                (i32::try_from(described.width), i32::try_from(described.height))
                && across > 0
                && down > 0
            {
                whole = (across, down);
            }
        }
        place_in(crop, (width, height), whole)
    }
}

/// Where a picture of `picture` pixels, cut at `crop` (left, top, right,
/// bottom; none when empty), is in a buffer of `whole` pixels, as parts of
/// the buffer: left, top, width, height.
fn place_in(crop: [i32; 4], picture: (i32, i32), whole: (i32, i32)) -> [f32; 4] {
    let [mut left, mut top, mut right, mut bottom] = crop;
    if right <= left || bottom <= top || left < 0 || top < 0 {
        [left, top, right, bottom] = [0, 0, picture.0, picture.1];
    }
    let (across, down) = (whole.0.max(right), whole.1.max(bottom));
    if across <= 0 || down <= 0 || right <= left || bottom <= top {
        return [0.0, 0.0, 1.0, 1.0];
    }
    let (across, down) = (across as f32, down as f32);
    [
        left as f32 / across,
        top as f32 / down,
        (right - left) as f32 / across,
        (bottom - top) as f32 / down,
    ]
}

impl Drop for Image {
    fn drop(&mut self) {
        // SAFETY: taken from a reader that still lives, given back once.
        unsafe { sys::AImage_delete(self.image) };
    }
}

/// Whether the phone decodes `mime` on its video hardware. Found out once
/// for each, by opening a decoder.
pub fn decodes_on_hardware(mime: &str) -> bool {
    static FOUND: OnceLock<[(String, bool); 3]> = OnceLock::new();
    let found = FOUND.get_or_init(|| {
        ["video/avc", "video/hevc", "video/av01"].map(|mime| {
            let on = Decoder::new(mime, 1280, 720, 60, None).is_ok_and(|decoder| {
                log::info!("{mime} is decoded by {}", decoder.name());
                decoder.hardware()
            });
            (mime.to_owned(), on)
        })
    });
    found.iter().any(|(known, on)| known == mime && *on)
}

/// Whether a decoder named `name` is the phone's video hardware: Android's
/// own decoders on the processor are named `c2.android.…` or
/// `OMX.google.…`.
fn hardware(name: &str) -> bool {
    let name = name.to_lowercase();
    !name.is_empty()
        && !name.starts_with("c2.android.")
        && !name.starts_with("omx.google.")
        && !name.contains(".sw.")
        && !name.ends_with(".sw")
}

/// The name of `codec`. `AMediaCodec_getName` came with Android 9: it is
/// looked for, so that the app still starts on Android 8.
fn name(codec: *mut sys::AMediaCodec) -> String {
    type GetName =
        unsafe extern "C" fn(*mut sys::AMediaCodec, *mut *mut c_char) -> sys::media_status_t;
    type ReleaseName = unsafe extern "C" fn(*mut sys::AMediaCodec, *mut c_char);
    // SAFETY: the C library's own lookup, with C strings.
    let (get, release) = unsafe {
        (
            libc::dlsym(libc::RTLD_DEFAULT, c"AMediaCodec_getName".as_ptr()),
            libc::dlsym(libc::RTLD_DEFAULT, c"AMediaCodec_releaseName".as_ptr()),
        )
    };
    if get.is_null() || release.is_null() {
        return String::new();
    }
    // SAFETY: the symbols have these types in the NDK's media library.
    let (get, release) = unsafe {
        (
            std::mem::transmute::<*mut c_void, GetName>(get),
            std::mem::transmute::<*mut c_void, ReleaseName>(release),
        )
    };
    let mut out = ptr::null_mut();
    // SAFETY: a valid codec; `out` is written, then given back.
    if unsafe { get(codec, &mut out) } != sys::media_status_t::AMEDIA_OK || out.is_null() {
        return String::new();
    }
    // SAFETY: a C string the codec gave, read before it is given back.
    let name = unsafe { CStr::from_ptr(out) }.to_string_lossy().into_owned();
    // SAFETY: given back once.
    unsafe { release(codec, out) };
    name
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

#[cfg(test)]
mod place_tests {
    use super::place_in;

    #[test]
    fn a_picture_in_a_taller_buffer_is_drawn_without_the_lines_under_it() {
        let place = place_in([0, 0, 1920, 1080], (1920, 1080), (1920, 1088));
        // Each line of the picture is read from one line of the buffer.
        assert_eq!(place, [0.0, 0.0, 1.0, 1080.0 / 1088.0]);
        // No cut told: the picture's own size is the cut.
        assert_eq!(
            place_in([0; 4], (1920, 1080), (1920, 1088)),
            place_in([0, 0, 1920, 1080], (1920, 1080), (1920, 1088))
        );
        // A buffer said smaller than the cut is not believed.
        assert_eq!(place_in([0, 0, 1920, 1080], (1920, 1080), (0, 0)), [0.0, 0.0, 1.0, 1.0]);
    }
}
