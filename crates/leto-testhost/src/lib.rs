//! A stand-in for a Sunshine or Apollo host.
//!
//! The tests of the clients run against it, and `cargo run -p leto-testhost
//! --example serve` keeps one running to try a client by hand without a
//! gaming PC. It is not a host: it captures no screen and runs no app. What
//! it streams is a test card and a melody.
//!
//! It implements what a client sees of a real host. Before streaming:
//! `serverinfo`, the five pairing steps, the app list and app covers, over
//! plain HTTP and over HTTPS with client certificates. The pairing arithmetic
//! follows `nvhttp.cpp` in Sunshine and Apollo, and the XML is written by hand
//! the way those hosts write it, so a client's parsing is exercised too. Then
//! `launch`, `resume` and `cancel`, and the stream itself, in [`stream`].

// Test support: a failed assumption should stop the test loudly.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod media;
mod stream;

use std::{
    collections::HashMap,
    io::{Read, Write},
    net::{IpAddr, Ipv4Addr, TcpListener, TcpStream},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use leto_core::model::Pin;
use moonlight_common::{
    crypto::rustcrypto::RustCryptoBackend,
    http::{
        ClientIdentifier, ClientSecret, ServerIdentifier,
        pair::{HashAlgorithm, PairingCryptoBackend},
    },
    stream::{AesIv, AesKey},
};
use rustls::{
    DigitallySignedStruct, DistinguishedName, ServerConfig, ServerConnection, SignatureScheme,
    StreamOwned,
    crypto::{WebPkiSupportedAlgorithms, verify_tls12_signature, verify_tls13_signature},
    pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer, UnixTime},
    server::danger::{ClientCertVerified, ClientCertVerifier},
};

pub use stream::StreamReport;

/// A tiny valid PNG: one transparent pixel.
pub const COVER_PNG: &[u8] = &[
    0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1F, 0x15, 0xC4,
    0x89, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9C, 0x63, 0x00, 0x01, 0x00, 0x00,
    0x05, 0x00, 0x01, 0x0D, 0x0A, 0x2D, 0xB4, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE,
    0x42, 0x60, 0x82,
];

pub const HOST_NAME: &str = "Salon";
pub const HOST_ID: &str = "7c9e6679-7425-40de-944b-e07fc1f90ae7";

/// One app the host offers.
#[derive(Debug, Clone)]
pub struct App {
    pub id: u32,
    pub title: String,
    pub hdr: bool,
    /// The cover image, as the bytes of an image file.
    pub cover: Vec<u8>,
}

impl App {
    /// An app with the smallest possible cover.
    pub fn new(id: u32, title: &str, hdr: bool) -> Self {
        Self { id, title: title.to_owned(), hdr, cover: COVER_PNG.to_vec() }
    }
}

/// How a [`FakeHost`] presents itself.
#[derive(Debug, Clone)]
pub struct Options {
    pub name: String,
    /// The unique id the host reports. Clients tell hosts apart by it.
    pub id: String,
    /// The interface to listen on: `"127.0.0.1"`, or `"0.0.0.0"` for all.
    pub interface: String,
    /// The port of the plain HTTP API, or 0 for any free port.
    pub port: u16,
    /// How long the host waits for someone to type the PIN.
    pub pin_patience: Duration,
    pub apps: Vec<App>,
    /// Encrypt the RTSP handshake for clients that can, as a real host does
    /// by default. Without it the handshake is in clear text, as a host
    /// configured to never encrypt does it.
    pub encrypt_rtsp: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            name: HOST_NAME.to_owned(),
            id: HOST_ID.to_owned(),
            interface: "127.0.0.1".to_owned(),
            port: 0,
            pin_patience: Duration::from_secs(5),
            apps: vec![
                App::new(1, "Desktop", true),
                App::new(881_448_767, "Steam Big Picture", false),
            ],
            encrypt_rtsp: true,
        }
    }
}

