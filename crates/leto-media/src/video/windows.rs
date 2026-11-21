//! Pictures decoded by Windows itself, with Media Foundation: on the
//! graphics card when it decodes the codec, which is where H.265 and AV1
//! are worth having.
//!
//! The decoders are Microsoft's own, which Windows has for H.264 and gets
//! for H.265 and AV1 from the Store's video extensions. They are
//! synchronous: a picture goes in, and comes out decoded on the same
//! thread. Given a Direct3D 11 device they decode on the graphics card, and
//! the picture is read back from it.

use std::{cell::Cell, ptr, sync::OnceLock};

use leto_core::engine::{Codec, Codecs, Decodable};
use windows::{
    Win32::{
        Foundation::HMODULE,
        Graphics::{
            Direct3D::D3D_DRIVER_TYPE_HARDWARE,
            Direct3D11::{
                D3D11_CREATE_DEVICE_VIDEO_SUPPORT, D3D11_DECODER_PROFILE_AV1_VLD_PROFILE0,
                D3D11_DECODER_PROFILE_H264_VLD_NOFGT, D3D11_DECODER_PROFILE_HEVC_VLD_MAIN,
                D3D11_SDK_VERSION, D3D11CreateDevice, ID3D11Device, ID3D11Multithread,
                ID3D11VideoDevice,
            },
            Dxgi::IDXGIAdapter,
        },
        Media::MediaFoundation::{
            IMF2DBuffer2, IMFActivate, IMFDXGIDeviceManager, IMFMediaType, IMFSample, IMFTransform,
            MF_E_NOTACCEPTING, MF_E_TRANSFORM_NEED_MORE_INPUT, MF_E_TRANSFORM_STREAM_CHANGE,
            MF_LOW_LATENCY, MF_MT_FRAME_RATE, MF_MT_FRAME_SIZE, MF_MT_INTERLACE_MODE,
            MF_MT_MAJOR_TYPE, MF_MT_MINIMUM_DISPLAY_APERTURE, MF_MT_SUBTYPE, MF_SA_D3D11_AWARE,
            MF_VERSION, MF2DBuffer_LockFlags_Read, MFCreateDXGIDeviceManager, MFCreateMediaType,
            MFCreateMemoryBuffer, MFCreateSample, MFMediaType_Video, MFSTARTUP_LITE, MFStartup,
            MFT_CATEGORY_VIDEO_DECODER, MFT_ENUM_FLAG_LOCALMFT, MFT_ENUM_FLAG_SORTANDFILTER,
            MFT_ENUM_FLAG_SYNCMFT, MFT_MESSAGE_NOTIFY_BEGIN_STREAMING,
            MFT_MESSAGE_NOTIFY_START_OF_STREAM, MFT_MESSAGE_SET_D3D_MANAGER,
            MFT_OUTPUT_DATA_BUFFER, MFT_OUTPUT_STREAM_CAN_PROVIDE_SAMPLES,
            MFT_OUTPUT_STREAM_PROVIDES_SAMPLES, MFT_REGISTER_TYPE_INFO, MFTEnumEx, MFVideoArea,
            MFVideoFormat_AV1, MFVideoFormat_H264, MFVideoFormat_HEVC, MFVideoFormat_NV12,
            MFVideoInterlace_Progressive,
        },
        System::Com::{COINIT_MULTITHREADED, CoInitializeEx, CoTaskMemFree},
    },
    core::{GUID, Interface},
};

use crate::{
    Error,
    convert::{Nv12, Planes},
};

/// The size a decoder is tried at, to know that it works.
const TRIED: (u32, u32, u32) = (1280, 720, 60);

