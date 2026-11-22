//! The camera of a client, on the PC.
//!
//! No Sunshine or Apollo takes one. A Leto host answers Leto's API on the
//! port thirteen above its own, as `leto_return::api` describes it, then
//! takes the packets that follow, decodes them and keeps the last picture
//! for whoever shows it on the PC.
//!
//! A microphone can be asked for the same way, of a host that has someone
//! to hand it to: see [`Cameras::hear`]. Its packets come to the same port,
//! and its Opus frames leave as they are.

use std::{
    io::{self, Read, Write},
    net::{Ipv4Addr, Ipv6Addr, SocketAddr, TcpListener, TcpStream, UdpSocket},
    sync::{
        Arc, Condvar, Mutex, MutexGuard, PoisonError,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

use leto_return::{
    api::{self, Answer, Ask, CameraAsk, CameraOffer, Grant, Info, Offer},
    camera::{Assembler, Key},
    voice,
};
use openh264::{
    OpenH264API,
    decoder::{Decoder, DecoderConfig, Flush},
    formats::YUVSource,
};
use rustls::{HandshakeKind, ServerConfig, ServerConnection, StreamOwned};
use serde::Serialize;

/// How far above the host's own port Leto's API and the camera are.
pub const OFFSET: u16 = 13;

/// How long a device has to send its request once it connected.
const PATIENCE: Duration = Duration::from_secs(10);

/// The most pictures a second the host takes.
const FASTEST: u32 = 30;

/// The largest request Leto's API reads.
const REQUEST_LIMIT: usize = 64 * 1024;

/// A device that sent nothing for this long has stopped without saying so:
/// another one may take its place.
const SILENCE: Duration = Duration::from_secs(5);

/// What a test can observe of a client's camera: the one that is sending,
/// or the last one that did.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct CameraReport {
    /// A client asked to send its camera and has not said that it stopped.
    pub open: bool,
    /// What it said it would send: width, height and pictures a second.
    pub asked: Option<(u32, u32, u32)>,
    /// Everything that reached the port while a camera was open.
    pub packets: u64,
    /// What of it was not a packet of that camera.
    pub refused: u64,
    /// Pictures that arrived whole and decoded.
    pub pictures: u64,
    pub key_frames: u64,
    /// Pictures that lost a piece on the way, built on one that did, or
    /// did not decode.
    pub lost: u64,
    /// The size of the last picture.
    pub size: Option<(usize, usize)>,
}

/// What can be observed of a client's microphone: the one that is sending,
/// or the last one that did.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct VoiceReport {
    /// A client asked to send its microphone and has not said that it
    /// stopped.
    pub open: bool,
    /// Frames that arrived and were handed over.
    pub frames: u64,
    /// What looked like a packet of a microphone and was not one of this
    /// microphone: under another key, changed on the way, or late.
    pub refused: u64,
    /// When the last frame arrived.
    pub heard: Option<Instant>,
}

/// A picture of a client's camera: rows of pixels, four bytes each (red,
/// green, blue and an opacity that is always full).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Seen {
    pub width: usize,
    pub height: usize,
    pub rgba: Vec<u8>,
}

/// The cameras of the clients of one host: one at a time.
pub struct Cameras {
    port: u16,
    socket: UdpSocket,
    /// The largest picture the host says it takes.
    largest: (u32, u32),
    state: Mutex<State>,
    /// Woken when the host answers again after holding its answers.
    let_go: Condvar,
}

#[derive(Default)]
struct State {
    /// The owner of the host does not let devices send their camera.
    refused_by_owner: bool,
    /// The host keeps what it is asked unanswered for now.
    held: bool,
    receiving: Option<Receiving>,
    /// Pictures that arrived whole and did not decode.
    undecoded: u64,
    report: CameraReport,
    seen: Option<Arc<Seen>>,
    /// Who takes the Opus frames of a microphone. Without anyone, the host
    /// takes no microphone.
    listener: Option<mpsc::Sender<Vec<u8>>>,
    hearing: Option<Hearing>,
    voice: VoiceReport,
}

