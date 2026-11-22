//! The camera of this device on its way to a PC.
//!
//! No Sunshine or Apollo takes a camera, so GameStream has no word for one.
//! A PC that does says so on a small API of its own, thirteen ports above
//! the GameStream one, and agrees there on where the pictures go and on the
//! key that seals them: see `leto_return::api`. Both sides present the
//! certificates of the pairing there, as everywhere else: only the PC this
//! device knows is asked, and it only answers a device it knows.
//!
//! The pictures then leave as sealed packets, see `leto_return::camera`.
//! This module does not make them: whoever has a camera encodes its pictures
//! as H.264 and hands them to a [`CameraLink`].

use std::{
    fmt,
    net::{IpAddr, SocketAddr, UdpSocket},
    thread,
    time::Duration,
};

pub use leto_core::engine::CameraSettings;
use leto_core::{engine::CameraOffer, model::join_address};
use leto_protocol::ports::Service as Port;
use leto_return::{
    api,
    camera::{CLOCK, Packer},
    voice,
};
use moonlight_common::http::{ClientIdentifier, ClientSecret};

use crate::{Error, ProtocolError, parse_pem, transport::Transport};

/// How long a PC has to answer on Leto's API. One that has no such API may
/// let the question fall without a word, and whoever asked waits this long
/// to learn that the PC takes no camera.
const PATIENCE: Duration = Duration::from_secs(3);

/// How long a packet may wait for room on its way out. Pictures are worth
/// little a moment later: better to lose one than to fall behind.
const SEND_PATIENCE: Duration = Duration::from_millis(100);

/// Why a PC does not take the camera of this device.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CameraRefusal {
    /// It takes no camera at all: every Sunshine and Apollo.
    NotTaken,
    /// Its owner does not let this device send one.
    NotAllowed,
    /// Another device is sending its camera.
    Busy,
    /// It cannot take pictures of this size, or this many.
    Unsuitable,
}

impl fmt::Display for CameraRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::NotTaken => "it takes none",
            Self::NotAllowed => "its owner does not allow it",
            Self::Busy => "another device is sending its own",
            Self::Unsuitable => "it cannot take these pictures",
        })
    }
}

/// Leto's API on a PC this device is paired with.
pub(crate) struct LetoApi {
    transport: Transport,
    hostport: String,
}

impl LetoApi {
    /// The API of the PC at `address`, whose GameStream port is `port` and
    /// whose certificate from pairing is `certificate`.
    pub(crate) fn of(
        address: &str,
        port: u16,
        identifier: &ClientIdentifier,
        secret: &ClientSecret,
        certificate: &str,
    ) -> Result<Self, Error> {
        let port = u16::try_from(i32::from(port) + Port::LetoApi.offset())
            .map_err(|_| Error::Camera(CameraRefusal::NotTaken))?;
        let transport = Transport::paired(
            &secret.to_pem(),
            &identifier.to_pem(),
            &parse_pem(certificate)?,
            PATIENCE,
        )?;
        Ok(Self { transport, hostport: join_address(address, port) })
    }

    /// The camera the PC takes, when it takes one that this device can send.
    pub(crate) fn camera_offer(&self) -> Option<CameraOffer> {
        self.offer().camera
    }

    /// What the PC takes on this API.
    ///
    /// Whatever goes wrong means that it takes nothing: nothing listens
    /// there on a PC that runs Sunshine or Apollo, and something else might.
    pub(crate) fn offer(&self) -> ReturnOffer {
        self.asked().unwrap_or_default()
    }

    fn asked(&self) -> Option<ReturnOffer> {
        let reply = self
            .transport
            .request("GET", &self.hostport, api::INFO_PATH, None)
            .inspect_err(|error| log::debug!("{} has no Leto API: {error}", self.hostport))
            .ok()?;
        let info: api::Info = serde_json::from_slice(&reply.body)
            .inspect_err(|error| log::debug!("{} said something else: {error}", self.hostport))
            .ok()?;
        let camera = info.return_channel.camera.and_then(|camera| {
            let usable = camera.codecs.iter().any(|codec| codec == api::H264)
                && camera.max_width > 0
                && camera.max_height > 0
                && camera.max_fps > 0;
            usable.then_some(CameraOffer {
                max_width: camera.max_width,
                max_height: camera.max_height,
                max_fps: camera.max_fps,
            })
        });
        Some(ReturnOffer { microphone: info.return_channel.microphone, camera })
    }

