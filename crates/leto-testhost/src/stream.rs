//! The streaming half of the stand-in host.
//!
//! What a Sunshine or Apollo host does once an app is launched: the RTSP
//! handshake on a TCP port, then three UDP services for control, video and
//! audio. The picture is a test card and the sound a short melody, both
//! encoded for real (H.264 and Opus), so a client decodes them exactly as it
//! would decode a game.
//!
//! The handshake follows `rtsp.cpp` of Sunshine and Apollo, read at Apollo
//! commit a40b179. The packets themselves are built by moonlight-common's own
//! payloaders and control host.

use std::{
    collections::{HashMap, VecDeque},
    io::{self, Read, Write},
    net::{IpAddr, SocketAddr, TcpListener, TcpStream, UdpSocket},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use leto_core::{
    engine::{Codec, Codecs},
    input::{GamepadButtons, GamepadKind, GamepadState, Input, Key, Modifiers, MouseButton},
};
use moonlight_common::{
    ServerVersion,
    crypto::rustcrypto::RustCryptoBackend,
    stream::{
        AesIv, AesKey,
        control::{
            ActiveGamepads, ControllerButtons, ControllerType, KeyAction, KeyModifiers,
            MouseButton as ProtocolMouseButton, MouseButtonAction,
        },
        proto::{
            Instant as ProtocolInstant,
            audio::payloader::{AudioPayloader, AudioPayloaderConfig},
            control::{
                packet::{ControlPacket, ControlPacketConfig, EnetChannel, TerminationReason},
                peer::{
                    ControlEncryptionMethod, ControlHost, ControlHostConfig, ControlHostEvent,
                    ControlPeerConfig, ControlPeerId, ControlPeerRole,
                },
            },
            crypto::CryptoBackend,
            runtime::{Receive, UdpStream},
            video::payloader::{VideoPayloader, VideoPayloaderConfig, VideoPayloaderFecConfig},
        },
        video::ServerFrameType,
    },
};

use opus_decoder::OpusDecoder;

use crate::{
    camera::Cameras,
    media::{Hands, Melody, TestCard},
};

/// The version the stand-in host reports, the one of a current Sunshine.
pub(crate) const APP_VERSION: &str = "7.1.431.-1";

/// Encryption the host can do, as bits of `x-ss-general.encryptionSupported`.
const ENCRYPT_CONTROL: u32 = 0x01;
const ENCRYPT_VIDEO: u32 = 0x02;
const ENCRYPT_AUDIO: u32 = 0x04;
/// An extension: the microphone of the client, towards the host.
const ENCRYPT_MICROPHONE: u32 = 0x08;

/// The microphone extension, as the Apollo microphone fork and Foundation
/// Sunshine speak it: the client sends one Opus frame per packet, one
/// channel at 48 kHz, behind this header.
const MICROPHONE_HEADER: usize = 12;
const MICROPHONE_PACKET: u8 = 0x61;
const MICROPHONE_MAGIC: u32 = 0x1234_5678;
const MICROPHONE_RATE: u32 = 48_000;
/// The longest frame Opus allows, in samples.
const MICROPHONE_FRAME: usize = 5760;
/// How much of what the microphone sent is kept to listen to: ten seconds.
const HEARD_LIMIT: usize = MICROPHONE_RATE as usize * 10;

/// Size of the header in front of an encrypted RTSP message: type and length,
/// sequence number, authentication tag.
const RTSP_HEADER: usize = 4 + 4 + 16;
const RTSP_ENCRYPTED: u32 = 0x8000_0000;
/// Largest RTSP message accepted from a client.
const RTSP_LIMIT: usize = 64 * 1024;

/// Sound is sent in packets of this length, as Sunshine does.
const AUDIO_PACKET: Duration = Duration::from_millis(5);
const AUDIO_RATE: u32 = 48_000;
/// Bit rate of Sunshine's stereo stream. With packets of 5 ms it makes every
/// packet 60 bytes long, and the audio packet format needs that fixed size.
const AUDIO_BITRATE: i32 = 96_000;

/// What `/launch` or `/resume` asked for.
pub(crate) struct Launch {
    pub key: AesKey,
    pub key_id: AesIv,
    /// Answer RTSP encrypted, as a host does for a client that sent `corever=1`.
    pub encrypt_rtsp: bool,
    /// Take the microphone of a client that offers it.
    pub microphone: bool,
    /// Where the camera of a client arrives, when the host takes one: the
    /// test card shows what it sees.
    pub camera: Option<Arc<Cameras>>,
    /// The codecs the host says it encodes.
    pub encodes: Codecs,
}

/// How long the host may take to say that a client left
/// ([`StreamReport::client_left`]) when the client went away without a word,
/// which is how a client built on the protocol library leaves.
///
/// The host is told nothing: it gives up on a client that stopped answering.
/// It looks at that each time its wait for an answer runs out, and doubles
/// the wait each time. It gives up the first time it looks after thirty
/// seconds, or after five once it has asked six times: the defaults of ENet,
/// in `rusty_enet` 0.4.1. The first wait is as long as the host thinks an
/// answer takes. With a client on the same computer that makes five to ten
/// seconds. A host that was held up for a moment as it met its client thinks
/// answers are slow: held up for 150 milliseconds, it was seen to take 29
/// seconds, and by the rule above it can take up to a minute.
pub const CLIENT_MISSED_WITHIN: Duration = Duration::from_secs(65);

/// What a test can observe of a stream.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct StreamReport {
    /// The client finished the handshake and opened the control connection.
    pub connected: bool,
    /// The client said that it leaves, or stopped answering for so long that
    /// the host gave up on it: see [`CLIENT_MISSED_WITHIN`].
    pub client_left: bool,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub video_encrypted: bool,
    pub audio_encrypted: bool,
    pub control_encrypted: bool,
    pub frames_sent: u64,
    pub keyframes_sent: u64,
    /// Keyframes the client asked for after the first one.
    pub keyframes_requested: u64,
    pub audio_packets_sent: u64,
    /// The client set up the microphone stream, encrypted: the host listens.
    pub microphone: bool,
    /// Packets of the client's microphone that arrived and decoded.
    pub microphone_packets: u64,
    /// The codec the client asked for. The pictures are H.264 whatever it
    /// is.
    pub codec: Codec,
}