/// The codecs Windows decodes on this device: on the graphics card, and at
/// all. Each decoder is started once to know, then that is remembered.
pub fn decodable() -> Decodable {
    static FOUND: OnceLock<Decodable> = OnceLock::new();
    *FOUND.get_or_init(|| {
        let none = Decodable { hardware: Codecs::NONE, any: Codecs::NONE };
        if let Err(error) = start() {
            log::info!("Windows decodes no video for Leto: {}", error.message());
            return none;
        }
        // A decoder that works on a card without the codec works on the
        // processor: the card has to say it decodes it.
        let profiles = graphics_profiles().unwrap_or_else(|error| {
            log::info!("the graphics card decodes no video for Leto: {}", error.message());
            Vec::new()
        });
        let mut found = none;
        for codec in Codec::BEST_FIRST {
            let (width, height, fps) = TRIED;
            let card = profiles.contains(&profile(codec));
            match Decoder::new(codec, width, height, fps, card) {
                Ok(decoder) => {
                    found.any = found.any.with(codec);
                    if decoder.on_graphics_card() {
                        found.hardware = found.hardware.with(codec);
                    }
                }
                Err(error) => log::debug!("Windows decodes no {}: {error}", codec.name()),
            }
        }
        log::info!("Windows decodes {:?}, on the graphics card {:?}", found.any, found.hardware);
        found
    })
}

/// One stream's pictures, decoded by Windows.
///
/// It is used by one thread at a time, which may not be the one that made
/// it: Microsoft's decoders and Direct3D devices live in the multithreaded
/// apartment, where any thread may call them.
pub struct Decoder {
    transform: IMFTransform,
    /// The graphics card the pictures are decoded on, when they are.
    card: Option<(ID3D11Device, IMFDXGIDeviceManager)>,
    /// The decoder hands out its own samples, as it does on the card.
    gives_samples: bool,
    /// How large a sample to hand it otherwise.
    sample_size: u32,
    /// The size of the pictures that come out: the size the decoder works
    /// at, rows padded, and the part of it that is shown.
    coded: (usize, usize),
    shown: (usize, usize),
    /// Time of the next picture and how long each lasts, in 100 ns.
    time: i64,
    step: i64,
}

// SAFETY: every object in it was made in the multithreaded apartment, where
// COM lets any thread call it; the decoder takes `&mut self` for all it
// does, so no two threads call it at once.
unsafe impl Send for Decoder {}

impl Decoder {
    /// A decoder for `codec`, on the graphics card when `on_card` and it
    /// can. The stream is `width` by `height` at `fps`.
    pub fn new(
        codec: Codec,
        width: u32,
        height: u32,
        fps: u32,
        on_card: bool,
    ) -> Result<Self, Error> {
        start().map_err(|error| failed("Media Foundation did not start", &error))?;
        let activate = decoder(codec)
            .ok_or_else(|| Error::Video(format!("Windows has no {} decoder", codec.name())))?;
        // SAFETY: Media Foundation started on this thread, and the object
        // made is the decoder's own interface.
        let transform: IMFTransform = unsafe { activate.ActivateObject() }
            .map_err(|error| failed("the decoder did not start", &error))?;
        let attributes = unsafe { transform.GetAttributes() }.ok();
        if let Some(attributes) = &attributes {
            // A picture out for every picture in, at once.
            // SAFETY: setting a number on the decoder's own attributes.
            let _ = unsafe { attributes.SetUINT32(&MF_LOW_LATENCY, 1) };
        }
        let aware = attributes
            .as_ref()
            // SAFETY: reading a number from the decoder's own attributes.
            .and_then(|attributes| unsafe { attributes.GetUINT32(&MF_SA_D3D11_AWARE) }.ok())
            .is_some_and(|aware| aware != 0);
        let card = if on_card && aware {
            match graphics_card(&transform) {
                Ok(card) => Some(card),
                Err(error) => {
                    log::info!("{} is decoded on the processor: {error}", codec.name());
                    None
                }
            }
        } else {
            None
        };

        input_type(codec, width, height, fps)
            // SAFETY: the type is handed to the decoder, which keeps its own
            // reference.
            .and_then(|input| unsafe { transform.SetInputType(0, &input, 0) })
            .map_err(|error| failed("the decoder refused the stream", &error))?;
        let mut decoder = Self {
            transform,
            card,
            gives_samples: false,
            sample_size: 0,
            coded: (width as usize, height as usize),
            shown: (width as usize, height as usize),
            time: 0,
            step: 10_000_000 / i64::from(fps.max(1)),
        };
        decoder.choose_output()?;
        // SAFETY: plain messages to the decoder, which is ready for them.
        // Decoders that need neither ignore them.
        unsafe {
            let _ = decoder.transform.ProcessMessage(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0);
            let _ = decoder.transform.ProcessMessage(MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0);
        }
        Ok(decoder)
    }