/// The host's certificate and key, and the TLS setup made from them.
struct HostIdentity {
    certificate_pem: String,
    certificate: ServerIdentifier,
    /// The host's private key. The crypto helpers call any private key a
    /// "client secret".
    key: ClientSecret,
    tls: Arc<ServerConfig>,
}

/// What one pairing attempt has established so far.
#[derive(Default)]
struct Pairing {
    client_certificate: Option<pem::Pem>,
    key: [u8; 16],
    server_secret: [u8; 16],
    server_challenge: [u8; 16],
    client_hash: Vec<u8>,
}

struct Shared {
    identity: Mutex<HostIdentity>,
    /// The PIN typed on the host, once someone typed it.
    typed_pin: Mutex<Option<Pin>>,
    pairing: Mutex<Pairing>,
    /// Certificates of paired clients, DER encoded.
    paired: Mutex<Vec<Vec<u8>>>,
    playing: Mutex<Playing>,
    options: Options,
    http_port: u16,
    https_port: u16,
    stop: AtomicBool,
}

/// The app the host is running, and the stream to the client watching it.
#[derive(Default)]
struct Playing {
    /// The running app. It keeps running when the client leaves.
    app: Option<u32>,
    session: Option<stream::Session>,
    /// What the last stream that ended had done.
    last: Option<StreamReport>,
}

impl Playing {
    /// Ends the stream, if there is one, and keeps its report.
    fn end_stream(&mut self, tell_client: bool) {
        if let Some(mut session) = self.session.take() {
            session.stop(tell_client);
            self.last = Some(session.report());
        }
    }
}

/// A running fake host. Stops when dropped.
pub struct FakeHost {
    shared: Arc<Shared>,
    threads: Vec<thread::JoinHandle<()>>,
}

impl FakeHost {
    /// A host reachable on the loopback interface only.
    pub fn start() -> Self {
        Self::start_with(Options::default())
    }

    /// A host listening on `interface`, for example `"0.0.0.0"` for all.
    pub fn start_on(interface: &str) -> Self {
        Self::start_with(Options { interface: interface.to_owned(), ..Options::default() })
    }

    pub fn start_with(options: Options) -> Self {
        let http = TcpListener::bind((options.interface.as_str(), options.port)).unwrap();
        let https = TcpListener::bind((options.interface.as_str(), 0)).unwrap();
        let shared = Arc::new(Shared {
            identity: Mutex::new(new_identity()),
            typed_pin: Mutex::new(None),
            pairing: Mutex::new(Pairing::default()),
            paired: Mutex::new(Vec::new()),
            playing: Mutex::new(Playing::default()),
            options,
            http_port: http.local_addr().unwrap().port(),
            https_port: https.local_addr().unwrap().port(),
            stop: AtomicBool::new(false),
        });

        let threads =
            vec![serve(http, Arc::clone(&shared), false), serve(https, Arc::clone(&shared), true)];
        Self { shared, threads }
    }

    /// The address a person would type to add this host.
    pub fn address(&self) -> String {
        format!("127.0.0.1:{}", self.shared.http_port)
    }

    /// The port of the plain HTTP API, the one hosts announce.
    pub fn port(&self) -> u16 {
        self.shared.http_port
    }

    /// Someone types a PIN in the host's pairing page.
    pub fn type_pin(&self, pin: Pin) {
        *self.shared.typed_pin.lock().unwrap() = Some(pin);
    }

    /// The host's owner removes every paired device.
    pub fn forget_clients(&self) {
        self.shared.paired.lock().unwrap().clear();
    }

    pub fn paired_clients(&self) -> usize {
        self.shared.paired.lock().unwrap().len()
    }

    /// The app the host is running, whether or not a client is watching.
    pub fn running_app(&self) -> Option<u32> {
        self.shared.playing.lock().unwrap().app
    }

    /// What the current stream has done so far, or the last one if it ended.
    pub fn stream_report(&self) -> Option<StreamReport> {
        let playing = self.shared.playing.lock().unwrap();
        playing.session.as_ref().map(stream::Session::report).or(playing.last)
    }