/// What the client settled in its `ANNOUNCE`.
#[derive(Debug, Clone, Copy)]
struct Negotiated {
    width: u32,
    height: u32,
    fps: u32,
    /// Size of the video payload of a packet, before the headers.
    packet_size: usize,
    bitrate_kbps: u32,
    min_fec_packets: usize,
    encryption: u32,
    codec: Codec,
}

struct Inner {
    launch: Launch,
    version: ServerVersion,
    video: UdpSocket,
    audio: UdpSocket,
    control: UdpSocket,
    microphone: UdpSocket,
    /// What the client must send on the video and audio ports to be found.
    ping: [u8; 16],
    /// What the client must present when it opens the control connection.
    connect_data: u32,
    rtsp_sequence: AtomicU32,
    stop: AtomicBool,
    /// The host itself ends the stream: tell the client before leaving.
    terminate: AtomicBool,
    wants_keyframe: AtomicBool,
    negotiated: Mutex<Option<Negotiated>>,
    media: Mutex<Vec<thread::JoinHandle<()>>>,
    connected: AtomicBool,
    client_left: AtomicBool,
    frames_sent: AtomicU64,
    keyframes_sent: AtomicU64,
    keyframes_requested: AtomicU64,
    audio_packets_sent: AtomicU64,
    /// The client asked for the microphone stream with a `SETUP`.
    microphone_asked: AtomicBool,
    microphone_packets: AtomicU64,
    /// What the client's microphone sent, decoded: one channel at 48 kHz.
    heard: Mutex<VecDeque<f32>>,
    /// Everything the client did with keyboard, mouse and gamepads, in order.
    inputs: Mutex<Vec<Input>>,
    /// What of it the test card shows.
    hands: Mutex<Hands>,
}

/// One stream to one client, from `/launch` until either side leaves.
pub(crate) struct Session {
    inner: Arc<Inner>,
    rtsp_port: u16,
    rtsp: Option<thread::JoinHandle<()>>,
}

impl Session {
    /// Opens the ports of a new stream on `interface`.
    pub fn start(interface: IpAddr, launch: Launch) -> io::Result<Self> {
        let listener = TcpListener::bind((interface, 0))?;
        listener.set_nonblocking(true)?;
        let rtsp_port = listener.local_addr()?.port();

        let mut random = [0u8; 12];
        fill_random(&mut random);
        // Sixteen printable characters, as Sunshine sends them in a header.
        let ping = hex(&random[..8]);
        let mut ping_bytes = [0u8; 16];
        ping_bytes.copy_from_slice(ping.as_bytes());

        let inner = Arc::new(Inner {
            launch,
            version: APP_VERSION.parse().expect("the stand-in host's version is well formed"),
            video: UdpSocket::bind((interface, 0))?,
            audio: UdpSocket::bind((interface, 0))?,
            control: UdpSocket::bind((interface, 0))?,
            microphone: UdpSocket::bind((interface, 0))?,
            ping: ping_bytes,
            connect_data: u32::from_le_bytes([random[8], random[9], random[10], random[11]]) | 1,
            rtsp_sequence: AtomicU32::new(0),
            stop: AtomicBool::new(false),
            terminate: AtomicBool::new(false),
            wants_keyframe: AtomicBool::new(false),
            negotiated: Mutex::new(None),
            media: Mutex::new(Vec::new()),
            connected: AtomicBool::new(false),
            client_left: AtomicBool::new(false),
            frames_sent: AtomicU64::new(0),
            keyframes_sent: AtomicU64::new(0),
            keyframes_requested: AtomicU64::new(0),
            audio_packets_sent: AtomicU64::new(0),
            microphone_asked: AtomicBool::new(false),
            microphone_packets: AtomicU64::new(0),
            heard: Mutex::new(VecDeque::new()),
            inputs: Mutex::new(Vec::new()),
            hands: Mutex::new(Hands::default()),
        });

        let rtsp = thread::spawn({
            let inner = Arc::clone(&inner);
            move || serve_rtsp(&inner, &listener)
        });
        Ok(Self { inner, rtsp_port, rtsp: Some(rtsp) })
    }

    /// The address a client finishes the handshake on, as `/launch` reports it.
    pub fn url(&self, address: IpAddr) -> String {
        let scheme = if self.inner.launch.encrypt_rtsp { "rtspenc" } else { "rtsp" };
        format!("{scheme}://{}", SocketAddr::new(address, self.rtsp_port))
    }