/// The microphone that is being sent.
struct Hearing {
    /// The certificate of the device that sends it.
    device: Vec<u8>,
    /// The number of the key that seals it.
    key_id: u32,
    opener: voice::Opener,
    /// When it was asked for, or last heard.
    heard: Instant,
}

/// The camera that is being sent.
struct Receiving {
    /// The certificate of the device that sends it.
    device: Vec<u8>,
    /// The number of the key that seals it.
    key_id: u32,
    assembler: Assembler,
    decoder: Decoder,
    /// When it was last heard of.
    heard: Instant,
}

impl Cameras {
    /// Opens Leto's API and the camera's port, both thirteen above
    /// `host_port`. Fails when either is taken.
    pub fn open(
        interface: &str,
        host_port: u16,
        largest: (u32, u32),
    ) -> io::Result<(Arc<Self>, TcpListener)> {
        let port = host_port
            .checked_add(OFFSET)
            .ok_or_else(|| io::Error::new(io::ErrorKind::AddrNotAvailable, "no port that high"))?;
        let listener = TcpListener::bind((interface, port))?;
        // With no limit on how long it waits for a packet: see `receive`.
        let socket = UdpSocket::bind((interface, port))?;
        let cameras =
            Self { port, socket, largest, state: Mutex::default(), let_go: Condvar::new() };
        Ok((Arc::new(cameras), listener))
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    pub fn report(&self) -> CameraReport {
        self.state().report
    }

    /// The last picture of the camera that is being sent.
    pub fn seen(&self) -> Option<Arc<Seen>> {
        self.state().seen.clone()
    }

    /// The certificate of the device whose camera is being sent.
    pub fn sender(&self) -> Option<Vec<u8>> {
        self.state().receiving.as_ref().map(|receiving| receiving.device.clone())
    }

    pub fn voice_report(&self) -> VoiceReport {
        self.state().voice
    }

    /// The certificate of the device whose microphone is being sent.
    pub fn speaker(&self) -> Option<Vec<u8>> {
        self.state().hearing.as_ref().map(|hearing| hearing.device.clone())
    }

    /// From now on the host takes a microphone, and hands its Opus frames
    /// (one channel at 48 kHz, twenty milliseconds each) to whoever holds
    /// what this returns. It stops taking one when that is dropped.
    pub fn hear(&self) -> mpsc::Receiver<Vec<u8>> {
        let (listener, frames) = mpsc::channel();
        self.state().listener = Some(listener);
        frames
    }

    /// The owner of the host lets devices send their camera, or not.
    pub fn refuse(&self, refused: bool) {
        self.state().refused_by_owner = refused;
    }

    /// While `held`, the host leaves a device that asks to send its camera
    /// without an answer: a PC that is slow, for as long as a test wants.
    pub fn hold(&self, held: bool) {
        self.state().held = held;
        self.let_go.notify_all();
    }

    fn info(&self) -> Info {
        Info {
            api: api::VERSION,
            return_channel: Offer {
                microphone: self.state().listener.is_some(),
                camera: Some(CameraOffer {
                    codecs: vec![api::H264.to_owned()],
                    max_width: self.largest.0,
                    max_height: self.largest.1,
                    max_fps: FASTEST,
                }),
            },
        }
    }

    /// What `device` asks to send, answered or refused with an HTTP status.
    ///
    /// The camera and the microphone are asked for apart: an ask that names
    /// one leaves the other as it is.
    fn ask(&self, device: &[u8], ask: &Ask) -> Result<Answer, u16> {
        let mut answer = Answer::default();
        if let Some(camera) = &ask.camera {
            answer.camera = Some(self.ask_camera(device, camera)?);
        }
        if ask.microphone {
            // `None` from a host that takes no microphone, which the device
            // knew before it asked.
            answer.microphone = self.ask_microphone(device)?;
        }
        Ok(answer)
    }

    fn ask_microphone(&self, device: &[u8]) -> Result<Option<Grant>, u16> {
        let mut state = self.state();
        if state.listener.is_none() {
            return Ok(None);
        }
        if state.refused_by_owner {
            return Err(403);
        }
        let taken = state
            .hearing
            .as_ref()
            .is_some_and(|other| other.device != device && other.heard.elapsed() < SILENCE);
        if taken {
            return Err(409);
        }
        let key = fresh_key()?;
        state.hearing = Some(Hearing {
            device: device.to_vec(),
            key_id: key.id,
            opener: voice::Opener::new(&key),
            heard: Instant::now(),
        });
        state.voice = VoiceReport { open: true, ..VoiceReport::default() };
        Ok(Some(Grant::new(self.port, &key)))
    }

    fn ask_camera(&self, device: &[u8], camera: &CameraAsk) -> Result<Grant, u16> {
        let fits = camera.codec == api::H264
            && (1..=self.largest.0).contains(&camera.width)
            && (1..=self.largest.1).contains(&camera.height)
            && (1..=FASTEST).contains(&camera.fps);
        if !fits {
            return Err(400);
        }
        let mut state = self.state();
        while state.held {
            state = self.let_go.wait(state).unwrap_or_else(PoisonError::into_inner);
        }
        if state.refused_by_owner {
            return Err(403);
        }
        let taken = state
            .receiving
            .as_ref()
            .is_some_and(|other| other.device != device && other.heard.elapsed() < SILENCE);
        if taken {
            return Err(409);
        }

        let key = fresh_key()?;
        // A picture comes out as soon as it went in, and damage is not
        // hidden: what does not decode is counted.
        let config = DecoderConfig::new().flush_after_decode(Flush::NoFlush);
        let decoder =
            Decoder::with_api_config(OpenH264API::from_source(), config).map_err(|_| 500u16)?;
        state.receiving = Some(Receiving {
            device: device.to_vec(),
            key_id: key.id,
            assembler: Assembler::new(&key),
            decoder,
            heard: Instant::now(),
        });
        state.undecoded = 0;
        state.seen = None;
        state.report = CameraReport {
            open: true,
            asked: Some((camera.width, camera.height, camera.fps)),
            ..CameraReport::default()
        };
        Ok(Grant::new(self.port, &key))
    }

    /// `device` says that it stopped sending what the key of this number
    /// sealed, or whatever it sends. What was agreed since stays.
    fn close(&self, device: &[u8], key_id: Option<u32>) {
        let mut state = self.state();
        let meant = state.receiving.as_ref().is_some_and(|receiving| {
            receiving.device == device && key_id.is_none_or(|id| id == receiving.key_id)
        });
        if meant {
            state.receiving = None;
            state.seen = None;
            state.report.open = false;
        }
        let meant = state.hearing.as_ref().is_some_and(|hearing| {
            hearing.device == device && key_id.is_none_or(|id| id == hearing.key_id)
        });
        if meant {
            state.hearing = None;
            state.voice.open = false;
        }
    }

    /// Takes the packets of the camera until `stop` is set and
    /// [`Self::wake`] was called.
    ///
    /// It waits for each packet as long as it takes. It used to wait fifty
    /// milliseconds at a time, to look at `stop` in between, and on Windows
    /// packets then went missing when the computer was busy: sent, and never
    /// seen here. On GitHub's runner that was two of the sixteen that
    /// `every_packet_is_taken_however_busy_the_computer_is` sends, and none
    /// on Linux. Microsoft says of a socket whose wait ran out of time that
    /// it should not be used any more, and of TCP that data may be lost
    /// because the wait can be cancelled just as it would have ended. That
    /// this is what happened to those packets is likely, and not proven.
    pub fn receive(&self, stop: &AtomicBool) {
        let mut packet = [0u8; 2048];
        loop {
            let received = self.socket.recv_from(&mut packet);
            if stop.load(Ordering::SeqCst) {
                return;
            }
            let Ok((length, _)) = received else {
                // Nothing a client sends does this. Not in a tight loop, in
                // case it lasts.
                thread::sleep(Duration::from_millis(1));
                continue;
            };
            let mut state = self.state();
            if voice::is_one(&packet[..length]) {
                state.heard(&packet[..length]);
                continue;
            }
            let State { receiving, undecoded, report, seen, .. } = &mut *state;
            // Nobody was told to send: whatever this is, it is not looked at.
            let Some(receiving) = receiving else { continue };
            let picture = receiving.assembler.push(&packet[..length]);
            let tally = receiving.assembler.tally();
            if tally.refused == report.refused {
                receiving.heard = Instant::now();
            }
            if let Some(picture) = picture {
                match receiving.decoder.decode(&picture.data) {
                    Ok(Some(decoded)) => {
                        let (width, height) = decoded.dimensions();
                        let mut rgba = vec![0u8; width * height * 4];
                        decoded.write_rgba8(&mut rgba);
                        report.pictures += 1;
                        report.key_frames += u64::from(picture.key_frame);
                        report.size = Some((width, height));
                        *seen = Some(Arc::new(Seen { width, height, rgba }));
                    }
                    Ok(None) => {}
                    Err(_) => *undecoded += 1,
                }
            }
            report.packets = tally.packets;
            report.refused = tally.refused;
            report.lost = tally.lost + *undecoded;
        }
    }

    /// Lets [`Self::receive`] see that `stop` was set, by sending it a
    /// packet of the host's own: for whoever set `stop`, after setting it.
    pub fn wake(&self) {
        let Ok(mut here) = self.socket.local_addr() else { return };
        if here.ip().is_unspecified() {
            // The port is open on every address of this computer, and this
            // is the one that always exists.
            here.set_ip(match here {
                SocketAddr::V4(_) => Ipv4Addr::LOCALHOST.into(),
                SocketAddr::V6(_) => Ipv6Addr::LOCALHOST.into(),
            });
        }
        let _ = self.socket.send_to(&[0], here);
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl State {
    /// Takes what says that it is a packet of a microphone.
    fn heard(&mut self, packet: &[u8]) {
        // Nobody was told to send: whatever this is, it is not looked at.
        let Some(hearing) = &mut self.hearing else { return };
        let Some(frame) = hearing.opener.open(packet) else {
            self.voice.refused += 1;
            return;
        };
        let now = Instant::now();
        hearing.heard = now;
        self.voice.frames += 1;
        self.voice.heard = Some(now);
        let taken =
            self.listener.as_ref().is_some_and(|listener| listener.send(frame.data).is_ok());
        if !taken {
            // Whoever listened is gone: so is the microphone.
            self.listener = None;
            self.hearing = None;
            self.voice.open = false;
        }
    }
}

/// A key nobody had before, from the system's random numbers.
fn fresh_key() -> Result<Key, u16> {
    let (mut bytes, mut id) = ([0u8; 16], [0u8; 4]);
    let random = ring::rand::SystemRandom::new();
    ring::rand::SecureRandom::fill(&random, &mut bytes).map_err(|_| 500u16)?;
    ring::rand::SecureRandom::fill(&random, &mut id).map_err(|_| 500u16)?;
    Ok(Key { bytes, id: u32::from_le_bytes(id) })
}

/// One request to Leto's API.
struct Request {
    method: String,
    path: String,
    /// What follows the question mark of the path, when one does.
    query: String,
    body: Vec<u8>,
}

/// What a host knows of the device at the other end of a connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Access {
    /// Not a device the host is paired with.
    Stranger,
    /// A paired device, which the owner of the PC lets send its camera and
    /// its microphone, or not.
    Paired { camera: bool },
}

/// Answers one connection to Leto's API.
///
/// `tls` asks the device for its certificate, and `access` says what the
/// host knows of the one it showed: the certificate is all that tells a
/// paired device from a stranger, as on the rest of a host.
pub fn handle(
    stream: TcpStream,
    tls: Arc<ServerConfig>,
    access: impl FnOnce(&[u8]) -> Access,
    cameras: &Cameras,
) {
    // A stranger that connects and says nothing does not keep a thread.
    let _ = stream.set_read_timeout(Some(PATIENCE));
    let _ = stream.set_write_timeout(Some(PATIENCE));
    let Ok(connection) = ServerConnection::new(tls) else { return };
    let mut stream = StreamOwned::new(connection, stream);
    // Reading the request runs the handshake first.
    let Some(request) = read_request(&mut stream) else { return };
    // Apollo and Sunshine answer a session that is taken up again with an
    // alert. A Leto host beside one of them keeps to the same rule, so that
    // a client never succeeds here with what fails next door.
    if stream.conn.handshake_kind() == Some(HandshakeKind::Resumed) {
        return;
    }
    let device =
        stream.conn.peer_certificates().and_then(|chain| chain.first()).map(|c| c.to_vec());
    let (status, body) = match device {
        Some(device) => match access(&device) {
            Access::Paired { camera } => answer(cameras, &device, camera, &request),
            Access::Stranger => (401, Vec::new()),
        },
        None => (401, Vec::new()),
    };
    write_response(&mut stream, status, &body);
    stream.conn.send_close_notify();
    let _ = stream.flush();
}

fn answer(cameras: &Cameras, device: &[u8], allowed: bool, request: &Request) -> (u16, Vec<u8>) {
    match (request.method.as_str(), request.path.as_str()) {
        ("GET", api::INFO_PATH) => (200, json(&cameras.info())),
        ("POST", api::RETURN_CHANNEL_PATH) => {
            let Ok(ask) = serde_json::from_slice::<Ask>(&request.body) else {
                return (400, Vec::new());
            };
            if (ask.camera.is_some() || ask.microphone) && !allowed {
                return (403, Vec::new());
            }
            match cameras.ask(device, &ask) {
                Ok(answer) => (200, json(&answer)),
                Err(status) => (status, Vec::new()),
            }
        }
        ("DELETE", api::RETURN_CHANNEL_PATH) => {
            cameras.close(device, api::goodbye_key(&request.query));
            (204, Vec::new())
        }
        (_, api::INFO_PATH | api::RETURN_CHANNEL_PATH) => (405, Vec::new()),
        _ => (404, Vec::new()),
    }
}

fn json(value: &impl Serialize) -> Vec<u8> {
    // The messages of the API have no map keys to refuse.
    serde_json::to_vec(value).unwrap_or_default()
}

/// Reads one request: its method, its path and its body.
fn read_request(stream: &mut impl Read) -> Option<Request> {
    let mut buffer = Vec::new();
    let mut chunk = [0u8; 4096];
    let mut read_more = |buffer: &mut Vec<u8>| {
        let read = stream.read(&mut chunk).ok().filter(|read| *read != 0)?;
        buffer.extend_from_slice(&chunk[..read]);
        (buffer.len() <= REQUEST_LIMIT).then_some(())
    };
    let head_end = loop {
        if let Some(at) = buffer.windows(4).position(|window| window == b"\r\n\r\n") {
            break at + 4;
        }
        read_more(&mut buffer)?;
    };
    let head = String::from_utf8_lossy(&buffer[..head_end]).into_owned();
    let mut lines = head.lines();
    let mut words = lines.next()?.split(' ');
    let (method, target) = (words.next()?, words.next()?);
    let length = lines
        .filter_map(|line| line.split_once(':'))
        .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
        .map_or(Some(0), |(_, value)| value.trim().parse::<usize>().ok())?;
    if length > REQUEST_LIMIT {
        return None;
    }
    let mut body = buffer.split_off(head_end);
    while body.len() < length {
        read_more(&mut body)?;
    }
    body.truncate(length);
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    Some(Request {
        method: method.to_owned(),
        path: path.to_owned(),
        query: query.to_owned(),
        body,
    })
}

fn write_response(stream: &mut impl Write, status: u16, body: &[u8]) {
    let reason = match status {
        200 => "OK",
        204 => "No Content",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        409 => "Conflict",
        _ => "Internal Server Error",
    };
    let head = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\
         Connection: close\r\n\r\n",
        body.len()
    );
    let _ = stream.write_all(head.as_bytes());
    let _ = stream.write_all(body);
    let _ = stream.flush();
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use leto_return::{api::CameraAsk, camera::Packer};

    use super::*;

    fn cameras() -> Arc<Cameras> {
        // Any two free ports thirteen apart.
        loop {
            let probe = TcpListener::bind(("127.0.0.1", 0)).unwrap();
            let port = probe.local_addr().unwrap().port();
            if let Ok((cameras, _listener)) = Cameras::open("127.0.0.1", port, (1280, 720)) {
                return cameras;
            }
        }
    }

    fn ask(width: u32, height: u32, fps: u32, codec: &str) -> Ask {
        Ask {
            microphone: false,
            camera: Some(CameraAsk {
                codec: codec.to_owned(),
                width,
                height,
                fps,
                bitrate_kbps: 2500,
            }),
        }
    }

    #[test]
    fn a_request_is_read_with_its_body_and_a_broken_one_is_not() {
        let text = b"POST /leto/v1/return-channel?x=1 HTTP/1.1\r\nHost: pc\r\n\
                     content-length: 4\r\n\r\n{\"a\"extra";
        let request = read_request(&mut &text[..]).expect("a whole request");
        assert_eq!(
            (request.method.as_str(), request.path.as_str(), request.query.as_str()),
            ("POST", api::RETURN_CHANNEL_PATH, "x=1")
        );
        assert_eq!(request.body, b"{\"a\"");

        let get = read_request(&mut &b"GET /leto/v1/info HTTP/1.1\r\n\r\n"[..]).expect("a request");
        assert_eq!(
            (get.method.as_str(), get.path.as_str(), get.body.len()),
            ("GET", api::INFO_PATH, 0)
        );

        // Cut short, a length that is no number, a body larger than allowed.
        assert!(read_request(&mut &b"GET /leto/v1/info HTTP/1.1\r\n"[..]).is_none());
        assert!(
            read_request(&mut &b"POST / HTTP/1.1\r\nContent-Length: 9\r\n\r\nabc"[..]).is_none()
        );
        assert!(read_request(&mut &b"POST / HTTP/1.1\r\nContent-Length: x\r\n\r\n"[..]).is_none());
        let huge = format!("POST / HTTP/1.1\r\nContent-Length: {}\r\n\r\n", REQUEST_LIMIT + 1);
        assert!(read_request(&mut huge.as_bytes()).is_none());
    }

    #[test]
    fn the_host_agrees_to_what_it_takes_and_says_no_to_the_rest() {
        let cameras = cameras();
        let (one, other) = (b"one device".as_slice(), b"another device".as_slice());

        // What it cannot take.
        assert_eq!(cameras.ask(one, &ask(1920, 1080, 30, api::H264)).err(), Some(400));
        assert_eq!(cameras.ask(one, &ask(1280, 720, 60, api::H264)).err(), Some(400));
        assert_eq!(cameras.ask(one, &ask(0, 720, 30, api::H264)).err(), Some(400));
        assert_eq!(cameras.ask(one, &ask(1280, 720, 30, "av1")).err(), Some(400));
        assert!(!cameras.report().open);

        // What its owner does not allow.
        cameras.refuse(true);
        assert_eq!(cameras.ask(one, &ask(1280, 720, 30, api::H264)).err(), Some(403));
        cameras.refuse(false);

        let granted = cameras.ask(one, &ask(1280, 720, 30, api::H264)).expect("it fits");
        let grant = granted.camera.expect("a camera was asked for");
        assert_eq!(grant.port, cameras.port());
        assert!(grant.key().is_some());
        assert_eq!(cameras.report().asked, Some((1280, 720, 30)));
        assert!(cameras.report().open);

        // One device at a time, and each ask draws a new key.
        assert_eq!(cameras.ask(other, &ask(640, 480, 30, api::H264)).err(), Some(409));
        let again = cameras.ask(one, &ask(640, 480, 30, api::H264)).expect("the same device");
        let again = again.camera.expect("a camera was asked for");
        assert_ne!(again.key(), grant.key());

        // Only the device that sends can say that it stopped, and a goodbye
        // that comes late does not end what was agreed after it.
        cameras.close(other, None);
        assert!(cameras.report().open);
        cameras.close(one, Some(grant.key_id));
        assert!(cameras.report().open);
        cameras.close(one, Some(again.key_id));
        assert!(!cameras.report().open);
        assert!(cameras.ask(other, &ask(640, 480, 30, api::H264)).is_ok());
    }

    #[test]
    fn a_device_that_went_silent_gives_its_place_to_the_next() {
        let cameras = cameras();
        cameras.ask(b"one", &ask(640, 480, 30, api::H264)).expect("it fits");
        cameras.state().receiving.as_mut().unwrap().heard = Instant::now() - SILENCE;
        assert!(cameras.ask(b"another", &ask(640, 480, 30, api::H264)).is_ok());
    }

    #[test]
    fn what_arrives_before_anyone_was_told_to_send_is_not_looked_at() {
        let cameras = cameras();
        let stop = Arc::new(AtomicBool::new(false));
        let receiving = thread::spawn({
            let (cameras, stop) = (Arc::clone(&cameras), Arc::clone(&stop));
            move || cameras.receive(&stop)
        });
        let sender = UdpSocket::bind(("127.0.0.1", 0)).unwrap();
        let to = ("127.0.0.1", cameras.port());

        let mut stray = Packer::new(&Key { bytes: [7; 16], id: 7 });
        stray
            .pack(0, true, b"not for this host", |packet| drop(sender.send_to(packet, to)))
            .unwrap();
        thread::sleep(Duration::from_millis(100));
        assert_eq!(cameras.report(), CameraReport::default());

        // Once a camera is open, a packet under another key is refused.
        cameras.ask(b"one", &ask(640, 480, 30, api::H264)).expect("it fits");
        stray.pack(0, true, b"still not", |packet| drop(sender.send_to(packet, to))).unwrap();
        let start = Instant::now();
        while cameras.report().refused == 0 {
            assert!(start.elapsed() < Duration::from_secs(10), "the packet never arrived");
            thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(cameras.report().pictures, 0);

        stop.store(true, Ordering::SeqCst);
        cameras.wake();
        receiving.join().unwrap();
    }

    #[test]
    fn every_packet_is_taken_however_busy_the_computer_is() {
        let cameras = cameras();
        let stop = Arc::new(AtomicBool::new(false));
        let receiving = thread::spawn({
            let (cameras, stop) = (Arc::clone(&cameras), Arc::clone(&stop));
            move || cameras.receive(&stop)
        });
        let granted = cameras.ask(b"one", &ask(640, 480, 30, api::H264)).expect("it fits");
        let key = granted.camera.and_then(|grant| grant.key()).expect("a key for the camera");
        let mut packer = Packer::new(&key);
        let sender = UdpSocket::bind(("127.0.0.1", 0)).unwrap();
        let to = ("127.0.0.1", cameras.port());

        // As many threads with work to do as there are processors: whoever
        // else needs one waits for it, the thread that takes the packets too.
        let rest = Arc::new(AtomicBool::new(false));
        let processors = thread::available_parallelism().map_or(2, usize::from);
        let busy: Vec<_> = (0..processors)
            .map(|_| {
                let rest = Arc::clone(&rest);
                thread::spawn(move || {
                    while !rest.load(Ordering::Relaxed) {
                        std::hint::spin_loop();
                    }
                })
            })
            .collect();

        // One packet at a time, each after the port was quiet for a while:
        // none has a neighbour that would make up for it.
        const PACKETS: u32 = 16;
        for number in 0..PACKETS {
            thread::sleep(Duration::from_millis(60));
            packer
                .pack(number * 3000, true, b"one picture", |packet| {
                    assert_eq!(sender.send_to(packet, to).ok(), Some(packet.len()));
                })
                .unwrap();
        }
        let start = Instant::now();
        while cameras.report().packets < u64::from(PACKETS)
            && start.elapsed() < Duration::from_secs(5)
        {
            thread::sleep(Duration::from_millis(5));
        }
        rest.store(true, Ordering::Relaxed);
        for thread in busy {
            thread.join().unwrap();
        }
        let report = cameras.report();
        assert_eq!(
            (report.packets, report.refused),
            (u64::from(PACKETS), 0),
            "packets that were sent one by one were not all taken"
        );

        stop.store(true, Ordering::SeqCst);
        cameras.wake();
        receiving.join().unwrap();
    }
}
