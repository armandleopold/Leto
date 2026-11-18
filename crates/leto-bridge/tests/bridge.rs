//! A browser's view of the bridge, end to end: a stand-in host on one side,
//! on the other what a browser does, over HTTPS and a WebSocket.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::{
    fs,
    net::{IpAddr, Ipv4Addr},
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU32, Ordering},
    },
    time::Duration,
};

use futures_util::{SinkExt as _, StreamExt as _};
use leto_bridge::{Bridge, Options};
use leto_client::{
    Client,
    engine::{Event, Request},
    input::{Input, Key, Modifiers},
};
use leto_core::{
    bridge::{Down, Frame, Up},
    model::HostId,
};
use leto_testhost::{FakeHost, quiet_voice};
use rustls::{
    ClientConfig, RootCertStore,
    pki_types::{CertificateDer, ServerName, pem::PemObject as _},
};
use tokio::{
    io::{AsyncReadExt as _, AsyncWriteExt as _},
    net::TcpStream,
    time::{Instant, sleep, timeout},
};
use tokio_rustls::{TlsConnector, client::TlsStream};
use tokio_tungstenite::{
    WebSocketStream, client_async,
    tungstenite::{Message, client::IntoClientRequest as _},
};

/// How long any step may take before the test gives up.
const PATIENCE: Duration = Duration::from_secs(30);

/// A bridge on this computer, with a folder of its own and a web client of
/// one page.
struct Setup {
    bridge: Bridge,
    folder: PathBuf,
    /// Every code the gate drew after the first.
    codes: Arc<Mutex<Vec<String>>>,
}

impl Setup {
    fn new() -> Self {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let name = format!(
            "leto-bridge-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::SeqCst)
        );
        let folder = std::env::temp_dir().join(name);
        let _ = fs::remove_dir_all(&folder);
        let pages = folder.join("web");
        fs::create_dir_all(&pages).unwrap();
        fs::write(pages.join("index.html"), "<!doctype html><title>Leto</title>").unwrap();

        let client = Arc::new(Client::open(&folder.join("client")).unwrap());
        let options = Options {
            folder: folder.join("bridge"),
            pages,
            interface: IpAddr::V4(Ipv4Addr::LOCALHOST),
            port: 0,
        };
        let codes = Arc::new(Mutex::new(Vec::new()));
        let heard = Arc::clone(&codes);
        let announce = Box::new(move |code: &str| heard.lock().unwrap().push(code.to_owned()));
        let bridge = Bridge::start(client, options, announce).unwrap();
        Self { bridge, folder, codes }
    }

    fn host(&self) -> String {
        format!("localhost:{}", self.bridge.port())
    }

    /// A TLS connection that trusts this bridge's certificate and no other.
    async fn connect(&self) -> TlsStream<TcpStream> {
        let mut roots = RootCertStore::empty();
        let certificate = CertificateDer::from_pem_slice(self.bridge.certificate().as_bytes());
        roots.add(certificate.unwrap()).unwrap();
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let config = ClientConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_root_certificates(roots)
            .with_no_client_auth();
        let stream = TcpStream::connect(("127.0.0.1", self.bridge.port())).await.unwrap();
        let name = ServerName::try_from("localhost").unwrap();
        TlsConnector::from(Arc::new(config)).connect(name, stream).await.unwrap()
    }

    /// Sends a request and returns everything the bridge answers.
    async fn fetch(&self, request: &str) -> String {
        let mut stream = self.connect().await;
        stream.write_all(request.as_bytes()).await.unwrap();
        let mut answer = Vec::new();
        // The bridge closes after one answer. Some ends are less tidy than
        // others: what arrived before is what counts.
        let _ = timeout(PATIENCE, stream.read_to_end(&mut answer)).await.unwrap();
        String::from_utf8_lossy(&answer).into_owned()
    }

    /// Opens the WebSocket as a page of this bridge would.
    async fn open(&self) -> Browser {
        let host = self.host();
        let mut request = format!("wss://{host}/link").into_client_request().unwrap();
        request.headers_mut().insert("Origin", format!("https://{host}").parse().unwrap());
        let (socket, _) = client_async(request, self.connect().await).await.unwrap();
        Browser { socket }
    }

    /// A browser the bridge already let in.
    async fn welcomed(&self) -> Browser {
        let mut browser = self.open().await;
        browser.say(&Up::Code(self.bridge.code())).await;
        let welcome = browser.hear().await;
        assert!(matches!(welcome, Down::Welcome { key: Some(_), .. }), "{welcome:?}");
        browser
    }
}

impl Drop for Setup {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.folder);
    }
}

/// What a browser does on its WebSocket.
struct Browser {
    socket: WebSocketStream<TlsStream<TcpStream>>,
}