    /// The app closes on the host: the host tells the client and the stream
    /// ends.
    pub fn close_app(&self) {
        let mut playing = self.shared.playing.lock().unwrap();
        playing.end_stream(true);
        playing.app = None;
    }

    /// The host ends the stream but the app keeps running, as when the
    /// host's owner stops the session from the host's own interface.
    pub fn drop_stream(&self) {
        self.shared.playing.lock().unwrap().end_stream(true);
    }

    /// Something else now answers on this address with the same name and id
    /// but another certificate and key: an impostor, or a reinstalled host.
    pub fn become_impostor(&self) {
        *self.shared.identity.lock().unwrap() = new_identity();
    }
}

impl Drop for FakeHost {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::SeqCst);
        self.shared.playing.lock().unwrap().end_stream(false);
        for thread in self.threads.drain(..) {
            let _ = thread.join();
        }
    }
}

fn new_identity() -> HostIdentity {
    let (certificate, key) = RustCryptoBackend.generate_client_identity().unwrap();
    let certificate_pem = certificate.to_pem();
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let verifier = AnyClient { algorithms: provider.signature_verification_algorithms };
    let tls = ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_client_cert_verifier(Arc::new(verifier))
        .with_single_cert(
            vec![CertificateDer::from(certificate_pem.contents().to_vec())],
            PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key.to_pem().contents().to_vec())),
        )
        .unwrap();
    HostIdentity {
        certificate_pem: certificate_pem.to_string(),
        certificate: ServerIdentifier::from_pem(certificate_pem),
        key,
        tls: Arc::new(tls),
    }
}

/// Like Sunshine, accept any client certificate during the TLS handshake and
/// decide afterwards whether that client is paired.
#[derive(Debug)]
struct AnyClient {
    algorithms: WebPkiSupportedAlgorithms,
}

impl ClientCertVerifier for AnyClient {
    fn root_hint_subjects(&self) -> &[DistinguishedName] {
        &[]
    }

    fn verify_client_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _now: UnixTime,
    ) -> Result<ClientCertVerified, rustls::Error> {
        Ok(ClientCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        verify_tls12_signature(message, cert, dss, &self.algorithms)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        verify_tls13_signature(message, cert, dss, &self.algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.algorithms.supported_schemes()
    }
}

fn serve(listener: TcpListener, shared: Arc<Shared>, secure: bool) -> thread::JoinHandle<()> {
    listener.set_nonblocking(true).unwrap();
    thread::spawn(move || {
        while !shared.stop.load(Ordering::SeqCst) {
            match listener.accept() {
                Ok((stream, _)) => {
                    let shared = Arc::clone(&shared);
                    thread::spawn(move || {
                        let _ = stream.set_nonblocking(false);
                        let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));
                        if secure {
                            handle_secure(stream, &shared)
                        } else {
                            handle_plain(stream, &shared)
                        }
                    });
                }
                Err(_) => thread::sleep(Duration::from_millis(5)),
            }
        }
    })
}

fn handle_plain(mut stream: TcpStream, shared: &Shared) {
    let Some(target) = read_request(&mut stream) else { return };
    let (kind, body) = respond(shared, &target, None, local_ip(&stream));
    write_response(&mut stream, kind, &body);
}

fn handle_secure(stream: TcpStream, shared: &Shared) {
    let config = Arc::clone(&shared.identity.lock().unwrap().tls);
    let Ok(connection) = ServerConnection::new(config) else { return };
    let mut stream = StreamOwned::new(connection, stream);
    // Reading the request runs the handshake first.
    let Some(target) = read_request(&mut stream) else { return };
    let client =
        stream.conn.peer_certificates().and_then(|chain| chain.first()).map(|c| c.to_vec());
    let (kind, body) = respond(shared, &target, Some(client), local_ip(&stream.sock));
    write_response(&mut stream, kind, &body);
    stream.conn.send_close_notify();
    let _ = stream.flush();
}

/// The address of this machine the client reached.
fn local_ip(stream: &TcpStream) -> IpAddr {
    stream.local_addr().map_or(IpAddr::V4(Ipv4Addr::LOCALHOST), |address| address.ip())
}