    pub fn on_graphics_card(&self) -> bool {
        self.card.is_some()
    }

    /// Decodes one picture, and copies it into `planes` when they are given.
    /// Returns whether there was a picture.
    pub fn decode(&mut self, data: &[u8], planes: Option<&mut Planes>) -> Result<bool, Error> {
        // The thread that decodes may not be the one that made the decoder.
        start().map_err(|error| failed("Media Foundation did not start", &error))?;
        let sample = self.sample(data)?;
        // SAFETY: the sample was made for this call and is the decoder's to
        // read from now on.
        match unsafe { self.transform.ProcessInput(0, &sample, 0) } {
            Ok(()) => {}
            Err(error) if error.code() == MF_E_NOTACCEPTING => {
                // Pictures are waiting: they are dropped, which is rare.
                self.take_output(None)?;
                // SAFETY: as above.
                unsafe { self.transform.ProcessInput(0, &sample, 0) }
                    .map_err(|error| failed("the decoder took no picture", &error))?;
            }
            Err(error) => return Err(failed("the decoder took no picture", &error)),
        }
        self.take_output(planes)
    }

    /// The data of one picture, as a sample the decoder reads.
    fn sample(&mut self, data: &[u8]) -> Result<IMFSample, Error> {
        let length =
            u32::try_from(data.len()).map_err(|_| Error::Video("a picture too large".into()))?;
        // SAFETY: the buffer is made at the length of `data`, which is
        // copied into it while it is locked, and nowhere past it.
        unsafe {
            let buffer =
                MFCreateMemoryBuffer(length).map_err(|error| failed("no memory", &error))?;
            let mut start = ptr::null_mut();
            buffer.Lock(&mut start, None, None).map_err(|error| failed("no memory", &error))?;
            ptr::copy_nonoverlapping(data.as_ptr(), start, data.len());
            let _ = buffer.Unlock();
            buffer.SetCurrentLength(length).map_err(|error| failed("no memory", &error))?;
            let sample = MFCreateSample().map_err(|error| failed("no sample", &error))?;
            sample.AddBuffer(&buffer).map_err(|error| failed("no sample", &error))?;
            let _ = sample.SetSampleTime(self.time);
            let _ = sample.SetSampleDuration(self.step);
            self.time += self.step;
            Ok(sample)
        }
    }

    /// Takes every picture the decoder has ready. Each goes into `planes`,
    /// when they are given, so the last one stays.
    fn take_output(&mut self, mut planes: Option<&mut Planes>) -> Result<bool, Error> {
        let mut shown = false;
        loop {
            let given = if self.gives_samples { None } else { Some(self.empty_sample()?) };
            let mut output = [MFT_OUTPUT_DATA_BUFFER {
                dwStreamID: 0,
                pSample: std::mem::ManuallyDrop::new(given),
                dwStatus: 0,
                pEvents: std::mem::ManuallyDrop::new(None),
            }];
            let mut status = 0;
            // SAFETY: one output buffer, as the decoder has one stream. What
            // it puts in the buffer is taken back below, so nothing leaks.
            let result = unsafe { self.transform.ProcessOutput(0, &mut output, &mut status) };
            let [output] = output;
            let sample = std::mem::ManuallyDrop::into_inner(output.pSample);
            drop(std::mem::ManuallyDrop::into_inner(output.pEvents));
            match result {
                Ok(()) => {
                    if let (Some(sample), Some(planes)) = (sample, planes.as_deref_mut()) {
                        shown |= self.read(&sample, planes)?;
                    }
                }
                Err(error) if error.code() == MF_E_TRANSFORM_NEED_MORE_INPUT => return Ok(shown),
                // The size, or the padding, changed: say what comes out now.
                Err(error) if error.code() == MF_E_TRANSFORM_STREAM_CHANGE => {
                    self.choose_output()?
                }
                Err(error) => return Err(failed("the decoder gave no picture", &error)),
            }
        }
    }