/// One message from the bridge.
enum Heard {
    Text(Down),
    Binary(Vec<u8>),
}

impl Browser {
    async fn say(&mut self, message: &Up) {
        let text = serde_json::to_string(message).unwrap();
        self.socket.send(Message::text(text)).await.unwrap();
    }

    async fn next(&mut self) -> Heard {
        loop {
            let message = timeout(PATIENCE, self.socket.next())
                .await
                .expect("the bridge said nothing")
                .expect("the bridge hung up")
                .unwrap();
            match message {
                Message::Text(text) => return Heard::Text(serde_json::from_str(&text).unwrap()),
                Message::Binary(bytes) => return Heard::Binary(bytes.to_vec()),
                _ => {}
            }
        }
    }

    /// The next text message, skipping pictures and sound.
    async fn hear(&mut self) -> Down {
        loop {
            if let Heard::Text(down) = self.next().await {
                return down;
            }
        }
    }

    /// The next piece of news `wanted` picks, skipping everything else.
    async fn news<T>(&mut self, mut wanted: impl FnMut(Event) -> Option<T>) -> T {
        loop {
            if let Down::News(event) = self.hear().await
                && let Some(found) = wanted(event)
            {
                return found;
            }
        }
    }

    /// Adds the stand-in host and pairs with it, as a person would.
    async fn pair(&mut self, host: &FakeHost) -> HostId {
        self.say(&Up::Ask(Request::Add(host.address()))).await;
        let added = self
            .news(|event| match event {
                Event::Added(result) => Some(result.unwrap()),
                _ => None,
            })
            .await;
        self.say(&Up::Ask(Request::Pair { host: added.id.clone(), attempt: 1 })).await;
        let pin = self
            .news(|event| match event {
                Event::PinChosen { attempt: 1, pin } => Some(pin),
                _ => None,
            })
            .await;
        host.type_pin(pin);
        self.news(|event| match event {
            Event::PairingEnded { attempt: 1, result } => Some(result.unwrap()),
            _ => None,
        })
        .await;
        added.id
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn the_web_client_is_served_over_https() {
    let setup = Setup::new();
    let host = setup.host();

    let page = setup.fetch(&format!("GET / HTTP/1.1\r\nHost: {host}\r\n\r\n")).await;
    assert!(page.starts_with("HTTP/1.1 200 OK\r\n"), "{page}");
    assert!(page.contains("Content-Type: text/html; charset=utf-8\r\n"));
    assert!(page.ends_with("<!doctype html><title>Leto</title>"));

    let head = setup.fetch(&format!("HEAD /index.html HTTP/1.1\r\nHost: {host}\r\n\r\n")).await;
    assert!(head.starts_with("HTTP/1.1 200 OK\r\n") && head.ends_with("\r\n\r\n"), "{head}");

    let missing = setup.fetch(&format!("GET /missing.js HTTP/1.1\r\nHost: {host}\r\n\r\n")).await;
    assert!(missing.starts_with("HTTP/1.1 404 "), "{missing}");

    // Nothing outside the web client's folder, however the path is bent.
    for path in ["/../bridge/browsers.json", "/..%2Fbridge%2Fcertificate.json", "/web/index.html"] {
        let answer = setup.fetch(&format!("GET {path} HTTP/1.1\r\nHost: {host}\r\n\r\n")).await;
        assert!(answer.starts_with("HTTP/1.1 404 "), "{path}: {answer}");
    }

    let post = setup.fetch(&format!("POST / HTTP/1.1\r\nHost: {host}\r\n\r\n")).await;
    assert!(post.starts_with("HTTP/1.1 405 "), "{post}");
}

#[tokio::test(flavor = "multi_thread")]
async fn an_address_typed_without_https_is_sent_to_the_right_door() {
    let setup = Setup::new();
    let mut stream = TcpStream::connect(("127.0.0.1", setup.bridge.port())).await.unwrap();
    let request = format!("GET /somewhere HTTP/1.1\r\nHost: {}\r\n\r\n", setup.host());
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut answer = String::new();
    timeout(PATIENCE, stream.read_to_string(&mut answer)).await.unwrap().unwrap();
    assert!(answer.starts_with("HTTP/1.1 308 "), "{answer}");
    assert!(answer.contains(&format!("Location: https://{}/\r\n", setup.host())), "{answer}");
}

#[tokio::test(flavor = "multi_thread")]
async fn only_a_page_of_the_bridge_may_open_the_websocket() {
    let setup = Setup::new();
    let host = setup.host();
    let upgrade = |origin: &str| {
        format!(
            "GET /link HTTP/1.1\r\nHost: {host}\r\nConnection: Upgrade\r\nUpgrade: websocket\r\n\
             Sec-WebSocket-Version: 13\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n{origin}\r\n"
        )
    };
    let foreign = setup.fetch(&upgrade("Origin: https://evil.example\r\n")).await;
    assert!(foreign.starts_with("HTTP/1.1 403 "), "{foreign}");
    let nobody = setup.fetch(&upgrade("")).await;
    assert!(nobody.starts_with("HTTP/1.1 403 "), "{nobody}");
    let ordinary = setup.fetch(&format!("GET /link HTTP/1.1\r\nHost: {host}\r\n\r\n")).await;
    assert!(ordinary.starts_with("HTTP/1.1 400 "), "{ordinary}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_browser_is_let_in_with_the_code_then_with_its_key() {
    let setup = Setup::new();
    let code = setup.bridge.code();

    let mut browser = setup.open().await;
    browser.say(&Up::Hello { key: None }).await;
    assert_eq!(browser.hear().await, Down::CodeNeeded { refused: false });

    // Asking before being let in gets the same answer, and no work.
    browser.say(&Up::Ask(Request::Search)).await;
    assert_eq!(browser.hear().await, Down::CodeNeeded { refused: false });
    browser.say(&Up::Hello { key: Some("made-up".into()) }).await;
    assert_eq!(browser.hear().await, Down::CodeNeeded { refused: false });

    browser.say(&Up::Code(code.clone())).await;
    let Down::Welcome { key: Some(key), name } = browser.hear().await else {
        panic!("the right code was refused")
    };
    assert_eq!(name, leto_bridge::computer_name());
    // The code is spent: the next browser needs the new one.
    assert_eq!(*setup.codes.lock().unwrap(), [setup.bridge.code()]);
    assert_ne!(setup.bridge.code(), code);

    // Now it may ask.
    browser.say(&Up::Ask(Request::Add("not an address".into()))).await;
    let refused = browser
        .news(|event| match event {
            Event::Added(result) => Some(result.unwrap_err()),
            _ => None,
        })
        .await;
    assert!(refused.ends_with('.'), "{refused}");

    // Later, the key is enough, and no new one is handed out.
    let mut again = setup.open().await;
    again.say(&Up::Hello { key: Some(key) }).await;
    assert!(matches!(again.hear().await, Down::Welcome { key: None, .. }));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_wrong_code_is_refused_slowly_and_nothing_is_looked_at_meanwhile() {
    let setup = Setup::new();
    let code = setup.bridge.code();
    let wrong = if code.starts_with('0') { "10000000" } else { "00000000" };

    let mut browser = setup.open().await;
    let start = Instant::now();
    browser.say(&Up::Code(wrong.to_owned())).await;
    // Right behind it, even the right code is not looked at.
    browser.say(&Up::Code(code.clone())).await;
    assert_eq!(browser.hear().await, Down::CodeNeeded { refused: true });
    assert!(start.elapsed() >= Duration::from_millis(1900), "the refusal came at once");
    assert_eq!(browser.hear().await, Down::CodeNeeded { refused: true });

    // Whoever read the refusals is listened to again.
    browser.say(&Up::Code(code)).await;
    assert!(matches!(browser.hear().await, Down::Welcome { key: Some(_), .. }));
}

#[tokio::test(flavor = "multi_thread")]
async fn what_is_not_a_message_ends_the_link() {
    let setup = Setup::new();
    let mut browser = setup.open().await;
    browser.socket.send(Message::text("{\"Ask\":\"Everything\"}")).await.unwrap();
    let ended = timeout(PATIENCE, async {
        loop {
            match browser.socket.next().await {
                None | Some(Err(_)) | Some(Ok(Message::Close(_))) => break,
                Some(Ok(_)) => {}
            }
        }
    })
    .await;
    assert!(ended.is_ok(), "the bridge kept listening");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_pc_is_paired_and_its_apps_listed_through_the_bridge() {
    let setup = Setup::new();
    let host = FakeHost::start();
    let mut browser = setup.welcomed().await;
    let id = browser.pair(&host).await;

    browser.say(&Up::Ask(Request::Apps(id.clone()))).await;
    let apps = browser
        .news(|event| match event {
            Event::Apps { host, result } if host == id => Some(result.unwrap()),
            _ => None,
        })
        .await;
    assert!(!apps.is_empty());
    for app in &apps {
        let about = browser
            .news(|event| match event {
                Event::Cover { host, app, .. } if host == id => Some(app),
                _ => None,
            })
            .await;
        assert_eq!(about, app.id);
    }

    // Another browser of the same bridge finds the PC paired already: the
    // bridge is one device to the PC.
    let mut other = setup.welcomed().await;
    other.say(&Up::Ask(Request::Search)).await;
    let known = other
        .news(|event| match event {
            Event::Hosts(hosts) => hosts.into_iter().find(|host| host.id == id),
            _ => None,
        })
        .await;
    assert!(known.paired);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_stream_reaches_the_browser_and_input_reaches_the_pc() {
    let setup = Setup::new();
    let host = FakeHost::start();
    let mut browser = setup.welcomed().await;
    let id = browser.pair(&host).await;

    browser.say(&Up::Ask(Request::Play { host: id, app: 1, attempt: 5 })).await;

    // In whatever order they come: the news that it plays, what the
    // pictures and the sound are made of, then pictures and sound.
    let (mut playing, mut pictures, mut sound) = (false, None, None);
    let (mut first_picture, mut sounds) = (None, 0);
    while !(playing && first_picture.is_some() && sounds >= 5) {
        match browser.next().await {
            Heard::Text(Down::News(Event::Playing { attempt: 5, .. })) => playing = true,
            Heard::Text(Down::News(Event::PlayFailed { message, .. })) => panic!("{message}"),
            Heard::Text(Down::Pictures(format)) => pictures = Some(format),
            Heard::Text(Down::Sound(format)) => sound = Some(format.unwrap()),
            Heard::Text(_) => {}
            Heard::Binary(message) => match Frame::parse(&message).unwrap() {
                Frame::Picture { keyframe, data, .. } => {
                    // The format is said before the first picture arrives.
                    assert!(pictures.is_some(), "a picture came before its format");
                    if first_picture.is_none() {
                        assert!(keyframe, "the first picture is not a keyframe");
                        assert!(data.starts_with(&[0, 0, 0, 1]) || data.starts_with(&[0, 0, 1]));
                        first_picture = Some(data.len());
                    }
                }
                Frame::Sound(packet) => {
                    assert!(sound.is_some(), "sound came before its format");
                    assert!(!packet.is_empty());
                    sounds += 1;
                }
                Frame::Voice(_) => panic!("the bridge sent a microphone down"),
            },
        }
    }
    let pictures = pictures.unwrap();
    assert!(pictures.codec.starts_with("avc1."), "{pictures:?}");
    assert_eq!((pictures.width, pictures.height, pictures.fps), (1920, 1080, 60));
    let sound = sound.unwrap();
    assert_eq!((sound.sample_rate, sound.channels), (48_000, 2));

    // What the person does reaches the PC.
    let key =
        Input::Key { key: Key::letter('L').unwrap(), down: true, modifiers: Modifiers::default() };
    browser.say(&Up::Input(vec![key, Input::MouseMove { dx: 3, dy: -4 }])).await;
    let start = Instant::now();
    while host.inputs().len() < 2 {
        assert!(start.elapsed() < PATIENCE, "the input never reached the host");
        sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(host.inputs(), [key, Input::MouseMove { dx: 3, dy: -4 }]);

    // So does the microphone, packet by packet.
    let voice = Frame::Voice(&quiet_voice()).encode();
    let start = Instant::now();
    while host.stream_report().is_none_or(|report| report.microphone_packets < 3) {
        assert!(start.elapsed() < PATIENCE, "the microphone never reached the host");
        browser.socket.send(Message::binary(voice.clone())).await.unwrap();
        sleep(Duration::from_millis(10)).await;
    }

    // A browser that lost a picture gets a fresh keyframe. The engine asks
    // for some of its own while a stream starts: wait until those are
    // behind, so that the one seen next is the one asked for here.
    let asked_before = timeout(PATIENCE, async {
        loop {
            let asked = host.stream_report().unwrap().keyframes_requested;
            let mut quiet = 0;
            while quiet < 30 {
                if let Heard::Binary(message) = browser.next().await
                    && let Some(Frame::Picture { keyframe, .. }) = Frame::parse(&message)
                {
                    if keyframe {
                        break;
                    }
                    quiet += 1;
                }
            }
            if quiet == 30 && host.stream_report().unwrap().keyframes_requested == asked {
                break asked;
            }
        }
    })
    .await
    .expect("the stream never settled");
    browser.say(&Up::KeyframeNeeded).await;
    let fresh = timeout(PATIENCE, async {
        loop {
            if let Heard::Binary(message) = browser.next().await
                && let Some(Frame::Picture { keyframe: true, .. }) = Frame::parse(&message)
            {
                break;
            }
        }
    })
    .await;
    assert!(fresh.is_ok(), "no keyframe came");
    assert!(host.stream_report().unwrap().keyframes_requested > asked_before);

    // The browser closes its tab: the stream is left, the app keeps running.
    drop(browser);
    let start = Instant::now();
    while !host.stream_report().is_some_and(|report| report.client_left) {
        assert!(start.elapsed() < PATIENCE, "the stream was never left");
        sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(host.running_app(), Some(1));
}
