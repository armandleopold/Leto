//! A stand-in for Apollo's own pages: what a browser, or a Leto that
//! shares the PC beside Apollo, asks to list the devices and to change
//! what each may do.
//!
//! It answers as Apollo does (`confighttp.cpp`, commit `adc5c5a`) on the
//! port after the stand-in host's, from the folder
//! [`Host::write_apollo_folder`](crate::Host::write_apollo_folder) wrote:
//! it shows the certificate kept there, and reads and rewrites the devices
//! of `sunshine_state.json`, as Apollo rewrites its own file. One sign-in
//! at a time, as on Apollo.

use std::{
    fs,
    io::{self, Read, Write},
    net::{TcpListener, TcpStream},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    thread,
    time::Duration,
};

use leto_host::apollo::{Apollo, EVERY_RIGHT};
use rustls::{
    ServerConfig, ServerConnection, StreamOwned,
    pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs1KeyDer, PrivatePkcs8KeyDer},
};
use serde_json::{Value, json};

/// What the pages keep between two requests.
struct Kept {
    folder: PathBuf,
    name: String,
    password: String,
    /// What the one signed in shows with every request.
    pass: Mutex<Option<String>>,
    signed_in: AtomicU64,
    /// The devices that are streaming, and those whose stream was ended
    /// from the pages.
    connected: Mutex<Vec<String>>,
    disconnected: Mutex<Vec<String>>,
    /// The PIN a device is showing, and the PINs and names that were typed.
    awaited: Mutex<Option<String>>,
    typed: Mutex<Vec<(String, String)>>,
}