    /// Asks the PC to take the microphone, and opens the way there.
    pub(crate) fn send_microphone(self) -> Result<VoiceLink, Error> {
        let ask = api::Ask { microphone: true, camera: None };
        let ask = serde_json::to_vec(&ask).map_err(|error| Error::Protocol(error.to_string()))?;
        let reply = self
            .transport
            .request("POST", &self.hostport, api::RETURN_CHANNEL_PATH, Some(&ask))
            .map_err(refusal)?;
        let answer: api::Answer = serde_json::from_slice(&reply.body)
            .map_err(|error| Error::Protocol(error.to_string()))?;
        let grant = answer.microphone.ok_or(Error::Camera(CameraRefusal::NotTaken))?;
        let key = grant
            .key()
            .ok_or_else(|| Error::Protocol("the key for the microphone is not a key".to_owned()))?;
        let socket = way(reply.peer.ip(), grant.port)?;
        Ok(VoiceLink { api: Some(self), key_id: key.id, socket, sealer: voice::Sealer::new(&key) })
    }

    /// Asks the PC to take the camera, and opens the way there.
    pub(crate) fn send_camera(self, sending: CameraSettings) -> Result<CameraLink, Error> {
        let ask = api::Ask {
            microphone: false,
            camera: Some(api::CameraAsk {
                codec: api::H264.to_owned(),
                width: sending.width,
                height: sending.height,
                fps: sending.fps,
                bitrate_kbps: sending.bitrate_kbps,
            }),
        };
        let ask = serde_json::to_vec(&ask).map_err(|error| Error::Protocol(error.to_string()))?;
        let reply = self
            .transport
            .request("POST", &self.hostport, api::RETURN_CHANNEL_PATH, Some(&ask))
            .map_err(refusal)?;
        let answer: api::Answer = serde_json::from_slice(&reply.body)
            .map_err(|error| Error::Protocol(error.to_string()))?;
        let grant = answer.camera.ok_or(Error::Camera(CameraRefusal::NotTaken))?;
        let key = grant
            .key()
            .ok_or_else(|| Error::Protocol("the key for the camera is not a key".to_owned()))?;

        let socket = way(reply.peer.ip(), grant.port)?;
        Ok(CameraLink { api: Some(self), key_id: key.id, socket, packer: Packer::new(&key) })
    }

    /// Says that this device stopped sending what the key of this number
    /// sealed. The PC leaves alone what it agreed to since.
    fn stop(&self, key_id: u32) {
        if let Err(error) =
            self.transport.request("DELETE", &self.hostport, &api::goodbye(key_id), None)
        {
            // The PC notices by itself when nothing arrives any more.
            log::debug!("{} was not told that the camera stopped: {error}", self.hostport);
        }
    }
}

/// What a PC takes from this device on Leto's API.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ReturnOffer {
    /// It takes the microphone here. A PC that takes it inside the stream
    /// does not say so here.
    pub microphone: bool,
    pub camera: Option<CameraOffer>,
}

/// A socket whose packets go to `port` at the address that answered, which
/// is the PC: a name may stand for several. They hardly wait.
fn way(to: IpAddr, port: u16) -> Result<UdpSocket, Error> {
    let to = SocketAddr::new(to, port);
    let from: SocketAddr =
        if to.is_ipv4() { ([0, 0, 0, 0], 0).into() } else { ([0u16; 8], 0).into() };
    let socket = UdpSocket::bind(from)?;
    socket.connect(to)?;
    socket.set_write_timeout(Some(SEND_PATIENCE))?;
    Ok(socket)
}