    /// A sample for the decoder to write a picture into, when it gives none
    /// of its own.
    fn empty_sample(&self) -> Result<IMFSample, Error> {
        // SAFETY: a buffer of the size the decoder asked for, in a sample.
        unsafe {
            let buffer = MFCreateMemoryBuffer(self.sample_size)
                .map_err(|error| failed("no memory", &error))?;
            let sample = MFCreateSample().map_err(|error| failed("no sample", &error))?;
            sample.AddBuffer(&buffer).map_err(|error| failed("no sample", &error))?;
            Ok(sample)
        }
    }

    /// Asks for NV12, and learns how large the pictures are.
    fn choose_output(&mut self) -> Result<(), Error> {
        let mut chosen = None;
        for index in 0.. {
            // SAFETY: the types the decoder offers, one by one, until none.
            let Ok(offered) = (unsafe { self.transform.GetOutputAvailableType(0, index) }) else {
                break;
            };
            // SAFETY: reading the subtype of a media type.
            if unsafe { offered.GetGUID(&MF_MT_SUBTYPE) }
                .is_ok_and(|subtype| subtype == MFVideoFormat_NV12)
            {
                chosen = Some(offered);
                break;
            }
        }
        let chosen = chosen.ok_or_else(|| Error::Video("the decoder gives no NV12".into()))?;
        // SAFETY: the type the decoder offered, given back.
        unsafe { self.transform.SetOutputType(0, &chosen, 0) }
            .map_err(|error| failed("the decoder refused NV12", &error))?;
        self.learn_size(&chosen);
        // SAFETY: asking what the decoder's output is like.
        let info = unsafe { self.transform.GetOutputStreamInfo(0) }
            .map_err(|error| failed("the decoder hides its output", &error))?;
        let own =
            (MFT_OUTPUT_STREAM_PROVIDES_SAMPLES.0 | MFT_OUTPUT_STREAM_CAN_PROVIDE_SAMPLES.0) as u32;
        self.gives_samples = info.dwFlags & own != 0;
        self.sample_size = info.cbSize.max(1);
        Ok(())
    }

    fn learn_size(&mut self, kind: &IMFMediaType) {
        // SAFETY: reading a number from a media type.
        if let Ok(size) = unsafe { kind.GetUINT64(&MF_MT_FRAME_SIZE) } {
            let (width, height) = ((size >> 32) as usize, (size & 0xFFFF_FFFF) as usize);
            self.coded = (width, height);
            self.shown = (width, height);
        }
        // What is shown of the picture can be smaller: 1080 rows of 1088.
        let mut area = [0u8; std::mem::size_of::<MFVideoArea>()];
        // SAFETY: the area is read into a buffer of its own size.
        let read = unsafe { kind.GetBlob(&MF_MT_MINIMUM_DISPLAY_APERTURE, &mut area, None) };
        if read.is_ok() {
            // SAFETY: the bytes are an MFVideoArea, which is plain numbers.
            let area: MFVideoArea = unsafe { ptr::read_unaligned(area.as_ptr().cast()) };
            let (width, height) = (area.Area.cx, area.Area.cy);
            if width > 0 && height > 0 {
                self.shown =
                    ((width as usize).min(self.coded.0), (height as usize).min(self.coded.1));
            }
        }
    }

    /// Copies the picture in `sample` into `planes`.
    fn read(&self, sample: &IMFSample, planes: &mut Planes) -> Result<bool, Error> {
        // SAFETY: reading the sample the decoder just gave.
        let buffer = unsafe { sample.GetBufferByIndex(0) }
            .map_err(|error| failed("a picture without pixels", &error))?;
        if let Ok(rows) = buffer.cast::<IMF2DBuffer2>() {
            // Rows as the decoder laid them out, on the graphics card or not.
            let (mut first_row, mut pitch, mut start, mut length) =
                (ptr::null_mut(), 0i32, ptr::null_mut(), 0u32);
            // SAFETY: locked for reading, and unlocked once read.
            unsafe {
                rows.Lock2DSize(
                    MF2DBuffer_LockFlags_Read,
                    &mut first_row,
                    &mut pitch,
                    &mut start,
                    &mut length,
                )
                .map_err(|error| failed("the picture could not be read", &error))?;
                let read = self.copy(start, length, first_row.offset_from(start), pitch, planes);
                let _ = rows.Unlock2D();
                read
            }
        } else {
            // One block, rows as wide as the picture.
            let (mut start, mut length) = (ptr::null_mut(), 0u32);
            // SAFETY: as above.
            unsafe {
                buffer
                    .Lock(&mut start, None, Some(&mut length))
                    .map_err(|error| failed("the picture could not be read", &error))?;
                let pitch = i32::try_from(self.coded.0).unwrap_or(0);
                let read = self.copy(start, length, 0, pitch, planes);
                let _ = buffer.Unlock();
                read
            }
        }
    }