/// Reads one request and returns its target (path and query).
fn read_request(stream: &mut impl Read) -> Option<String> {
    let mut buffer = Vec::new();
    let mut chunk = [0u8; 4096];
    while !buffer.windows(4).any(|window| window == b"\r\n\r\n") {
        let read = stream.read(&mut chunk).ok()?;
        if read == 0 {
            return None;
        }
        buffer.extend_from_slice(&chunk[..read]);
    }
    let text = String::from_utf8_lossy(&buffer);
    let mut words = text.lines().next()?.split(' ');
    (words.next()? == "GET").then(|| words.next().map(str::to_owned))?
}

fn write_response(stream: &mut impl Write, kind: &str, body: &[u8]) {
    let head = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let _ = stream.write_all(head.as_bytes());
    let _ = stream.write_all(body);
    let _ = stream.flush();
}

const XML: &str = "application/xml";

fn xml(inner: &str) -> (&'static str, Vec<u8>) {
    let text = format!(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<root status_code=\"200\">{inner}</root>"
    );
    (XML, text.into_bytes())
}

/// An error the way Sunshine reports one: in the root element.
fn failure(code: u16, message: &str) -> (&'static str, Vec<u8>) {
    let text = format!(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<root status_code=\"{code}\" \
         status_message=\"{message}\"><gamesession>0</gamesession></root>"
    );
    (XML, text.into_bytes())
}

fn unauthorized(path: &str) -> (&'static str, Vec<u8>) {
    let text = format!(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<root status_code=\"401\" query=\"{path}\" \
         status_message=\"The client is not authorized. Certificate verification failed.\"/>"
    );
    (XML, text.into_bytes())
}

/// Answers one request. `client` is `None` over plain HTTP, and over HTTPS it
/// is the certificate the client presented, if any.
fn respond(
    shared: &Shared,
    target: &str,
    client: Option<Option<Vec<u8>>>,
    local: IpAddr,
) -> (&'static str, Vec<u8>) {
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    let query: HashMap<&str, &str> =
        query.split('&').filter_map(|pair| pair.split_once('=')).collect();
    let secure = client.is_some();
    let authorized = client
        .flatten()
        .is_some_and(|certificate| shared.paired.lock().unwrap().contains(&certificate));

    if secure && !authorized {
        return unauthorized(path);
    }

    match path {
        "/serverinfo" => {
            let running = {
                let mut playing = shared.playing.lock().unwrap();
                // A client that left without a word: the app keeps running.
                if playing.session.as_ref().is_some_and(|session| !session.alive()) {
                    playing.end_stream(false);
                }
                playing.app
            };
            xml(&format!(
                "<appversion>{}</appversion><hostname>{}</hostname>\
             <GfeVersion>3.23.0.74</GfeVersion><uniqueid>{}</uniqueid>\
             <HttpsPort>{}</HttpsPort><ExternalPort>{}</ExternalPort>\
             <MaxLumaPixelsHEVC>1869449984</MaxLumaPixelsHEVC><mac>00:11:22:33:44:55</mac>\
             <LocalIP>127.0.0.1</LocalIP><ServerCodecModeSupport>3</ServerCodecModeSupport>\
             <PairStatus>{}</PairStatus><currentgame>{}</currentgame>\
             <state>{}</state>",
                stream::APP_VERSION,
                shared.options.name,
                shared.options.id,
                shared.https_port,
                shared.http_port,
                u8::from(authorized),
                running.unwrap_or(0),
                if running.is_some() { "SUNSHINE_SERVER_BUSY" } else { "SUNSHINE_SERVER_FREE" },
            ))
        }
        "/pair" => pair(shared, &query, secure),
        "/unpair" => {
            shared.paired.lock().unwrap().clear();
            xml("")
        }
        "/applist" if secure => {
            let apps: String = shared
                .options
                .apps
                .iter()
                .map(|app| {
                    format!(
                        "<App><IsHdrSupported>{}</IsHdrSupported><AppTitle>{}</AppTitle>\
                         <ID>{}</ID></App>",
                        u8::from(app.hdr),
                        app.title,
                        app.id
                    )
                })
                .collect();
            xml(&apps)
        }
        "/launch" | "/resume" if secure => launch(shared, path == "/resume", &query, local),
        "/cancel" if secure => {
            let mut playing = shared.playing.lock().unwrap();
            playing.end_stream(true);
            playing.app = None;
            xml("<cancel>1</cancel>")
        }
        "/appasset" if secure => {
            let wanted = query.get("appid").and_then(|id| id.parse::<u32>().ok());
            let app = shared.options.apps.iter().find(|app| Some(app.id) == wanted);
            ("image/png", app.map_or_else(|| COVER_PNG.to_vec(), |app| app.cover.clone()))
        }
        _ => unauthorized(path),
    }
}

