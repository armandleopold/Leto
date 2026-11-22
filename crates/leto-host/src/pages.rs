//! Apollo's own pages, asked to change what Leto never writes.
//!
//! Leto reads Apollo's folder and leaves it alone (see `apollo.rs`). What
//! the owner of the PC changes about a device, Apollo changes itself: Leto
//! asks it the way its own pages do, on the port after Apollo's, once the
//! owner has signed in with the name and the password of those pages. So
//! Apollo applies a change at once, to a stream that is running too, and
//! writes its own file.
//!
//! Read in `confighttp.cpp` and `pin.html` of Apollo, at commit `adc5c5a`:
//! `/api/login`, `/api/clients/list`, `/api/clients/update`,
//! `/api/clients/unpair`, `/api/clients/disconnect` and `/api/pin`.
//!
//! Apollo keeps one sign-in at a time: signing in here signs a browser out
//! of its pages, and the other way round. The password is used to sign in
//! and kept nowhere; what Apollo answers it with is kept in memory, and
//! neither is ever logged.

use std::{
    fmt,
    io::{self, Read, Write},
    net::{Ipv4Addr, SocketAddr, TcpStream},
    sync::Arc,
    time::Duration,
};

use rustls::{
    ClientConfig, ClientConnection, DigitallySignedStruct, SignatureScheme, StreamOwned,
    client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier},
    crypto::{WebPkiSupportedAlgorithms, verify_tls12_signature, verify_tls13_signature},
    pki_types::{CertificateDer, ServerName, UnixTime},
};
use serde_json::{Map, Value, json};

use crate::apollo::{Apollo, EVERY_RIGHT};

/// How long Apollo, on this very computer, may take to answer.
const PATIENCE: Duration = Duration::from_secs(3);

/// No answer of Apollo's pages is longer than this.
const LONGEST_ANSWER: usize = 4 << 20;

/// Why Apollo's pages did not do what was asked.
#[derive(Debug)]
pub enum Error {
    /// Nothing answers where Apollo's pages are.
    Unreachable(io::Error),
    /// The name or the password is not the one of Apollo's pages.
    Refused,
    /// Apollo no longer takes this sign-in: someone signed in elsewhere, or
    /// Apollo started again.
    SignedOut,
    /// Apollo's pages have no name and no password yet.
    NotSetUp,
    /// The pages are those of a Sunshine, which keeps no rights for each
    /// device.
    NotApollo,
    /// Apollo answered something else than its pages do: what.
    Unexpected(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unreachable(error) => write!(f, "Apollo's pages do not answer: {error}"),
            Self::Refused => write!(f, "Apollo does not know this name and this password"),
            Self::SignedOut => write!(f, "Apollo asks to sign in again"),
            Self::NotSetUp => write!(
                f,
                "Apollo's pages have no name and no password yet: open them once in a browser"
            ),
            Self::NotApollo => {
                write!(f, "these pages are not Apollo's: only Apollo says what each device may do")
            }
            Self::Unexpected(what) => write!(f, "Apollo answered unexpectedly: {what}"),
        }
    }
}

impl std::error::Error for Error {}

/// A device as Apollo's pages list it.
#[derive(Debug, Clone, PartialEq)]
pub struct Client {
    /// What Apollo knows it by.
    pub id: String,
    pub name: String,
    /// What Apollo lets it do: see [`crate::apollo::RIGHTS`].
    pub permissions: u32,
    /// It is streaming at this moment.
    pub connected: bool,
    /// All that Apollo said of it, to say back with a change: Apollo
    /// forgets what a change leaves out.
    said: Map<String, Value>,
}

impl Client {
    fn read(said: &Value) -> Option<Self> {
        let said = said.as_object()?.clone();
        let text = |field: &str| said.get(field).and_then(Value::as_str).unwrap_or("").to_owned();
        Some(Self {
            id: text("uuid"),
            name: text("name"),
            permissions: said
                .get("perm")
                .and_then(Value::as_u64)
                .map_or(EVERY_RIGHT, |permissions| permissions as u32),
            connected: said.get("connected").and_then(Value::as_bool).unwrap_or(false),
            said,
        })
    }