    /// Whether a client is, or may still become, connected.
    pub fn alive(&self) -> bool {
        !self.inner.client_left.load(Ordering::SeqCst) && !self.inner.stop.load(Ordering::SeqCst)
    }

    pub fn report(&self) -> StreamReport {
        let inner = &self.inner;
        let negotiated = *lock(&inner.negotiated);
        let encryption = negotiated.map_or(0, |negotiated| negotiated.encryption);
        StreamReport {
            connected: inner.connected.load(Ordering::SeqCst),
            client_left: inner.client_left.load(Ordering::SeqCst),
            width: negotiated.map_or(0, |negotiated| negotiated.width),
            height: negotiated.map_or(0, |negotiated| negotiated.height),
            fps: negotiated.map_or(0, |negotiated| negotiated.fps),
            video_encrypted: encryption & ENCRYPT_VIDEO != 0,
            audio_encrypted: encryption & ENCRYPT_AUDIO != 0,
            control_encrypted: encryption & ENCRYPT_CONTROL != 0,
            frames_sent: inner.frames_sent.load(Ordering::SeqCst),
            keyframes_sent: inner.keyframes_sent.load(Ordering::SeqCst),
            keyframes_requested: inner.keyframes_requested.load(Ordering::SeqCst),
            audio_packets_sent: inner.audio_packets_sent.load(Ordering::SeqCst),
            microphone: inner.listens(encryption),
            microphone_packets: inner.microphone_packets.load(Ordering::SeqCst),
            codec: negotiated.map_or(Codec::H264, |negotiated| negotiated.codec),
        }
    }

    /// What the client's microphone sent lately, decoded: one channel at
    /// 48 kHz, the last ten seconds at most.
    pub fn heard(&self) -> Vec<f32> {
        lock(&self.inner.heard).iter().copied().collect()
    }

    /// What the client did with keyboard, mouse and gamepads so far.
    pub fn inputs(&self) -> Vec<Input> {
        lock(&self.inner.inputs).clone()
    }

    /// Ends the stream. With `tell_client`, the host says goodbye first, as it
    /// does when the app closes or its owner stops the stream.
    pub fn stop(&mut self, tell_client: bool) {
        self.inner.terminate.store(tell_client, Ordering::SeqCst);
        self.inner.stop.store(true, Ordering::SeqCst);
        if let Some(rtsp) = self.rtsp.take() {
            let _ = rtsp.join();
        }
        let media: Vec<_> = lock(&self.inner.media).drain(..).collect();
        for thread in media {
            let _ = thread.join();
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.stop(false);
    }
}

// ---------------------------------------------------------------------------
// RTSP
// ---------------------------------------------------------------------------

impl Inner {
    /// Whether the host takes the microphone of this client, which settled
    /// on `encryption`. Like the Apollo microphone fork, it refuses one that
    /// would travel in clear.
    fn listens(&self, encryption: u32) -> bool {
        self.launch.microphone
            && self.microphone_asked.load(Ordering::SeqCst)
            && encryption & ENCRYPT_MICROPHONE != 0
    }
}

/// One RTSP request, decrypted and split.
struct RtspRequest {
    method: String,
    target: String,
    sequence: String,
    payload: String,
}

/// Answers RTSP requests until the session stops. Like Sunshine, one request
/// per connection: the client reads the answer until the connection closes.
fn serve_rtsp(inner: &Arc<Inner>, listener: &TcpListener) {
    while !inner.stop.load(Ordering::SeqCst) {
        match listener.accept() {
            Ok((mut stream, _)) => {
                let _ = stream.set_nonblocking(false);
                let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
                let _ = stream.set_nodelay(true);
                if let Some(request) = read_rtsp(inner, &mut stream) {
                    let response = answer_rtsp(inner, &request);
                    write_rtsp(inner, &mut stream, &response);
                }
                let _ = stream.shutdown(std::net::Shutdown::Both);
            }
            Err(_) => thread::sleep(Duration::from_millis(2)),
        }
    }
}

fn read_rtsp(inner: &Inner, stream: &mut TcpStream) -> Option<RtspRequest> {
    let text = if inner.launch.encrypt_rtsp {
        let mut header = [0u8; RTSP_HEADER];
        stream.read_exact(&mut header).ok()?;
        let type_and_length = u32::from_be_bytes([header[0], header[1], header[2], header[3]]);
        let sequence = u32::from_be_bytes([header[4], header[5], header[6], header[7]]);
        let length = (type_and_length & !RTSP_ENCRYPTED) as usize;
        if type_and_length & RTSP_ENCRYPTED == 0 || length > RTSP_LIMIT {
            return None;
        }
        let mut encrypted = vec![0u8; length];
        stream.read_exact(&mut encrypted).ok()?;
        let mut plain = vec![0u8; length];
        RustCryptoBackend
            .decrypt_aes_gcm(
                &inner.launch.key,
                &rtsp_iv(sequence, b'C'),
                &encrypted,
                &header[8..24],
                &mut plain,
            )
            .ok()?;
        String::from_utf8(plain).ok()?
    } else {
        let mut buffer = Vec::new();
        let mut chunk = [0u8; 4096];
        let head_end = loop {
            if let Some(position) = buffer.windows(4).position(|window| window == b"\r\n\r\n") {
                break position + 4;
            }
            let read = stream.read(&mut chunk).ok()?;
            if read == 0 || buffer.len() > RTSP_LIMIT {
                return None;
            }
            buffer.extend_from_slice(&chunk[..read]);
        };
        let head = String::from_utf8_lossy(&buffer[..head_end]).into_owned();
        let length = header_of(&head, "Content-length")
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(0)
            .min(RTSP_LIMIT);
        while buffer.len() < head_end + length {
            let read = stream.read(&mut chunk).ok()?;
            if read == 0 {
                return None;
            }
            buffer.extend_from_slice(&chunk[..read]);
        }
        String::from_utf8(buffer).ok()?
    };

    let (head, payload) = text.split_once("\r\n\r\n").unwrap_or((text.as_str(), ""));
    let mut words = head.lines().next()?.split(' ');
    Some(RtspRequest {
        method: words.next()?.to_owned(),
        target: words.next()?.to_owned(),
        sequence: header_of(head, "CSeq")?.to_owned(),
        payload: payload.to_owned(),
    })
}

/// The value of header `name` in the head of an RTSP message.
fn header_of<'a>(head: &'a str, name: &str) -> Option<&'a str> {
    head.lines().skip(1).find_map(|line| {
        let (key, value) = line.split_once(':')?;
        key.trim().eq_ignore_ascii_case(name).then_some(value.trim())
    })
}