    /// Copies the NV12 at `offset` in the `length` bytes from `start`, rows
    /// `pitch` bytes apart, into `planes`.
    ///
    /// # Safety
    ///
    /// `start` points to `length` bytes that can be read until this returns.
    unsafe fn copy(
        &self,
        start: *const u8,
        length: u32,
        offset: isize,
        pitch: i32,
        planes: &mut Planes,
    ) -> Result<bool, Error> {
        // SAFETY: as the caller promises.
        let bytes = unsafe { std::slice::from_raw_parts(start, length as usize) };
        let laid_out = usize::try_from(offset).ok().and_then(|offset| bytes.get(offset..));
        let split = laid_out.zip(usize::try_from(pitch).ok()).and_then(|(laid_out, stride)| {
            let luma = stride.checked_mul(self.rows(laid_out.len(), stride))?;
            let (luma, chroma) = laid_out.split_at_checked(luma)?;
            Some((luma, chroma, stride))
        });
        let (width, height) = self.shown;
        match split {
            Some((luma, chroma, stride))
                if planes.copy_nv12(&Nv12 { width, height, stride, luma, chroma }) =>
            {
                Ok(true)
            }
            _ => Err(Error::Video("a decoded picture is smaller than it says".into())),
        }
    }
}

impl Decoder {
    /// The rows of brightness above the colours, in planes `length` bytes
    /// long whose rows are `stride` apart. A picture on the graphics card
    /// can have more rows than the decoder works with, so they are found
    /// from the length when it is just that of NV12: a row of colour for
    /// every two of brightness.
    const fn rows(&self, length: usize, stride: usize) -> usize {
        let row_pairs = if stride == 0 { 0 } else { length / (stride * 3) };
        if row_pairs * stride * 3 == length && row_pairs * 2 >= self.coded.1 {
            row_pairs * 2
        } else {
            self.coded.1
        }
    }
}

thread_local! {
    /// Media Foundation was started for this thread.
    static STARTED: Cell<bool> = const { Cell::new(false) };
}

/// Readies COM and Media Foundation on this thread.
fn start() -> windows::core::Result<()> {
    if STARTED.get() {
        return Ok(());
    }
    // SAFETY: plain calls, once per thread. A thread whose COM was started
    // another way works all the same.
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        MFStartup(MF_VERSION, MFSTARTUP_LITE)?;
    }
    STARTED.set(true);
    Ok(())
}

/// What a stream in `codec` is like, for the decoder.
fn input_type(
    codec: Codec,
    width: u32,
    height: u32,
    fps: u32,
) -> windows::core::Result<IMFMediaType> {
    // SAFETY: a media type made here, which only this function changes.
    unsafe {
        let input = MFCreateMediaType()?;
        input.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
        input.SetGUID(&MF_MT_SUBTYPE, &subtype(codec))?;
        input.SetUINT64(&MF_MT_FRAME_SIZE, pair(width, height))?;
        input.SetUINT64(&MF_MT_FRAME_RATE, pair(fps.max(1), 1))?;
        input.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
        Ok(input)
    }
}