/// Starts an app, or goes back to the one that is running, and opens the
/// ports of a new stream. Answers as `nvhttp.cpp` does.
fn launch(
    shared: &Shared,
    resume: bool,
    query: &HashMap<&str, &str>,
    local: IpAddr,
) -> (&'static str, Vec<u8>) {
    let mut playing = shared.playing.lock().unwrap();
    let app = match (resume, playing.app) {
        (true, Some(app)) => app,
        (true, None) => return failure(503, "No running app to resume"),
        (false, Some(_)) => return failure(400, "An app is already running on this host"),
        (false, None) => {
            let wanted = query.get("appid").and_then(|id| id.parse::<u32>().ok());
            match shared.options.apps.iter().find(|app| Some(app.id) == wanted) {
                Some(app) => app.id,
                None => return failure(404, "Cannot find requested application"),
            }
        }
    };

    // The key and the number every part of the stream is encrypted with.
    let key = query.get("rikey").and_then(|key| unhex(key)).and_then(|key| key.try_into().ok());
    let key_id = query.get("rikeyid").and_then(|id| id.parse::<i64>().ok());
    let (Some(key), Some(key_id)) = (key, key_id) else {
        return failure(400, "Missing a required launch parameter");
    };
    let launch = stream::Launch {
        key: AesKey(key),
        key_id: AesIv(key_id as u32),
        // Clients that can encrypt RTSP say so with `corever`.
        encrypt_rtsp: shared.options.encrypt_rtsp
            && query.get("corever").and_then(|version| version.parse::<u32>().ok()) >= Some(1),
    };

    playing.end_stream(false);
    let session = match stream::Session::start(local, launch) {
        Ok(session) => session,
        Err(error) => return failure(503, &format!("Failed to start the stream: {error}")),
    };
    let url = session.url(local);
    playing.app = Some(app);
    playing.session = Some(session);
    if resume {
        xml(&format!("<sessionUrl0>{url}</sessionUrl0><resume>1</resume>"))
    } else {
        xml(&format!("<sessionUrl0>{url}</sessionUrl0><gamesession>1</gamesession>"))
    }
}