/// Apollo's pages, for as long as it lives.
pub struct ApolloPages {
    port: u16,
    kept: Arc<Kept>,
    stop: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl ApolloPages {
    /// Serves the pages of the Apollo whose settings are in `folder`, on
    /// the port after its own, for whoever signs in with `name` and
    /// `password`. Fails when that port is taken.
    pub fn start(folder: &Path, name: &str, password: &str) -> io::Result<Self> {
        let apollo = Apollo::read(folder).map_err(io::Error::other)?;
        let tls = Arc::new(tls(folder)?);
        let listener = TcpListener::bind(("127.0.0.1", apollo.pages_port()))?;
        listener.set_nonblocking(true)?;
        let kept = Arc::new(Kept {
            folder: folder.to_owned(),
            name: name.to_owned(),
            password: password.to_owned(),
            pass: Mutex::new(None),
            signed_in: AtomicU64::new(0),
            connected: Mutex::new(Vec::new()),
            disconnected: Mutex::new(Vec::new()),
            awaited: Mutex::new(None),
            typed: Mutex::new(Vec::new()),
        });
        let stop = Arc::new(AtomicBool::new(false));
        let thread = thread::spawn({
            let (kept, stop) = (Arc::clone(&kept), Arc::clone(&stop));
            move || {
                while !stop.load(Ordering::SeqCst) {
                    match listener.accept() {
                        Ok((stream, _)) => {
                            let _ = answer(stream, &tls, &kept);
                        }
                        Err(_) => thread::sleep(Duration::from_millis(5)),
                    }
                }
            }
        });
        Ok(Self { port: apollo.pages_port(), kept, stop, thread: Some(thread) })
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    /// How many times someone signed in.
    pub fn signed_in(&self) -> u64 {
        self.kept.signed_in.load(Ordering::SeqCst)
    }

    /// Forgets who signed in, as Apollo does when someone signs in from a
    /// browser, or when it starts again.
    pub fn sign_out(&self) {
        *lock(&self.kept.pass) = None;
    }

    /// The device Apollo knows as `id` is streaming from now on.
    pub fn connect(&self, id: &str) {
        lock(&self.kept.connected).push(id.to_owned());
    }

    /// The devices whose stream was ended from the pages.
    pub fn disconnected(&self) -> Vec<String> {
        lock(&self.kept.disconnected).clone()
    }

    /// A device is showing `pin` and waits for it to be typed here.
    pub fn await_pin(&self, pin: &str) {
        *lock(&self.kept.awaited) = Some(pin.to_owned());
    }

    /// The PINs that were typed, each with the name given to its device.
    pub fn typed(&self) -> Vec<(String, String)> {
        lock(&self.kept.typed).clone()
    }
}

impl Drop for ApolloPages {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Apollo's certificate and key, shown to whoever opens the pages.
fn tls(folder: &Path) -> io::Result<ServerConfig> {
    let read = |file: &str| {
        let text = fs::read(folder.join("credentials").join(file))?;
        pem::parse(text).map_err(io::Error::other)
    };
    let certificate = read("cacert.pem")?;
    let key = read("cakey.pem")?;
    let key = match key.tag() {
        "RSA PRIVATE KEY" => PrivateKeyDer::Pkcs1(PrivatePkcs1KeyDer::from(key.into_contents())),
        _ => PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key.into_contents())),
    };
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(io::Error::other)?
        .with_no_client_auth()
        .with_single_cert(vec![CertificateDer::from(certificate.into_contents())], key)
        .map_err(io::Error::other)
}

/// One request, answered as Apollo answers it.
fn answer(stream: TcpStream, tls: &Arc<ServerConfig>, kept: &Kept) -> io::Result<()> {
    stream.set_nonblocking(false)?;
    stream.set_read_timeout(Some(Duration::from_secs(2)))?;
    let connection = ServerConnection::new(Arc::clone(tls)).map_err(io::Error::other)?;
    let mut stream = StreamOwned::new(connection, stream);

    let mut bytes = Vec::new();
    let mut piece = [0u8; 4096];
    let (lines, body) = loop {
        let read = stream.read(&mut piece)?;
        if read == 0 {
            return Ok(());
        }
        bytes.extend_from_slice(&piece[..read]);
        let Some(end) = bytes.windows(4).position(|window| window == b"\r\n\r\n") else {
            continue;
        };
        let lines = String::from_utf8_lossy(&bytes[..end]).into_owned();
        let length = header(&lines, "content-length").and_then(|length| length.parse().ok());
        let length: usize = length.unwrap_or(0);
        if bytes.len() >= end + 4 + length {
            break (lines, bytes[end + 4..end + 4 + length].to_vec());
        }
    };
    let mut first = lines.lines().next().unwrap_or("").split_whitespace();
    let (method, path) = (first.next().unwrap_or(""), first.next().unwrap_or(""));
    let json_sent = header(&lines, "content-type")
        .is_some_and(|kind| kind.split(';').next().unwrap_or("").trim() == "application/json");
    let shown = header(&lines, "cookie").and_then(|cookies| {
        cookies.split(';').find_map(|cookie| cookie.trim().strip_prefix("auth=").map(str::to_owned))
    });
    let signed_in = shown.is_some() && *lock(&kept.pass) == shown;
    let asked: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
    let text = |field: &str| asked.get(field).and_then(Value::as_str).unwrap_or("").to_owned();

    let (status, pass, said) = match (method, path) {
        ("POST", "/api/login") if !json_sent => (400, None, refusal(400, "Content type mismatch")),
        ("POST", "/api/login") => {
            if text("username").eq_ignore_ascii_case(&kept.name)
                && text("password") == kept.password
            {
                // A new sign-in takes the place of the one before.
                let number = kept.signed_in.fetch_add(1, Ordering::SeqCst);
                let pass = format!("stand-in-pass-{number}-{}", std::process::id());
                *lock(&kept.pass) = Some(pass.clone());
                (200, Some(pass), String::new())
            } else {
                (401, None, String::new())
            }
        }
        (_, path) if path.starts_with("/api/") && !signed_in => {
            (401, None, refusal(401, "Unauthorized"))
        }
        ("POST", _) if !json_sent => (400, None, refusal(400, "Content type mismatch")),
        ("GET", "/api/clients/list") => {
            let mut connected = lock(&kept.connected).clone();
            let listed: Vec<Value> = devices(&kept.folder)?
                .into_iter()
                .map(|mut device| {
                    let id = device["uuid"].as_str().unwrap_or("").to_owned();
                    if let Some(device) = device.as_object_mut() {
                        device.remove("cert");
                        device.entry("perm").or_insert(EVERY_RIGHT.into());
                        device.entry("display_mode").or_insert("".into());
                        let streaming = connected.iter().position(|streaming| *streaming == id);
                        device.insert("connected".to_owned(), streaming.is_some().into());
                        if let Some(streaming) = streaming {
                            connected.remove(streaming);
                        }
                    }
                    device
                })
                .collect();
            let said = json!({ "named_certs": listed, "platform": "windows", "status": true });
            (200, None, said.to_string())
        }
        ("POST", "/api/clients/update") => {
            let mut known = devices(&kept.folder)?;
            let found = known.iter_mut().find(|device| device["uuid"] == asked["uuid"]);
            let done = found.is_some();
            if let Some(device) = found {
                // What is not said goes back to what Apollo starts from.
                device["name"] = text("name").into();
                device["display_mode"] = text("display_mode").into();
                let permissions = asked.get("perm").and_then(Value::as_u64).unwrap_or(0) as u32;
                device["perm"] = (permissions & EVERY_RIGHT).into();
                for (flag, unsaid) in [
                    ("enable_legacy_ordering", true),
                    ("allow_client_commands", true),
                    ("always_use_virtual_display", false),
                ] {
                    device[flag] =
                        asked.get(flag).and_then(Value::as_bool).unwrap_or(unsaid).into();
                }
                for commands in ["do", "undo"] {
                    match asked.get(commands) {
                        Some(said) => device[commands] = said.clone(),
                        None => {
                            device.as_object_mut().map(|device| device.remove(commands));
                        }
                    }
                }
                keep(&kept.folder, &known)?;
            }
            (200, None, json!({ "status": done }).to_string())
        }
        ("POST", "/api/clients/unpair") => {
            let mut known = devices(&kept.folder)?;
            let before = known.len();
            known.retain(|device| device["uuid"] != asked["uuid"]);
            keep(&kept.folder, &known)?;
            (200, None, json!({ "status": known.len() < before }).to_string())
        }
        ("POST", "/api/clients/disconnect") => {
            let id = text("uuid");
            let mut connected = lock(&kept.connected);
            let streaming = connected.iter().position(|streaming| *streaming == id);
            if let Some(streaming) = streaming {
                connected.remove(streaming);
                lock(&kept.disconnected).push(id);
            }
            (200, None, json!({ "status": streaming.is_some() }).to_string())
        }
        ("POST", "/api/pin") => {
            let (pin, name) = (text("pin"), text("name"));
            let awaited = lock(&kept.awaited).take_if(|awaited| *awaited == pin).is_some();
            lock(&kept.typed).push((pin, name));
            (200, None, json!({ "status": awaited }).to_string())
        }
        _ => (404, None, refusal(404, "Not Found")),
    };

    let reason = match status {
        200 => "OK",
        400 => "Bad Request",
        401 => "Unauthorized",
        _ => "Not Found",
    };
    let mut reply = format!("HTTP/1.1 {status} {reason}\r\nContent-Length: {}\r\n", said.len());
    if !said.is_empty() {
        reply.push_str("Content-Type: application/json\r\n");
    }
    if let Some(pass) = pass {
        reply.push_str(&format!(
            "Set-Cookie: auth={pass}; Secure; SameSite=Strict; Max-Age=2592000; Path=/\r\n"
        ));
    }
    reply.push_str("\r\n");
    reply.push_str(&said);
    stream.write_all(reply.as_bytes())?;
    stream.flush()?;
    stream.conn.send_close_notify();
    let _ = stream.flush();
    Ok(())
}

fn header<'a>(lines: &'a str, wanted: &str) -> Option<&'a str> {
    lines.lines().skip(1).find_map(|line| {
        let (name, value) = line.split_once(':')?;
        name.trim().eq_ignore_ascii_case(wanted).then_some(value.trim())
    })
}

fn refusal(status: u16, error: &str) -> String {
    json!({ "status_code": status, "status": false, "error": error }).to_string()
}

/// The devices of the file Apollo keeps them in.
fn devices(folder: &Path) -> io::Result<Vec<Value>> {
    let state: Value = serde_json::from_slice(&fs::read(folder.join("sunshine_state.json"))?)
        .map_err(io::Error::other)?;
    Ok(state["root"]["named_devices"].as_array().cloned().unwrap_or_default())
}

/// Writes the devices back, and leaves the rest of the file as it is.
fn keep(folder: &Path, devices: &[Value]) -> io::Result<()> {
    let file = folder.join("sunshine_state.json");
    let mut state: Value = serde_json::from_slice(&fs::read(&file)?).map_err(io::Error::other)?;
    state["root"]["named_devices"] = devices.into();
    fs::write(file, state.to_string())
}