/// Microsoft's decoder of `codec`, when Windows has one.
fn decoder(codec: Codec) -> Option<IMFActivate> {
    let input =
        MFT_REGISTER_TYPE_INFO { guidMajorType: MFMediaType_Video, guidSubtype: subtype(codec) };
    let mut found: *mut Option<IMFActivate> = ptr::null_mut();
    let mut count = 0u32;
    // SAFETY: Windows fills the list and its length; each entry is taken
    // out of it, and the list itself freed, as its documentation says.
    unsafe {
        let flags = MFT_ENUM_FLAG_SYNCMFT | MFT_ENUM_FLAG_LOCALMFT | MFT_ENUM_FLAG_SORTANDFILTER;
        let input = ptr::from_ref(&input);
        MFTEnumEx(MFT_CATEGORY_VIDEO_DECODER, flags, Some(input), None, &mut found, &mut count)
            .ok()?;
        if found.is_null() {
            return None;
        }
        let entries = std::slice::from_raw_parts_mut(found, count as usize);
        let first = entries.iter_mut().find_map(Option::take);
        for entry in entries.iter_mut() {
            drop(entry.take());
        }
        CoTaskMemFree(Some(found.cast_const().cast()));
        first
    }
}

/// A Direct3D 11 device on the graphics card, handed to `transform`.
fn graphics_card(
    transform: &IMFTransform,
) -> windows::core::Result<(ID3D11Device, IMFDXGIDeviceManager)> {
    let device = device()?;
    // SAFETY: the device is shared with the decoder's own threads, under
    // the lock Direct3D keeps when asked to.
    unsafe {
        if let Ok(shared) = device.cast::<ID3D11Multithread>() {
            let _ = shared.SetMultithreadProtected(true);
        }
        let mut token = 0;
        let mut manager = None;
        MFCreateDXGIDeviceManager(&mut token, &mut manager)?;
        let manager = manager
            .ok_or_else(|| windows::core::Error::from(windows::Win32::Foundation::E_POINTER))?;
        manager.ResetDevice(&device, token)?;
        transform.ProcessMessage(MFT_MESSAGE_SET_D3D_MANAGER, manager.as_raw() as usize)?;
        Ok((device, manager))
    }
}

fn device() -> windows::core::Result<ID3D11Device> {
    let mut device = None;
    // SAFETY: the default graphics card, with video, at the levels it has.
    unsafe {
        D3D11CreateDevice(
            None::<&IDXGIAdapter>,
            D3D_DRIVER_TYPE_HARDWARE,
            HMODULE::default(),
            D3D11_CREATE_DEVICE_VIDEO_SUPPORT,
            None,
            D3D11_SDK_VERSION,
            Some(&mut device),
            None,
            None,
        )?;
    }
    device.ok_or_else(|| windows::core::Error::from(windows::Win32::Foundation::E_POINTER))
}

/// What the graphics card decodes.
fn graphics_profiles() -> windows::core::Result<Vec<GUID>> {
    let video = device()?.cast::<ID3D11VideoDevice>()?;
    // SAFETY: the profiles are read by their index, up to their count.
    let count = unsafe { video.GetVideoDecoderProfileCount() };
    Ok((0..count).filter_map(|index| unsafe { video.GetVideoDecoderProfile(index) }.ok()).collect())
}

const fn subtype(codec: Codec) -> GUID {
    match codec {
        Codec::H264 => MFVideoFormat_H264,
        Codec::H265 => MFVideoFormat_HEVC,
        Codec::Av1 => MFVideoFormat_AV1,
    }
}

const fn profile(codec: Codec) -> GUID {
    match codec {
        Codec::H264 => D3D11_DECODER_PROFILE_H264_VLD_NOFGT,
        Codec::H265 => D3D11_DECODER_PROFILE_HEVC_VLD_MAIN,
        Codec::Av1 => D3D11_DECODER_PROFILE_AV1_VLD_PROFILE0,
    }
}

/// Two numbers in one, as Media Foundation keeps sizes and rates.
fn pair(high: u32, low: u32) -> u64 {
    (u64::from(high) << 32) | u64::from(low)
}

fn failed(what: &str, error: &windows::core::Error) -> Error {
    Error::Video(format!("{what}: {}", error.message()))
}

#[cfg(test)]
mod tests {
    use openh264::{
        OpenH264API,
        encoder::{Encoder, EncoderConfig, QpRange, RateControlMode},
        formats::{RgbSliceU8, YUVBuffer},
    };

    use super::*;
    use crate::{Picture, VideoDecoder};