/// What a PC meant by not agreeing.
fn refusal(error: ProtocolError) -> Error {
    match error {
        ProtocolError::StatusCode { code: 403, .. } => Error::Camera(CameraRefusal::NotAllowed),
        ProtocolError::StatusCode { code: 409, .. } => Error::Camera(CameraRefusal::Busy),
        ProtocolError::StatusCode { code: 400, .. } => Error::Camera(CameraRefusal::Unsuitable),
        ProtocolError::StatusCode { code: 404 | 405, .. } => Error::Camera(CameraRefusal::NotTaken),
        other => Error::from(other),
    }
}

/// The way of this device's camera to a PC that agreed to take it. Dropping
/// it tells the PC that the camera stopped.
pub struct CameraLink {
    /// Gone once the PC was told that the camera stopped.
    api: Option<LetoApi>,
    /// The number of the key the PC made for this camera.
    key_id: u32,
    socket: UdpSocket,
    packer: Packer,
}

impl CameraLink {
    /// Sends one picture: an H.264 access unit with start codes, taken `at`
    /// this long after the first one. `key_frame` says that it decodes on
    /// its own. It travels sealed, and hardly waits.
    ///
    /// Returns whether all of it left. When not, the PC cannot use the
    /// pictures that follow either, until one that decodes on its own.
    pub fn send(&mut self, at: Duration, key_frame: bool, picture: &[u8]) -> bool {
        // The clock of the pictures wraps after half a day, and may.
        let ticks = at.as_micros() * u128::from(CLOCK) / 1_000_000;
        let mut left = true;
        let packed = self.packer.pack(ticks as u32, key_frame, picture, |packet| {
            left &= self.socket.send(packet).is_ok_and(|sent| sent == packet.len());
        });
        match packed {
            Ok(_) => left,
            Err(refused) => {
                log::debug!("a picture of the camera did not leave: {refused}");
                false
            }
        }
    }

    /// Tells the PC that the camera stopped, and waits until it knows.
    pub fn close(mut self) {
        if let Some(api) = self.api.take() {
            api.stop(self.key_id);
        }
    }
}

impl Drop for CameraLink {
    fn drop(&mut self) {
        let Some(api) = self.api.take() else { return };
        let key_id = self.key_id;
        // Telling takes a moment, and whoever drops this may be drawing.
        let telling = thread::Builder::new()
            .name("leto: camera off".to_owned())
            .spawn(move || api.stop(key_id));
        if let Err(error) = telling {
            log::debug!("the PC was not told that the camera stopped: {error}");
        }
    }
}

/// The way of this device's microphone to a PC that agreed to take it on
/// Leto's API. Dropping it tells the PC that the microphone stopped.
pub struct VoiceLink {
    /// Gone once the PC was told that the microphone stopped.
    api: Option<LetoApi>,
    /// The number of the key the PC made for this microphone.
    key_id: u32,
    socket: UdpSocket,
    sealer: voice::Sealer,
}

impl VoiceLink {
    /// Sends one Opus frame (one channel at 48 kHz, twenty milliseconds)
    /// captured `at` this long after the first one. Returns whether it left.
    pub fn send(&mut self, at: Duration, frame: &[u8]) -> bool {
        // The clock of the frames wraps after a day, and may.
        let ticks = at.as_micros() * u128::from(voice::CLOCK) / 1_000_000;
        let Some(packet) = self.sealer.seal(ticks as u32, frame) else { return false };
        self.socket.send(packet).is_ok_and(|sent| sent == packet.len())
    }
}

impl Drop for VoiceLink {
    fn drop(&mut self) {
        let Some(api) = self.api.take() else { return };
        let key_id = self.key_id;
        // Telling takes a moment, and whoever drops this may be drawing.
        let telling = thread::Builder::new()
            .name("leto: microphone off".to_owned())
            .spawn(move || api.stop(key_id));
        if let Err(error) = telling {
            log::debug!("the PC was not told that the microphone stopped: {error}");
        }
    }
}
