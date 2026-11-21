//! The camera of an Android phone, which `leto-system` reads.

use std::borrow::Cow;

use leto_system::camera;

use super::{
    Source,
    pixels::{Layout, Raw},
    turn::{self, Planes},
};
use crate::Error;

pub(super) struct Phone {
    camera: camera::Camera,
    picture: Vec<u8>,
}

impl Phone {
    pub(super) fn open(name: Option<&str>) -> Result<Self, Error> {
        let camera = camera::Camera::open(name).map_err(Error::Camera)?;
        Ok(Self { camera, picture: Vec::new() })
    }
}

impl Source for Phone {
    fn next(&mut self) -> Result<Raw<'_>, Error> {
        let frame = self.camera.picture().map_err(Error::Camera)?;
        let planes = Planes {
            width: frame.width,
            height: frame.height,
            luma: &frame.luma,
            luma_stride: frame.luma_stride,
            u: &frame.u,
            v: &frame.v,
            chroma_stride: frame.chroma_stride,
            chroma_step: frame.chroma_step,
        };
        let (width, height) = turn::nv12(&planes, frame.turn, &mut self.picture);
        Ok(Raw { layout: Layout::Nv12, width, height, data: Cow::Borrowed(&self.picture) })
    }
}

/// The names of the phone's cameras.
pub(super) fn names() -> Result<Vec<String>, Error> {
    camera::names().map_err(Error::Camera)
}
