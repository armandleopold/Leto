//! The phone's own video decoders, through `leto_system::decoder`: on its
//! video hardware, which decodes a picture of the whole screen in a few
//! milliseconds where OpenH264 on the processor falls behind.

use std::time::{Duration, Instant};

use leto_core::engine::{Codec, Codecs, Decodable};
use leto_system::decoder::{self as system, Layout, Picture};

use crate::{Error, Planes, convert::Nv12};

/// The name Android knows a codec by.
const fn mime(codec: Codec) -> &'static str {
    match codec {
        Codec::H264 => "video/avc",
        Codec::H265 => "video/hevc",
        Codec::Av1 => "video/av01",
    }
}

/// The codecs the phone decodes on its video hardware.
///
/// A television says it decodes AV1 on its hardware when it does not keep
/// up: a Fire TV Stick 4K Max froze on 1080p at 60 pictures a second with
/// `OMX.MS.AV1.Decoder`, which gives its pictures planar as a decoder on
/// the processor does. AV1 is then only what the person chooses by hand,
/// and the automatic codec is H.265 or H.264.
pub fn decodable() -> Decodable {
    let mut any = Codecs::NONE;
    for codec in [Codec::H264, Codec::H265, Codec::Av1] {
        if system::decodes_on_hardware(mime(codec)) {
            any = any.with(codec);
        }
    }
    let mut hardware = any;
    if leto_system::television::is_television() {
        hardware = Codecs::NONE;
        for codec in [Codec::H264, Codec::H265] {
            if any.contains(codec) {
                hardware = hardware.with(codec);
            }
        }
    }
    Decodable { hardware, any }
}

pub struct Decoder {
    decoder: system::Decoder,
    copying: Duration,
}

impl Decoder {
    pub fn new(codec: Codec, width: u32, height: u32, fps: u32) -> Result<Self, Error> {
        let decoder =
            system::Decoder::new(mime(codec), width, height, fps).map_err(Error::Video)?;
        log::info!("{} is decoded by {}", codec.name(), decoder.name());
        Ok(Self { decoder, copying: Duration::ZERO })
    }

    pub fn on_graphics_card(&self) -> bool {
        self.decoder.hardware()
    }

    pub const fn copying(&self) -> Duration {
        self.copying
    }

    pub fn decode(&mut self, data: &[u8], planes: Option<&mut Planes>) -> Result<bool, Error> {
        self.copying = Duration::ZERO;
        let Some(planes) = planes else {
            return self.decoder.decode(data, None).map_err(Error::Video);
        };
        let mut copied = true;
        let mut copying = Duration::ZERO;
        let mut copy = |picture: Picture<'_>| {
            let start = Instant::now();
            copied = match picture.layout {
                Layout::Nv12 => planes.copy_nv12(&Nv12 {
                    width: picture.width,
                    height: picture.height,
                    stride: picture.stride,
                    luma: picture.luma,
                    chroma: picture.u,
                }),
                Layout::I420 => planes.copy_i420(
                    (picture.width, picture.height),
                    [picture.luma, picture.u, picture.v],
                    [picture.stride, picture.chroma_stride, picture.chroma_stride],
                ),
            };
            copying = start.elapsed();
        };
        let came = self.decoder.decode(data, Some(&mut copy)).map_err(Error::Video)?;
        self.copying = copying;
        if came && !copied {
            return Err(Error::Video("a decoded picture is smaller than it says".into()));
        }
        Ok(came)
    }
}