/// The five pairing steps, as `nvhttp.cpp` does them.
fn pair(shared: &Shared, query: &HashMap<&str, &str>, secure: bool) -> (&'static str, Vec<u8>) {
    let crypto = RustCryptoBackend;
    let failed = || xml("<paired>0</paired>");

    // Step 5, over HTTPS: the client proves it can reach the host as a paired
    // device. `respond` already checked its certificate.
    if secure {
        return if query.get("phrase") == Some(&"pairchallenge") {
            xml("<paired>1</paired>")
        } else {
            failed()
        };
    }

    // Step 1: the client sends a salt and its certificate, then waits for the
    // PIN to be typed on the host.
    if query.get("phrase") == Some(&"getservercert") {
        let (Some(salt), Some(certificate)) = (
            query.get("salt").and_then(|hex| unhex(hex)),
            query.get("clientcert").and_then(|hex| unhex(hex)),
        ) else {
            return failed();
        };
        let Ok(certificate) = pem::parse(certificate) else { return failed() };

        let deadline = Instant::now() + shared.options.pin_patience;
        let pin = loop {
            if let Some(pin) = shared.typed_pin.lock().unwrap().take() {
                break pin;
            }
            if Instant::now() > deadline {
                return failed();
            }
            thread::sleep(Duration::from_millis(10));
        };

        let mut salted = salt;
        salted.extend(pin.to_string().bytes());
        let mut pairing = shared.pairing.lock().unwrap();
        *pairing = Pairing { client_certificate: Some(certificate), ..Pairing::default() };
        pairing.key.copy_from_slice(&sha256(&salted)[..16]);

        let plain = shared.identity.lock().unwrap().certificate_pem.replace("\r\n", "\n");
        return xml(&format!("<paired>1</paired><plaincert>{}</plaincert>", hex(plain.as_bytes())));
    }

    let mut pairing = shared.pairing.lock().unwrap();
    let identity = shared.identity.lock().unwrap();

    // Step 2: the client sends a random challenge, encrypted with the key
    // both sides derived from the PIN.
    if let Some(challenge) = query.get("clientchallenge").and_then(|hex| unhex(hex)) {
        let Ok(challenge) = crypto.decrypt_aes(&pairing.key, &challenge) else { return failed() };
        crypto.random_bytes(&mut pairing.server_secret).unwrap();
        crypto.random_bytes(&mut pairing.server_challenge).unwrap();

        let mut proof = challenge;
        proof.extend(crypto.server_signature(&identity.certificate).unwrap());
        proof.extend(pairing.server_secret);
        let mut plain = sha256(&proof).to_vec();
        plain.extend(pairing.server_challenge);

        let answer = crypto.encrypt_aes(&pairing.key, &plain).unwrap();
        return xml(&format!(
            "<paired>1</paired><challengeresponse>{}</challengeresponse>",
            hex(&answer)
        ));
    }

    // Step 3: the client answers the host's challenge. The host reveals its
    // secret, signed with its private key.
    if let Some(answer) = query.get("serverchallengeresp").and_then(|hex| unhex(hex)) {
        let Ok(client_hash) = crypto.decrypt_aes(&pairing.key, &answer) else { return failed() };
        pairing.client_hash = client_hash;

        let mut secret = pairing.server_secret.to_vec();
        secret.extend(crypto.sign_data(&identity.key, &pairing.server_secret).unwrap());
        return xml(&format!("<pairingsecret>{}</pairingsecret><paired>1</paired>", hex(&secret)));
    }

    // Step 4: the client reveals its secret, signed. The host checks it
    // against the answer from step 3, which only works if the PINs matched.
    if let Some(secret) = query.get("clientpairingsecret").and_then(|hex| unhex(hex)) {
        let Some(certificate) = pairing.client_certificate.clone() else { return failed() };
        if secret.len() <= 16 {
            return failed();
        }
        let (client_secret, signature) = secret.split_at(16);

        let mut proof = pairing.server_challenge.to_vec();
        proof.extend(
            crypto.client_signature(&ClientIdentifier::from_pem(certificate.clone())).unwrap(),
        );
        proof.extend(client_secret);
        let pin_matched = pairing.client_hash.get(..32) == Some(&sha256(&proof)[..]);

        // The helper that checks a signature against a certificate calls that
        // certificate a "server identifier", whoever it belongs to.
        let signed = crypto
            .verify_signature(
                client_secret,
                signature,
                &ServerIdentifier::from_pem(certificate.clone()),
            )
            .unwrap_or(false);

        if pin_matched && signed {
            shared.paired.lock().unwrap().push(certificate.contents().to_vec());
            return xml("<paired>1</paired>");
        }
        return failed();
    }

    failed()
}

fn sha256(data: &[u8]) -> [u8; 32] {
    let mut out = [0u8; 32];
    RustCryptoBackend.hash(HashAlgorithm::Sha256, data, &mut out).unwrap();
    out
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02X}")).collect()
}

fn unhex(text: &str) -> Option<Vec<u8>> {
    if !text.len().is_multiple_of(2) {
        return None;
    }
    (0..text.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(text.get(index..index + 2)?, 16).ok())
        .collect()
}