/// The answer to one request: status line, extra headers, payload.
struct RtspResponse {
    status: &'static str,
    sequence: String,
    headers: Vec<(&'static str, String)>,
    payload: String,
}

fn answer_rtsp(inner: &Arc<Inner>, request: &RtspRequest) -> RtspResponse {
    let mut response = RtspResponse {
        status: "200 OK",
        sequence: request.sequence.clone(),
        headers: Vec::new(),
        payload: String::new(),
    };

    match request.method.as_str() {
        "OPTIONS" | "PLAY" => {}
        "DESCRIBE" => {
            let mut supported = ENCRYPT_CONTROL | ENCRYPT_VIDEO | ENCRYPT_AUDIO;
            let mut requested = ENCRYPT_CONTROL;
            let mut microphone = String::new();
            if inner.launch.microphone {
                // The way the Apollo microphone fork says it takes one.
                supported |= ENCRYPT_MICROPHONE;
                requested |= ENCRYPT_MICROPHONE;
                let port = inner.microphone.local_addr().map_or(0, |address| address.port());
                microphone = format!(
                    "m=audio {port} RTP/AVP 96\n\
                     a=rtpmap:96 opus/48000/1\n\
                     a=fmtp:96 minptime=10;useinbandfec=1\n"
                );
            }
            // The codecs beyond H.264, written as Sunshine writes them: H.265
            // as the start of a parameter set, on a line of its own.
            let mut codecs = String::new();
            if inner.launch.encodes.contains(Codec::H265) {
                codecs.push_str("sprop-parameter-sets=AAAAAU\n");
            }
            if inner.launch.encodes.contains(Codec::Av1) {
                codecs.push_str("a=rtpmap:98 AV1/90000\n");
            }
            // Stereo twice (normal and high quality), then the surround
            // layouts, written the way Sunshine writes them.
            response.payload = format!(
                "a=x-ss-general.featureFlags:0\n\
                 a=x-ss-general.encryptionSupported:{supported}\n\
                 a=x-ss-general.encryptionRequested:{requested}\n\
                 {codecs}\
                 {microphone}\
                 a=fmtp:97 surround-params=21101\n\
                 a=fmtp:97 surround-params=21101\n\
                 a=fmtp:97 surround-params=642041235\n\
                 a=fmtp:97 surround-params=660012345\n\
                 a=fmtp:97 surround-params=85304123567\n\
                 a=fmtp:97 surround-params=88001234567\n"
            );
        }
        "SETUP" => {
            // The target looks like `streamid=video/0/0`.
            let kind = request
                .target
                .split_once('=')
                .map_or("", |(_, rest)| rest.split('/').next().unwrap_or(""));
            let (socket, extra) = match kind {
                "audio" => (&inner.audio, ("X-SS-Ping-Payload", ascii(&inner.ping))),
                "video" => (&inner.video, ("X-SS-Ping-Payload", ascii(&inner.ping))),
                "control" => {
                    (&inner.control, ("X-SS-Connect-Data", inner.connect_data.to_string()))
                }
                "mic" if inner.launch.microphone => {
                    inner.microphone_asked.store(true, Ordering::SeqCst);
                    (&inner.microphone, ("X-SS-Ping-Payload", ascii(&inner.ping)))
                }
                _ => {
                    response.status = "404 NOT FOUND";
                    return response;
                }
            };
            let port = socket.local_addr().map_or(0, |address| address.port());
            response.headers.push(("Session", "DEADBEEFCAFE;timeout = 90".to_owned()));
            response.headers.push(("Transport", format!("server_port={port}")));
            response.headers.push(extra);
        }
        "ANNOUNCE" => match parse_announce(&request.payload) {
            Some(negotiated) => start_media(inner, negotiated),
            None => response.status = "400 BAD REQUEST",
        },
        _ => response.status = "404 NOT FOUND",
    }
    response
}

/// Reads the settings a client announces. Returns `None` when one the stream
/// cannot start without is missing.
fn parse_announce(payload: &str) -> Option<Negotiated> {
    let attributes: HashMap<&str, &str> = payload
        .lines()
        .filter_map(|line| line.trim().strip_prefix("a=")?.split_once(':'))
        .map(|(key, value)| (key, value.trim()))
        .collect();
    let number = |key: &str| attributes.get(key).and_then(|value| value.parse::<u32>().ok());

    Some(Negotiated {
        width: number("x-nv-video[0].clientViewportWd")?,
        height: number("x-nv-video[0].clientViewportHt")?,
        fps: number("x-nv-video[0].maxFPS")?.clamp(1, 240),
        packet_size: number("x-nv-video[0].packetSize")? as usize,
        bitrate_kbps: number("x-ml-video.configuredBitrateKbps")
            .or_else(|| number("x-nv-vqos[0].bw.maximumBitrateKbps"))
            .unwrap_or(10_000),
        min_fec_packets: number("x-nv-vqos[0].fec.minRequiredFecPackets").unwrap_or(2) as usize,
        encryption: number("x-ss-general.encryptionEnabled").unwrap_or(0),
        // Written for H.265 and AV1 only.
        codec: match number("x-nv-vqos[0].bitStreamFormat") {
            Some(1) => Codec::H265,
            Some(2) => Codec::Av1,
            _ => Codec::H264,
        },
    })
}

fn write_rtsp(inner: &Inner, stream: &mut TcpStream, response: &RtspResponse) {
    let mut message = format!("RTSP/1.0 {}\r\nCSeq: {}\r\n", response.status, response.sequence);
    for (name, value) in &response.headers {
        message.push_str(&format!("{name}: {value}\r\n"));
    }
    message.push_str("\r\n");
    message.push_str(&response.payload);

    let bytes = if inner.launch.encrypt_rtsp {
        let sequence = inner.rtsp_sequence.fetch_add(1, Ordering::SeqCst);
        let mut encrypted = vec![0u8; message.len()];
        let mut tag = [0u8; 16];
        if RustCryptoBackend
            .encrypt_aes_gcm(
                &inner.launch.key,
                &rtsp_iv(sequence, b'H'),
                message.as_bytes(),
                &mut encrypted,
                &mut tag,
            )
            .is_err()
        {
            return;
        }
        let mut framed = Vec::with_capacity(RTSP_HEADER + encrypted.len());
        framed.extend_from_slice(&(message.len() as u32 | RTSP_ENCRYPTED).to_be_bytes());
        framed.extend_from_slice(&sequence.to_be_bytes());
        framed.extend_from_slice(&tag);
        framed.extend_from_slice(&encrypted);
        framed
    } else {
        message.into_bytes()
    };
    let _ = stream.write_all(&bytes);
    let _ = stream.flush();
}

/// The nonce of an RTSP message: its sequence number, and who wrote it
/// (`C` for the client, `H` for the host).
fn rtsp_iv(sequence: u32, origin: u8) -> [u8; 12] {
    let mut iv = [0u8; 12];
    iv[..4].copy_from_slice(&sequence.to_le_bytes());
    iv[10] = origin;
    iv[11] = b'R';
    iv
}

// ---------------------------------------------------------------------------
// Control, video, audio
// ---------------------------------------------------------------------------

/// Starts the services once the client has announced its settings.
fn start_media(inner: &Arc<Inner>, negotiated: Negotiated) {
    {
        let mut slot = lock(&inner.negotiated);
        if slot.is_some() {
            return;
        }
        *slot = Some(negotiated);
    }
    let mut media = lock(&inner.media);
    for service in [serve_control, serve_video, serve_audio, serve_microphone] {
        let inner = Arc::clone(inner);
        media.push(thread::spawn(move || service(&inner, negotiated)));
    }
}

/// The control connection: where the client asks for keyframes and sends
/// input, and where the host says the stream is over.
fn serve_control(inner: &Inner, negotiated: Negotiated) {
    let clock = Instant::now();
    let now = || ProtocolInstant::from_std(clock);
    let encrypted = negotiated.encryption & ENCRYPT_CONTROL != 0;
    let (Ok(mut host), Some(packets)) = (
        ControlHost::new(
            now(),
            ControlHostConfig { peer_count: 1, peer_channel_count: EnetChannel::CHANNEL_COUNT },
            Arc::new(RustCryptoBackend),
        ),
        ControlPacketConfig::new(inner.version, encrypted),
    ) else {
        return;
    };
    let socket = &inner.control;
    let _ = socket.set_read_timeout(Some(Duration::from_millis(5)));
    let mut buffer = vec![0u8; 4096];
    let mut peer: Option<ControlPeerId> = None;
    // Once the host has said goodbye, how long it still waits for the
    // goodbye to arrive.
    let mut leaving: Option<Instant> = None;

    loop {
        if let Ok((length, source)) = socket.recv_from(&mut buffer) {
            let _ = host.handle_receive(now(), Receive { source, data: &buffer[..length] });
        }
        let _ = host.handle_timeout(now());

        while let Some(event) = host.poll_event() {
            match event {
                ControlHostEvent::Connected { id, sunshine_connect_data } => {
                    // A client that does not know this session's secret is
                    // not the one that launched it.
                    if sunshine_connect_data != Some(inner.connect_data) {
                        let _ = host.disconnect_now(id, 0);
                        continue;
                    }
                    let config = ControlPeerConfig {
                        role: ControlPeerRole::Server,
                        encryption: encrypted
                            .then_some((ControlEncryptionMethod::Sunshine, inner.launch.key)),
                        packets: packets.clone(),
                    };
                    if host.configure_peer(id, config).is_ok() {
                        peer = Some(id);
                        inner.connected.store(true, Ordering::SeqCst);
                    }
                }
                ControlHostEvent::Receive { packet, .. } => {
                    if matches!(
                        packet,
                        ControlPacket::RequestIdr | ControlPacket::InvalidateReferenceFrames { .. }
                    ) {
                        inner.wants_keyframe.store(true, Ordering::SeqCst);
                        inner.keyframes_requested.fetch_add(1, Ordering::SeqCst);
                    } else if let Some(input) = understood(&packet) {
                        lock(&inner.hands).follow(input, negotiated.width, negotiated.height);
                        lock(&inner.inputs).push(input);
                    }
                }
                ControlHostEvent::Disconnected { id } => {
                    if peer == Some(id) {
                        inner.client_left.store(true, Ordering::SeqCst);
                        inner.stop.store(true, Ordering::SeqCst);
                    }
                }
            }
        }

        if inner.stop.load(Ordering::SeqCst) && leaving.is_none() {
            if let (true, Some(id)) = (inner.terminate.load(Ordering::SeqCst), peer) {
                let packet =
                    ControlPacket::ServerTermination { reason: TerminationReason::GRACEFUL };
                let (channel, kind) = packet.channel(inner.version);
                let _ = host.send(id, channel, kind, packet);
                let _ = host.disconnect(id, 0);
            }
            leaving = Some(Instant::now());
        }

        while let Some(transmit) = host.poll_transmit() {
            let _ = socket.send_to(&transmit.data, transmit.destination);
        }

        if leaving.is_some_and(|since| since.elapsed() > Duration::from_millis(150)) {
            break;
        }
    }
}

/// What a packet of the client says about its keyboard, mouse or gamepads.
fn understood(packet: &ControlPacket) -> Option<Input> {
    Some(match *packet {
        ControlPacket::Keyboard { action, key_code, modifiers, .. } => Input::Key {
            key: Key(key_code.0 as u16),
            down: action == KeyAction::Down,
            modifiers: Modifiers {
                shift: modifiers.contains(KeyModifiers::SHIFT),
                ctrl: modifiers.contains(KeyModifiers::CTRL),
                alt: modifiers.contains(KeyModifiers::ALT),
                meta: modifiers.contains(KeyModifiers::META),
            },
        },
        ControlPacket::MouseMoveRelative { delta_x, delta_y } => {
            Input::MouseMove { dx: delta_x, dy: delta_y }
        }
        ControlPacket::MouseMoveAbsolute { x, y, reference_width, reference_height, .. } => {
            Input::MousePosition { x, y, width: reference_width, height: reference_height }
        }
        ControlPacket::MouseButton { action, button } => Input::MouseButton {
            button: match button {
                ProtocolMouseButton::Left => MouseButton::Left,
                ProtocolMouseButton::Middle => MouseButton::Middle,
                ProtocolMouseButton::Right => MouseButton::Right,
                ProtocolMouseButton::X1 => MouseButton::Back,
                ProtocolMouseButton::X2 => MouseButton::Forward,
            },
            down: action == MouseButtonAction::Press,
        },
        ControlPacket::MouseScroll { scroll_amount_1, .. } => {
            Input::Scroll { horizontal: 0, vertical: scroll_amount_1 }
        }
        ControlPacket::MouseHorizontalScroll { scroll_amount } => {
            Input::Scroll { horizontal: scroll_amount, vertical: 0 }
        }
        ControlPacket::ControllerArrival { controller_number, ty, .. } => Input::GamepadArrived {
            number: controller_number,
            kind: match ty {
                ControllerType::Xbox => GamepadKind::Xbox,
                ControllerType::PlayStation => GamepadKind::PlayStation,
                ControllerType::Nintendo => GamepadKind::Nintendo,
                ControllerType::Unknown | ControllerType::Steam => GamepadKind::Unknown,
            },
        },
        ControlPacket::ControllerState {
            controller_number,
            active_gamepad_mask,
            button_flags,
            button_flags_2,
            left_trigger,
            right_trigger,
            left_stick_x,
            left_stick_y,
            right_stick_x,
            right_stick_y,
            ..
        } => {
            let number = u8::try_from(controller_number).ok()?;
            // A gamepad that is no longer among the active ones was unplugged.
            if !active_gamepad_mask.contains(ActiveGamepads::from_id(number)?) {
                return Some(Input::GamepadLeft { number });
            }
            let held = ControllerButtons::from_bits_truncate(
                u32::from(button_flags as u16) | u32::from(button_flags_2 as u16) << 16,
            );
            let mut buttons = GamepadButtons::NONE;
            for (theirs, ours) in [
                (ControllerButtons::A, GamepadButtons::A),
                (ControllerButtons::B, GamepadButtons::B),
                (ControllerButtons::X, GamepadButtons::X),
                (ControllerButtons::Y, GamepadButtons::Y),
                (ControllerButtons::UP, GamepadButtons::UP),
                (ControllerButtons::DOWN, GamepadButtons::DOWN),
                (ControllerButtons::LEFT, GamepadButtons::LEFT),
                (ControllerButtons::RIGHT, GamepadButtons::RIGHT),
                (ControllerButtons::LB, GamepadButtons::LEFT_SHOULDER),
                (ControllerButtons::RB, GamepadButtons::RIGHT_SHOULDER),
                (ControllerButtons::PLAY, GamepadButtons::START),
                (ControllerButtons::BACK, GamepadButtons::BACK),
                (ControllerButtons::LS_CLK, GamepadButtons::LEFT_STICK),
                (ControllerButtons::RS_CLK, GamepadButtons::RIGHT_STICK),
                (ControllerButtons::SPECIAL, GamepadButtons::GUIDE),
            ] {
                buttons.set(ours, held.contains(theirs));
            }
            let stick = |value: i16| f32::from(value) / f32::from(i16::MAX);
            Input::Gamepad {
                number,
                state: GamepadState {
                    buttons,
                    left_trigger: f32::from(left_trigger) / 255.0,
                    right_trigger: f32::from(right_trigger) / 255.0,
                    left_stick: [stick(left_stick_x), stick(left_stick_y)],
                    right_stick: [stick(right_stick_x), stick(right_stick_y)],
                },
            }
        }
        _ => return None,
    })
}

/// Waits for the client to show itself on `socket` and returns its address.
///
/// A client cannot be sent anything until it has sent a ping: the ping opens
/// the way back through routers and firewalls, and tells the host the port.
fn wait_for_client(inner: &Inner, socket: &UdpSocket) -> Option<SocketAddr> {
    let _ = socket.set_read_timeout(Some(Duration::from_millis(20)));
    let mut buffer = [0u8; 64];
    while !inner.stop.load(Ordering::SeqCst) {
        if let Ok((length, source)) = socket.recv_from(&mut buffer)
            && length >= inner.ping.len()
            && buffer[..inner.ping.len()] == inner.ping
        {
            return Some(source);
        }
    }
    None
}

/// Empties the socket of the pings the client keeps sending.
fn drain(socket: &UdpSocket) {
    let mut buffer = [0u8; 64];
    while socket.recv_from(&mut buffer).is_ok() {}
}

fn serve_video(inner: &Inner, negotiated: Negotiated) {
    let socket = &inner.video;
    let Some(client) = wait_for_client(inner, socket) else { return };
    let _ = socket.set_nonblocking(true);

    let encrypted = negotiated.encryption & ENCRYPT_VIDEO != 0;
    let mut payloader = VideoPayloader::new(
        VideoPayloaderConfig {
            server_version: inner.version,
            // The client announces the payload size. The packet adds the RTP
            // header and, when encrypted, the encryption header.
            packet_size: negotiated.packet_size + 16 + if encrypted { 32 } else { 0 },
            max_data_shards_per_block: 90,
            fec: Some(VideoPayloaderFecConfig {
                min_required_fec_packets: negotiated.min_fec_packets,
                fec_percentage: 20,
            }),
            encryption: encrypted.then_some((inner.launch.key, inner.launch.key_id)),
        },
        Arc::new(RustCryptoBackend),
    );
    let Some(mut card) =
        TestCard::new(negotiated.width, negotiated.height, negotiated.fps, negotiated.bitrate_kbps)
    else {
        return;
    };

    let interval = Duration::from_secs_f64(1.0 / f64::from(negotiated.fps));
    let mut due = Instant::now();
    let mut number: u64 = 0;
    while !inner.stop.load(Ordering::SeqCst) {
        drain(socket);
        let keyframe = inner.wants_keyframe.swap(false, Ordering::SeqCst);
        let hands = *lock(&inner.hands);
        let seen = inner.launch.camera.as_ref().and_then(|cameras| cameras.seen());
        if let Some(frame) = card.next(keyframe, hands, seen.as_deref()) {
            let kind = if frame.keyframe { ServerFrameType::Idr } else { ServerFrameType::PFrame };
            // Video time counts in ninetieths of a millisecond.
            let timestamp = (number * 90_000 / u64::from(negotiated.fps)) as u32;
            if payloader.push_frame(timestamp, None, kind, frame.data).is_ok() {
                while let Some(packet) = payloader.poll_packet() {
                    send(socket, &packet, client);
                }
                inner.frames_sent.fetch_add(1, Ordering::SeqCst);
                if frame.keyframe {
                    inner.keyframes_sent.fetch_add(1, Ordering::SeqCst);
                }
            }
        }
        number += 1;
        due = pace(due, interval);
    }
}

fn serve_audio(inner: &Inner, negotiated: Negotiated) {
    let socket = &inner.audio;
    let Some(client) = wait_for_client(inner, socket) else { return };
    let _ = socket.set_nonblocking(true);

    let samples = (AUDIO_RATE as usize * AUDIO_PACKET.as_millis() as usize) / 1000;
    let frame_bytes = AUDIO_BITRATE as usize / 8 * AUDIO_PACKET.as_millis() as usize / 1000;
    let Some(mut melody) = Melody::new(AUDIO_RATE, AUDIO_BITRATE, samples) else { return };
    let encrypted = negotiated.encryption & ENCRYPT_AUDIO != 0;
    let mut payloader = AudioPayloader::new(
        AudioPayloaderConfig {
            // A real host always adds recovery packets. moonlight-common's
            // payloader sizes them wrongly for encrypted sound (at commit
            // 5a3aa1f) and panics, so they are left out in that case. A
            // client only needs them when packets are lost.
            fec: !encrypted,
            frame_len: frame_bytes,
            encryption: encrypted.then_some((inner.launch.key, inner.launch.key_id)),
        },
        Arc::new(RustCryptoBackend),
    );

    let mut due = Instant::now();
    let mut number: u32 = 0;
    while !inner.stop.load(Ordering::SeqCst) {
        drain(socket);
        if let Some(packet) = melody.next()
            && packet.len() == frame_bytes
            && payloader.push_frame(AUDIO_PACKET * number, packet).is_ok()
        {
            while let Some(packet) = payloader.poll_packet() {
                send(socket, &packet, client);
            }
            inner.audio_packets_sent.fetch_add(1, Ordering::SeqCst);
        }
        number = number.wrapping_add(1);
        due = pace(due, AUDIO_PACKET);
    }
}

/// The microphone of the client: the one part of a stream that flows towards
/// the host. What arrives is decoded and kept, and the test card shows how
/// loud it is.
fn serve_microphone(inner: &Inner, negotiated: Negotiated) {
    if !inner.listens(negotiated.encryption) {
        return;
    }
    let socket = &inner.microphone;
    let _ = socket.set_read_timeout(Some(Duration::from_millis(50)));
    let Ok(mut decoder) = OpusDecoder::new(MICROPHONE_RATE, 1) else { return };
    let mut packet = [0u8; 2048];
    let mut samples = vec![0f32; MICROPHONE_FRAME];
    lock(&inner.hands).voice = Some(0.0);

    while !inner.stop.load(Ordering::SeqCst) {
        let Ok((length, _)) = socket.recv_from(&mut packet) else {
            // Nothing for a moment: the meter falls back.
            if let Some(voice) = &mut lock(&inner.hands).voice {
                *voice *= 0.5;
            }
            continue;
        };
        let Some(frame) = open_microphone_packet(&inner.launch, &packet[..length]) else {
            continue;
        };
        let Ok(decoded) = decoder.decode_float(&frame, &mut samples, false) else { continue };
        let decoded = &samples[..decoded.min(samples.len())];
        inner.microphone_packets.fetch_add(1, Ordering::SeqCst);

        let loudness = loudness(decoded);
        if let Some(voice) = &mut lock(&inner.hands).voice {
            // Up at once, down slowly: easier to follow by eye.
            *voice = loudness.max(*voice * 0.85);
        }
        let mut heard = lock(&inner.heard);
        heard.extend(decoded);
        let extra = heard.len().saturating_sub(HEARD_LIMIT);
        heard.drain(..extra);
    }
}

/// The Opus frame inside a microphone packet, or `None` for anything else
/// that reaches the port.
fn open_microphone_packet(launch: &Launch, packet: &[u8]) -> Option<Vec<u8>> {
    let (header, encrypted) = packet.split_at_checked(MICROPHONE_HEADER)?;
    let magic = u32::from_le_bytes([header[8], header[9], header[10], header[11]]);
    if header[1] != MICROPHONE_PACKET || magic != MICROPHONE_MAGIC || encrypted.is_empty() {
        return None;
    }
    // Every packet has a nonce of its own: the number of the key plus the
    // number of the packet.
    let sequence = u16::from_le_bytes([header[2], header[3]]);
    let mut iv = [0u8; 16];
    iv[..4].copy_from_slice(&launch.key_id.0.wrapping_add(u32::from(sequence)).to_be_bytes());
    let mut frame = vec![0u8; encrypted.len() + 16];
    let length = RustCryptoBackend.decrypt_aes_cbc(&launch.key, &iv, encrypted, &mut frame).ok()?;
    frame.truncate(length);
    Some(frame)
}

/// How loud `samples` are, from 0 for silence to 1 for as loud as it gets.
fn loudness(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let mean = samples.iter().map(|sample| sample * sample).sum::<f32>() / samples.len() as f32;
    // A full sine has a mean square of a half.
    (mean * 2.0).sqrt().min(1.0)
}

/// Sends one packet, waiting a little when the system's buffer is full.
fn send(socket: &UdpSocket, packet: &[u8], client: SocketAddr) {
    for _ in 0..50 {
        match socket.send_to(packet, client) {
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_micros(200));
            }
            _ => return,
        }
    }
}

/// Sleeps until `due + interval` and returns that instant. After a stall it
/// starts counting again from now instead of rushing to catch up.
fn pace(due: Instant, interval: Duration) -> Instant {
    let next = due + interval;
    let now = Instant::now();
    if next > now {
        thread::sleep(next - now);
        next
    } else if now - next > interval * 4 {
        now
    } else {
        next
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

pub(crate) fn fill_random(bytes: &mut [u8]) {
    use moonlight_common::http::pair::PairingCryptoBackend;
    RustCryptoBackend.random_bytes(bytes).expect("the system provides random bytes");
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02X}")).collect()
}

fn ascii(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}