    /// What Apollo is told to keep of this device.
    fn told(&self) -> Value {
        let mut told = self.said.clone();
        told.remove("connected");
        told.insert("uuid".to_owned(), self.id.clone().into());
        told.insert("name".to_owned(), self.name.clone().into());
        told.insert("perm".to_owned(), (self.permissions & EVERY_RIGHT).into());
        Value::Object(told)
    }
}

/// Apollo's pages, signed in to.
pub struct Pages {
    at: SocketAddr,
    tls: Arc<ClientConfig>,
    /// What Apollo gave at sign-in, to show with every request.
    pass: String,
}

impl fmt::Debug for Pages {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Never what Apollo gave at sign-in.
        f.debug_struct("Pages").field("at", &self.at).finish_non_exhaustive()
    }
}

impl Pages {
    /// Signs in to the pages of `apollo` with their `name` and `password`.
    pub fn sign_in(apollo: &Apollo, name: &str, password: &str) -> Result<Self, Error> {
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let only = OnlyApollo {
            certificate: apollo.certificate().to_vec(),
            algorithms: provider.signature_verification_algorithms,
        };
        let tls = ClientConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .map_err(|error| Error::Unexpected(error.to_string()))?
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(only))
            .with_no_client_auth();
        let mut pages = Self {
            at: SocketAddr::from((Ipv4Addr::LOCALHOST, apollo.pages_port())),
            tls: Arc::new(tls),
            pass: String::new(),
        };
        let answer = pages.ask(
            "POST",
            "/api/login",
            Some(&json!({ "username": name, "password": password })),
        )?;
        match answer.status {
            200 => {}
            // A Sunshine has no such sign-in: it asks for the password the
            // way a browser gives it, or knows no such page.
            401 if answer.header("www-authenticate").is_some() => return Err(Error::NotApollo),
            404 | 405 => return Err(Error::NotApollo),
            401 => return Err(Error::Refused),
            // Pages nobody set up send whoever asks to do so.
            300..=399 => return Err(Error::NotSetUp),
            status => return Err(Error::Unexpected(format!("status {status} at sign-in"))),
        }
        pages.pass = answer
            .headers
            .iter()
            .filter(|(name, _)| name.eq_ignore_ascii_case("set-cookie"))
            .find_map(|(_, value)| value.split(';').next()?.trim().strip_prefix("auth="))
            .filter(|pass| !pass.is_empty())
            .ok_or_else(|| Error::Unexpected("nothing to sign in with".to_owned()))?
            .to_owned();
        Ok(pages)
    }

    /// The devices Apollo is paired with, as it knows them at this moment.
    pub fn clients(&self) -> Result<Vec<Client>, Error> {
        let said = self.said("GET", "/api/clients/list", None)?;
        let listed = said
            .get("named_certs")
            .and_then(Value::as_array)
            .ok_or_else(|| Error::Unexpected("no list of devices".to_owned()))?;
        Ok(listed.iter().filter_map(Client::read).collect())
    }

    /// Has Apollo keep `client` as it is here: its name and what it may do.
    /// Apollo applies it at once, to a stream of that device too.
    pub fn update(&self, client: &Client) -> Result<(), Error> {
        self.done("/api/clients/update", &client.told())
    }

    /// Has Apollo forget the device it knows as `id`.
    pub fn unpair(&self, id: &str) -> Result<(), Error> {
        self.done("/api/clients/unpair", &json!({ "uuid": id }))
    }

    /// Has Apollo end the stream of the device it knows as `id`.
    pub fn disconnect(&self, id: &str) -> Result<(), Error> {
        self.done("/api/clients/disconnect", &json!({ "uuid": id }))
    }

    /// Gives Apollo the PIN a device shows, and a name for that device.
    /// Whether Apollo was waiting for that PIN.
    pub fn pair(&self, pin: &str, name: &str) -> Result<bool, Error> {
        let said = self.said("POST", "/api/pin", Some(&json!({ "pin": pin, "name": name })))?;
        Ok(said.get("status").and_then(Value::as_bool).unwrap_or(false))
    }

    /// Asks for something Apollo answers with whether it did it.
    fn done(&self, path: &str, body: &Value) -> Result<(), Error> {
        let said = self.said("POST", path, Some(body))?;
        if said.get("status").and_then(Value::as_bool).unwrap_or(false) {
            Ok(())
        } else {
            Err(Error::Unexpected("Apollo did not do it".to_owned()))
        }
    }

    /// Asks, signed in, and reads what Apollo says.
    fn said(&self, method: &str, path: &str, body: Option<&Value>) -> Result<Value, Error> {
        let answer = self.ask(method, path, body)?;
        match answer.status {
            200 => serde_json::from_slice(&answer.body)
                .map_err(|_| Error::Unexpected(format!("{path} did not answer in JSON"))),
            401 => Err(Error::SignedOut),
            300..=399 => Err(Error::NotSetUp),
            status => Err(Error::Unexpected(format!("status {status} from {path}"))),
        }
    }

    /// One request to Apollo's pages, on a connection of its own.
    fn ask(&self, method: &str, path: &str, body: Option<&Value>) -> Result<Answer, Error> {
        let unreachable = Error::Unreachable;
        let stream = TcpStream::connect_timeout(&self.at, PATIENCE).map_err(unreachable)?;
        stream.set_read_timeout(Some(PATIENCE)).map_err(unreachable)?;
        stream.set_write_timeout(Some(PATIENCE)).map_err(unreachable)?;
        let name = ServerName::try_from("localhost")
            .map_err(|error| Error::Unexpected(error.to_string()))?;
        let connection = ClientConnection::new(Arc::clone(&self.tls), name)
            .map_err(|error| Error::Unexpected(error.to_string()))?;
        let mut stream = StreamOwned::new(connection, stream);

        let body = body.map(Value::to_string).unwrap_or_default();
        let mut request = format!("{method} {path} HTTP/1.1\r\nHost: localhost\r\n");
        if !self.pass.is_empty() {
            request.push_str(&format!("Cookie: auth={}\r\n", self.pass));
        }
        if method != "GET" {
            request.push_str("Content-Type: application/json\r\n");
        }
        request.push_str(&format!(
            "Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        ));
        stream.write_all(request.as_bytes()).map_err(unreachable)?;
        stream.flush().map_err(unreachable)?;
        Answer::read(&mut stream)
    }
}

/// What Apollo's pages answered.
struct Answer {
    status: u16,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl Answer {
    /// What the answer says under `name`, when it says anything.
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(known, _)| known.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }

    fn read(stream: &mut impl Read) -> Result<Self, Error> {
        let mut bytes = Vec::new();
        let mut piece = [0u8; 4096];
        // The lines before the body, then as much body as they announce.
        let mut body_at = None;
        let mut wanted = None;
        loop {
            if let (Some(at), Some(wanted)) = (body_at, wanted)
                && bytes.len() >= at + wanted
            {
                bytes.truncate(at + wanted);
                break;
            }
            let read = match stream.read(&mut piece) {
                Ok(0) => break,
                Ok(read) => read,
                // Apollo hangs up without a word once it has answered.
                Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => break,
                Err(error) => return Err(Error::Unreachable(error)),
            };
            bytes.extend_from_slice(&piece[..read]);
            if bytes.len() > LONGEST_ANSWER {
                return Err(Error::Unexpected("an answer without end".to_owned()));
            }
            if body_at.is_none()
                && let Some(end) = bytes.windows(4).position(|window| window == b"\r\n\r\n")
            {
                body_at = Some(end + 4);
                let lines = String::from_utf8_lossy(&bytes[..end]);
                wanted = lines.lines().skip(1).find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.trim().eq_ignore_ascii_case("content-length").then_some(())?;
                    value.trim().parse::<usize>().ok()
                });
            }
        }
        let body_at = body_at.ok_or_else(|| Error::Unexpected("an answer cut short".to_owned()))?;
        let lines = String::from_utf8_lossy(&bytes[..body_at]).into_owned();
        let mut lines = lines.lines();
        let status = lines
            .next()
            .and_then(|line| line.split_whitespace().nth(1))
            .and_then(|status| status.parse().ok())
            .ok_or_else(|| Error::Unexpected("an answer that is not HTTP".to_owned()))?;
        let headers = lines
            .filter_map(|line| line.split_once(':'))
            .map(|(name, value)| (name.trim().to_owned(), value.trim().to_owned()))
            .collect();
        Ok(Self { status, headers, body: bytes[body_at..].to_vec() })
    }
}