    /// Not a size a decoder works at: it rounds the rows up to 80, and only
    /// 72 of them are shown.
    const WIDTH: usize = 128;
    const HEIGHT: usize = 72;
    const ORANGE: [u8; 3] = [255, 107, 53];
    const BLUE: [u8; 3] = [20, 40, 200];

    /// Pictures of an orange left half and a blue right one, in H.264.
    fn pictures(count: usize) -> Vec<Vec<u8>> {
        let mut rgb = Vec::with_capacity(WIDTH * HEIGHT * 3);
        for _ in 0..HEIGHT {
            for column in 0..WIDTH {
                rgb.extend(if column < WIDTH / 2 { ORANGE } else { BLUE });
            }
        }
        let picture = YUVBuffer::from_rgb_source(RgbSliceU8::new(&rgb, (WIDTH, HEIGHT)));
        let config = EncoderConfig::new()
            .rate_control_mode(RateControlMode::Off)
            .qp(QpRange::new(18, 18))
            .skip_frames(false);
        let mut encoder = Encoder::with_api_config(OpenH264API::from_source(), config)
            .expect("the encoder starts");
        (0..count).map(|_| encoder.encode(&picture).expect("a picture encodes").to_vec()).collect()
    }

    fn assert_colours(picture: &Picture) {
        assert_eq!((picture.width, picture.height), (WIDTH, HEIGHT));
        for (column, wanted) in [(10, ORANGE), (WIDTH - 10, BLUE)] {
            for row in [2, HEIGHT / 2, HEIGHT - 2] {
                let at = (row * WIDTH + column) * 4;
                let found = &picture.rgba[at..at + 4];
                let near =
                    found.iter().zip(wanted).all(|(found, wanted)| found.abs_diff(wanted) <= 8);
                assert!(near && found[3] == 255, "{found:?} at {column}, {row}, not {wanted:?}");
            }
        }
    }

    /// Decodes `pictures` and gives the last picture that came out, in
    /// pixels.
    fn decoded(decoder: &mut Decoder, pictures: &[Vec<u8>]) -> Option<Picture> {
        let mut last = None;
        for data in pictures {
            let mut planes = Planes::default();
            if decoder.decode(data, Some(&mut planes)).expect("a picture decodes") {
                last = Some(planes);
            }
        }
        let mut picture = Picture::default();
        last.is_some_and(|planes| planes.to_rgba(&mut picture)).then_some(picture)
    }

    #[test]
    fn windows_decodes_h264_with_its_colours_where_it_has_a_decoder() {
        let found = decodable();
        assert!(
            found.any.and(found.hardware) == found.any,
            "the card decodes only what Windows does"
        );
        let pictures = pictures(8);
        if found.any.contains(Codec::H264) {
            // On the processor, then on the graphics card where it can.
            for on_card in [false, found.hardware.contains(Codec::H264)] {
                let mut decoder =
                    Decoder::new(Codec::H264, WIDTH as u32, HEIGHT as u32, 30, on_card)
                        .expect("the decoder Windows said it has starts");
                assert_eq!(decoder.on_graphics_card(), on_card);
                let picture = decoded(&mut decoder, &pictures).expect("pictures come out");
                assert_colours(&picture);
            }
        } else {
            eprintln!("this Windows has no H.264 decoder: OpenH264 stands in");
        }
        // Either way, a stream in H.264 is decoded, by Windows or OpenH264.
        let mut decoder =
            VideoDecoder::for_stream(Codec::H264, WIDTH as u32, HEIGHT as u32, 30, true)
                .expect("H.264 always has a decoder");
        assert_eq!(decoder.on_graphics_card(), found.hardware.contains(Codec::H264));
        let mut picture = Picture::default();
        let mut shown = false;
        for data in &pictures {
            shown |= decoder.decode(data, &mut picture).expect("a picture decodes");
        }
        assert!(shown);
        assert_colours(&picture);
    }

    #[test]
    fn a_codec_windows_does_not_decode_has_no_decoder() {
        let found = decodable();
        for codec in [Codec::H265, Codec::Av1] {
            let made = VideoDecoder::for_stream(codec, 1920, 1080, 60, true);
            assert_eq!(made.is_ok(), found.any.contains(codec), "{}", codec.name());
        }
    }
}
