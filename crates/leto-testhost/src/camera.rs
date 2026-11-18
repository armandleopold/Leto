//! The camera of a client, taken the way Leto's own host will take it.
//!
//! No Sunshine or Apollo does this. The stand-in host answers Leto's API on
//! the port thirteen above its own, as `leto_return::api` describes it, then
//! takes the packets that follow, decodes them and keeps the last picture:
//! the test card shows it in a corner, and a test can look at it.

use std::{
    io::{self, Read, Write},
    net::{TcpListener, TcpStream, UdpSocket},
    sync::{
        Arc, Mutex, MutexGuard, PoisonError,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use leto_return::{
    api::{self, Answer, Ask, CameraGrant, CameraOffer, Info, Offer},
    camera::{Assembler, Key},
};
use openh264::{
    OpenH264API,
    decoder::{Decoder, DecoderConfig, Flush},
    formats::YUVSource,
};
use rustls::{ServerConnection, StreamOwned};
use serde::Serialize;

use crate::Shared;

/// How far above the host's own port Leto's API and the camera are.
const OFFSET: u16 = 13;

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

/// A picture of a client's camera: rows of pixels, four bytes each (red,
/// green, blue and an opacity that is always full).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Seen {
    pub width: usize,
    pub height: usize,
    pub rgba: Vec<u8>,
}

/// The cameras of the clients of one host: one at a time.
pub(crate) struct Cameras {
    port: u16,
    socket: UdpSocket,
    /// The largest picture the host says it takes.
    largest: (u32, u32),
    state: Mutex<State>,
}

#[derive(Default)]
struct State {
    /// The owner of the host does not let devices send their camera.
    refused_by_owner: bool,
    receiving: Option<Receiving>,
    /// Pictures that arrived whole and did not decode.
    undecoded: u64,
    report: CameraReport,
    seen: Option<Arc<Seen>>,
}

/// The camera that is being sent.
struct Receiving {
    /// The certificate of the device that sends it.
    device: Vec<u8>,
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
        let socket = UdpSocket::bind((interface, port))?;
        socket.set_read_timeout(Some(Duration::from_millis(50)))?;
        let cameras = Self { port, socket, largest, state: Mutex::default() };
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

    /// The owner of the host lets devices send their camera, or not.
    pub fn refuse(&self, refused: bool) {
        self.state().refused_by_owner = refused;
    }

    fn info(&self) -> Info {
        Info {
            api: api::VERSION,
            return_channel: Offer {
                microphone: false,
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
    fn ask(&self, device: &[u8], ask: &Ask) -> Result<Answer, u16> {
        let Some(camera) = &ask.camera else {
            // Nothing this host takes was asked for.
            return Ok(Answer::default());
        };
        let fits = camera.codec == api::H264
            && (1..=self.largest.0).contains(&camera.width)
            && (1..=self.largest.1).contains(&camera.height)
            && (1..=FASTEST).contains(&camera.fps);
        if !fits {
            return Err(400);
        }
        let mut state = self.state();
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

        let (mut bytes, mut id) = ([0u8; 16], [0u8; 4]);
        crate::stream::fill_random(&mut bytes);
        crate::stream::fill_random(&mut id);
        let key = Key { bytes, id: u32::from_le_bytes(id) };
        // A picture comes out as soon as it went in, and damage is not
        // hidden: what does not decode is counted.
        let config = DecoderConfig::new().flush_after_decode(Flush::NoFlush);
        let decoder =
            Decoder::with_api_config(OpenH264API::from_source(), config).map_err(|_| 500u16)?;
        state.receiving = Some(Receiving {
            device: device.to_vec(),
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
        Ok(Answer { camera: Some(CameraGrant::new(self.port, &key)) })
    }

    /// `device` says that it stopped sending.
    fn close(&self, device: &[u8]) {
        let mut state = self.state();
        if state.receiving.as_ref().is_some_and(|receiving| receiving.device == device) {
            state.receiving = None;
            state.seen = None;
            state.report.open = false;
        }
    }

    /// Takes the packets of the camera until `stop` is set.
    pub fn receive(&self, stop: &AtomicBool) {
        let mut packet = [0u8; 2048];
        while !stop.load(Ordering::SeqCst) {
            let Ok((length, _)) = self.socket.recv_from(&mut packet) else { continue };
            let mut state = self.state();
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

    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// One request to Leto's API.
struct Request {
    method: String,
    path: String,
    body: Vec<u8>,
}

/// Answers one connection to Leto's API.
pub(crate) fn handle(stream: TcpStream, shared: &Shared, cameras: &Cameras) {
    let config = Arc::clone(&shared.identity.lock().unwrap().tls);
    let Ok(connection) = ServerConnection::new(config) else { return };
    let mut stream = StreamOwned::new(connection, stream);
    // Reading the request runs the handshake first.
    let Some(request) = read_request(&mut stream) else { return };
    let device =
        stream.conn.peer_certificates().and_then(|chain| chain.first()).map(|c| c.to_vec());
    // Like the rest of the host, only for a device it is paired with.
    let device = device.filter(|device| shared.paired.lock().unwrap().contains(device));
    let (status, body) = match device {
        Some(device) => answer(cameras, &device, &request),
        None => (401, Vec::new()),
    };
    write_response(&mut stream, status, &body);
    stream.conn.send_close_notify();
    let _ = stream.flush();
}

fn answer(cameras: &Cameras, device: &[u8], request: &Request) -> (u16, Vec<u8>) {
    match (request.method.as_str(), request.path.as_str()) {
        ("GET", api::INFO_PATH) => (200, json(&cameras.info())),
        ("POST", api::RETURN_CHANNEL_PATH) => {
            let Ok(ask) = serde_json::from_slice::<Ask>(&request.body) else {
                return (400, Vec::new());
            };
            match cameras.ask(device, &ask) {
                Ok(answer) => (200, json(&answer)),
                Err(status) => (status, Vec::new()),
            }
        }
        ("DELETE", api::RETURN_CHANNEL_PATH) => {
            cameras.close(device);
            (204, Vec::new())
        }
        (_, api::INFO_PATH | api::RETURN_CHANNEL_PATH) => (405, Vec::new()),
        _ => (404, Vec::new()),
    }
}

fn json(value: &impl Serialize) -> Vec<u8> {
    serde_json::to_vec(value).expect("the messages of the API have no map keys to refuse")
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
    Some(Request { method: method.to_owned(), path: target.split('?').next()?.to_owned(), body })
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
            (request.method.as_str(), request.path.as_str()),
            ("POST", api::RETURN_CHANNEL_PATH)
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
        assert_ne!(again.camera.and_then(|grant| grant.key()), grant.key());

        // Only the device that sends can say that it stopped.
        cameras.close(other);
        assert!(cameras.report().open);
        cameras.close(one);
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
        let receiving = std::thread::spawn({
            let (cameras, stop) = (Arc::clone(&cameras), Arc::clone(&stop));
            move || cameras.receive(&stop)
        });
        let sender = UdpSocket::bind(("127.0.0.1", 0)).unwrap();
        let to = ("127.0.0.1", cameras.port());

        let mut stray = Packer::new(&Key { bytes: [7; 16], id: 7 });
        stray
            .pack(0, true, b"not for this host", |packet| drop(sender.send_to(packet, to)))
            .unwrap();
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(cameras.report(), CameraReport::default());

        // Once a camera is open, a packet under another key is refused.
        cameras.ask(b"one", &ask(640, 480, 30, api::H264)).expect("it fits");
        stray.pack(0, true, b"still not", |packet| drop(sender.send_to(packet, to))).unwrap();
        let start = Instant::now();
        while cameras.report().refused == 0 {
            assert!(start.elapsed() < Duration::from_secs(10), "the packet never arrived");
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(cameras.report().pictures, 0);

        stop.store(true, Ordering::SeqCst);
        receiving.join().unwrap();
    }
}