/// Only the Apollo whose folder was read is spoken to: whoever answers on
/// its port has to show the very certificate that is in that folder.
#[derive(Debug)]
struct OnlyApollo {
    certificate: Vec<u8>,
    algorithms: WebPkiSupportedAlgorithms,
}

impl ServerCertVerifier for OnlyApollo {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        if end_entity.as_ref() == self.certificate.as_slice() {
            Ok(ServerCertVerified::assertion())
        } else {
            Err(rustls::Error::General("not the certificate of this PC's Apollo".to_owned()))
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls12_signature(message, cert, dss, &self.algorithms)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls13_signature(message, cert, dss, &self.algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.algorithms.supported_schemes()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn an_answer_is_read_up_to_the_length_it_announces() {
        let mut said: &[u8] = b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\
            content-length: 15\r\nSet-Cookie: auth=abc; Secure\r\n\r\n{\"status\":true}extra";
        let answer = Answer::read(&mut said).unwrap();
        assert_eq!(answer.status, 200);
        assert_eq!(answer.body, b"{\"status\":true}");
        assert!(answer.headers.contains(&("Set-Cookie".to_owned(), "auth=abc; Secure".to_owned())));

        // No length: all there is, and none is fine too.
        let mut bare: &[u8] = b"HTTP/1.1 401 Unauthorized\r\n\r\n";
        let answer = Answer::read(&mut bare).unwrap();
        assert_eq!((answer.status, answer.body.len()), (401, 0));

        let mut cut: &[u8] = b"HTTP/1.1 200 OK\r\nContent-Le";
        assert!(matches!(Answer::read(&mut cut), Err(Error::Unexpected(_))));
    }

    #[test]
    fn a_change_says_back_all_that_apollo_said_of_a_device() {
        let said = json!({
            "name": "Pixel",
            "uuid": "1234",
            "perm": 0x0300_0000u32,
            "display_mode": "1920x1080x60",
            "enable_legacy_ordering": false,
            "always_use_virtual_display": true,
            "do": [{ "cmd": "start.bat", "elevated": false }],
            "connected": true,
        });
        let mut client = Client::read(&said).unwrap();
        assert_eq!((client.id.as_str(), client.name.as_str()), ("1234", "Pixel"));
        assert!(client.connected);

        client.name = "Phone".to_owned();
        // More than Apollo has a word for is not sent to it.
        client.permissions = u32::MAX;
        let told = client.told();
        assert_eq!(told["name"], "Phone");
        assert_eq!(told["perm"], EVERY_RIGHT);
        assert_eq!(told["display_mode"], "1920x1080x60");
        assert_eq!(told["enable_legacy_ordering"], false);
        assert_eq!(told["always_use_virtual_display"], true);
        assert_eq!(told["do"][0]["cmd"], "start.bat");
        assert!(told.get("connected").is_none());
    }

    #[test]
    fn a_device_without_a_word_on_what_it_may_do_may_do_everything() {
        let client = Client::read(&json!({ "name": "Old", "uuid": "1" })).unwrap();
        assert_eq!(client.permissions, EVERY_RIGHT);
        assert!(!client.connected);
        assert!(Client::read(&json!("not a device")).is_none());
    }
}
