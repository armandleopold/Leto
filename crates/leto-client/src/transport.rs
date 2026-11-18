//! HTTP and HTTPS requests to a host.
//!
//! The GameStream API is a handful of `GET` requests answered with XML. This
//! module implements exactly that on top of a TCP socket, so it can do the one
//! thing off-the-shelf HTTP clients make hard: **pin the host's certificate**.
//!
//! A host's certificate is self-signed and names nobody, so the usual web
//! checks (a trusted authority, a matching host name) cannot apply. What
//! pairing gives the client instead is the exact certificate of the host. Every
//! later connection must present that same certificate, byte for byte, or it is
//! refused. Without this, anyone on the network could pose as the PC.

use std::{
    io::{self, Read, Write},
    net::{IpAddr, SocketAddr, TcpStream, ToSocketAddrs},
    str::FromStr,
    sync::Arc,
    time::Duration,
};

use moonlight_common::{
    error::Error,
    http::{
        ClientInfo, Endpoint, Request, TextResponse,
        client::{DEFAULT_LONG_TIMEOUT, DEFAULT_TIMEOUT, blocking_client::RequestClient},
    },
};
use pem::Pem;
use rustls::{
    CertificateError, ClientConfig, ClientConnection, DigitallySignedStruct, SignatureScheme,
    StreamOwned,
    client::{
        Resumption,
        danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier},
    },
    crypto::{WebPkiSupportedAlgorithms, verify_tls12_signature, verify_tls13_signature},
    pki_types::{
        CertificateDer, PrivateKeyDer, PrivatePkcs1KeyDer, PrivatePkcs8KeyDer, ServerName, UnixTime,
    },
};

/// Largest response accepted. App lists and box art are far smaller.
const MAX_RESPONSE: usize = 16 * 1024 * 1024;

/// Largest response head (status line and headers) accepted.
const MAX_HEAD: usize = 64 * 1024;

/// Longest wait for a connection to open. A PC that is on answers at once,
/// even far away. One that is off never does.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// Makes requests to one host, with or without the pairing certificates.
#[derive(Clone)]
pub struct Transport {
    timeout: Duration,
    /// Present once the client knows the host's certificate.
    tls: Option<Arc<ClientConfig>>,
}

impl RequestClient for Transport {
    fn with_defaults() -> Result<Self, Error> {
        Ok(Self { timeout: DEFAULT_TIMEOUT, tls: None })
    }

    fn with_defaults_long_timeout() -> Result<Self, Error> {
        Ok(Self { timeout: DEFAULT_LONG_TIMEOUT, tls: None })
    }

    fn with_certificates(
        client_private_key: &Pem,
        client_certificate: &Pem,
        server_certificate: &Pem,
    ) -> Result<Self, Error> {
        let config = tls_config(client_private_key, client_certificate, server_certificate)?;
        Ok(Self { timeout: DEFAULT_TIMEOUT, tls: Some(Arc::new(config)) })
    }

    fn send_http<E>(
        &self,
        client_info: ClientInfo,
        hostport: &str,
        request: &E::Request,
    ) -> Result<E::Response, Error>
    where
        E: Endpoint,
        E::Response: TextResponse,
    {
        let body = self.get(false, hostport, &target::<E>(&client_info, request)?)?;
        parse_text::<E::Response>(body)
    }

    fn send_https<E>(
        &self,
        client_info: ClientInfo,
        hostport: &str,
        request: &E::Request,
    ) -> Result<E::Response, Error>
    where
        E: Endpoint,
        E::Response: TextResponse,
    {
        let body = self.get(true, hostport, &target::<E>(&client_info, request)?)?;
        parse_text::<E::Response>(body)
    }

    fn send_https_with_bytes<E>(
        &self,
        client_info: ClientInfo,
        hostport: &str,
        request: &E::Request,
    ) -> Result<E::Response, Error>
    where
        E: Endpoint<Response = Vec<u8>>,
    {
        self.get(true, hostport, &target::<E>(&client_info, request)?)
    }
}

/// What a PC answered, and the address it answered from.
pub(crate) struct Reply {
    pub body: Vec<u8>,
    pub peer: SocketAddr,
}

impl Transport {
    /// For requests outside the GameStream API to a PC this device is paired
    /// with, each given up on after `timeout`.
    pub(crate) fn paired(
        client_private_key: &Pem,
        client_certificate: &Pem,
        server_certificate: &Pem,
        timeout: Duration,
    ) -> Result<Self, Error> {
        let config = tls_config(client_private_key, client_certificate, server_certificate)?;
        Ok(Self { timeout, tls: Some(Arc::new(config)) })
    }

    /// Sends one request with an optional JSON body to `hostport`, over
    /// HTTPS with the certificates of the pairing and nothing less. An
    /// answer that is not a success is an [`Error::StatusCode`].
    pub(crate) fn request(
        &self,
        method: &str,
        hostport: &str,
        path: &str,
        body: Option<&[u8]>,
    ) -> Result<Reply, Error> {
        // Not the query, as for the GameStream API.
        let shown = path.split('?').next().unwrap_or_default();
        log::debug!("{method} https://{hostport}{shown}");
        self.exchange_request(method, hostport, path, body).inspect_err(|error| {
            log::debug!("{method} https://{hostport}{shown} failed: {error:?}")
        })
    }

    fn exchange_request(
        &self,
        method: &str,
        hostport: &str,
        path: &str,
        body: Option<&[u8]>,
    ) -> Result<Reply, Error> {
        let config = self.tls.clone().ok_or(Error::Unauthenticated)?;
        let tcp = connect(hostport, self.timeout)?;
        let peer = tcp.peer_addr()?;
        let mut request =
            format!("{method} {path} HTTP/1.1\r\nHost: {hostport}\r\nConnection: close\r\n");
        if let Some(body) = body {
            request.push_str("Content-Type: application/json\r\n");
            request.push_str(&format!("Content-Length: {}\r\n", body.len()));
        }
        request.push_str("\r\n");
        let mut request = request.into_bytes();
        request.extend_from_slice(body.unwrap_or_default());

        let connection =
            ClientConnection::new(config, server_name(hostport, &tcp)).map_err(other)?;
        let mut stream = StreamOwned::new(connection, tcp);
        let body = exchange(&mut stream, &request)?;
        Ok(Reply { body, peer })
    }

    /// Sends `GET target` to `hostport` and returns the response body.
    fn get(&self, secure: bool, hostport: &str, target: &str) -> Result<Vec<u8>, Error> {
        // Not the query: while pairing, it carries the challenges.
        let path = target.split('?').next().unwrap_or_default();
        let scheme = if secure { "https" } else { "http" };
        log::debug!("GET {scheme}://{hostport}{path}");
        self.exchange_get(secure, hostport, target)
            .inspect_err(|error| log::debug!("GET {scheme}://{hostport}{path} failed: {error:?}"))
    }

    fn exchange_get(&self, secure: bool, hostport: &str, target: &str) -> Result<Vec<u8>, Error> {
        let tcp = connect(hostport, self.timeout)?;
        let request =
            format!("GET {target} HTTP/1.1\r\nHost: {hostport}\r\nConnection: close\r\n\r\n");

        if secure {
            let config = self.tls.clone().ok_or(Error::Unauthenticated)?;
            let name = server_name(hostport, &tcp);
            let connection = ClientConnection::new(config, name).map_err(other)?;
            let mut stream = StreamOwned::new(connection, tcp);
            exchange(&mut stream, request.as_bytes())
        } else {
            let mut stream = tcp;
            exchange(&mut stream, request.as_bytes())
        }
    }
}

/// Builds the path and query of a request to endpoint `E`.
fn target<E: Endpoint>(client_info: &ClientInfo, request: &E::Request) -> Result<String, Error> {
    let mut query = String::new();
    client_info.append_query_params(&mut query).map_err(other)?;
    request.append_query_params(&mut query).map_err(other)?;
    Ok(format!("{}?{query}", E::path()))
}

fn parse_text<R: TextResponse>(body: Vec<u8>) -> Result<R, Error> {
    let text = String::from_utf8(body).map_err(other)?;
    R::from_str(&text)
}

/// Opens a TCP connection, trying every address the name resolves to.
fn connect(hostport: &str, timeout: Duration) -> Result<TcpStream, Error> {
    let addresses = hostport.to_socket_addrs().map_err(|_| Error::ConnectionFailed)?;
    let mut last = Error::ConnectionFailed;
    for address in addresses {
        match TcpStream::connect_timeout(&address, timeout.min(CONNECT_TIMEOUT)) {
            Ok(stream) => {
                stream.set_read_timeout(Some(timeout))?;
                stream.set_write_timeout(Some(timeout))?;
                stream.set_nodelay(true)?;
                return Ok(stream);
            }
            Err(error) if error.kind() == io::ErrorKind::TimedOut => {
                last = Error::ConnectionTimeout
            }
            Err(_) => last = Error::ConnectionFailed,
        }
    }
    Err(last)
}

/// Writes the request and reads the whole response, returning its body.
fn exchange(stream: &mut (impl Read + Write), request: &[u8]) -> Result<Vec<u8>, Error> {
    stream.write_all(request).map_err(io_error)?;
    stream.flush().map_err(io_error)?;

    // Read until the end of the headers.
    let mut buffer = Vec::with_capacity(4096);
    let head_end = loop {
        if let Some(position) = find(&buffer, b"\r\n\r\n") {
            break position + 4;
        }
        if buffer.len() > MAX_HEAD {
            return Err(invalid("the host sent an oversized response head"));
        }
        if read_some(stream, &mut buffer)? == 0 {
            return Err(Error::ConnectionFailed);
        }
    };

    let mut headers = [httparse::EMPTY_HEADER; 48];
    let mut response = httparse::Response::new(&mut headers);
    match response.parse(&buffer[..head_end]) {
        Ok(httparse::Status::Complete(_)) => {}
        Ok(httparse::Status::Partial) | Err(_) => {
            return Err(invalid("the host sent something that is not HTTP"));
        }
    }
    let code = response.code.unwrap_or_default();
    if !(200..300).contains(&code) {
        return Err(Error::StatusCode {
            code: i32::from(code),
            reason: response.reason.unwrap_or_default().to_owned(),
        });
    }

    let header = |name: &str| {
        response
            .headers
            .iter()
            .find(|header| header.name.eq_ignore_ascii_case(name))
            .and_then(|header| std::str::from_utf8(header.value).ok())
            .map(|value| value.trim().to_owned())
    };
    let chunked = header("transfer-encoding").is_some_and(|v| v.eq_ignore_ascii_case("chunked"));
    let length = match header("content-length") {
        Some(value) => Some(
            value.parse::<usize>().map_err(|_| invalid("the host sent a bad Content-Length"))?,
        ),
        None => None,
    };

    let mut body = buffer.split_off(head_end);
    match (chunked, length) {
        (true, _) => {
            read_to_end(stream, &mut body)?;
            decode_chunked(&body)
        }
        (false, Some(length)) => {
            if length > MAX_RESPONSE {
                return Err(invalid("the host sent an oversized response"));
            }
            while body.len() < length {
                if read_some(stream, &mut body)? == 0 {
                    return Err(Error::ConnectionFailed);
                }
            }
            body.truncate(length);
            Ok(body)
        }
        (false, None) => {
            read_to_end(stream, &mut body)?;
            Ok(body)
        }
    }
}

/// Reads once into `buffer`. Returns the number of bytes read, 0 at the end.
fn read_some(stream: &mut impl Read, buffer: &mut Vec<u8>) -> Result<usize, Error> {
    let mut chunk = [0u8; 8192];
    loop {
        match stream.read(&mut chunk) {
            Ok(read) => {
                buffer.extend_from_slice(&chunk[..read]);
                return Ok(read);
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            // A host that closes without a TLS goodbye. The data read so far
            // is still authenticated, record by record.
            Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => return Ok(0),
            Err(error) => return Err(io_error(error)),
        }
    }
}

fn read_to_end(stream: &mut impl Read, buffer: &mut Vec<u8>) -> Result<(), Error> {
    while read_some(stream, buffer)? != 0 {
        if buffer.len() > MAX_RESPONSE {
            return Err(invalid("the host sent an oversized response"));
        }
    }
    Ok(())
}

/// Decodes a body sent with `Transfer-Encoding: chunked`.
fn decode_chunked(mut data: &[u8]) -> Result<Vec<u8>, Error> {
    let bad = || invalid("the host sent a broken chunked response");
    let mut body = Vec::with_capacity(data.len());
    loop {
        let line_end = find(data, b"\r\n").ok_or_else(bad)?;
        let line = std::str::from_utf8(&data[..line_end]).map_err(|_| bad())?;
        // A chunk size may be followed by extensions after a semicolon.
        let size = line.split(';').next().unwrap_or_default().trim();
        let size = usize::from_str_radix(size, 16).map_err(|_| bad())?;
        data = &data[line_end + 2..];
        if size == 0 {
            return Ok(body);
        }
        if data.len() < size + 2 || &data[size..size + 2] != b"\r\n" {
            return Err(bad());
        }
        body.extend_from_slice(&data[..size]);
        data = &data[size + 2..];
    }
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|window| window == needle)
}

/// The name handed to TLS. It is never checked (the certificate is pinned
/// instead) and never sent, but rustls wants one.
fn server_name(hostport: &str, tcp: &TcpStream) -> ServerName<'static> {
    let host = hostport.rsplit_once(':').map_or(hostport, |(host, _)| host);
    let host = host.trim_start_matches('[').trim_end_matches(']');
    if let Ok(ip) = IpAddr::from_str(host) {
        return ServerName::IpAddress(ip.into());
    }
    if let Ok(name) = ServerName::try_from(host.to_owned()) {
        return name;
    }
    let peer = tcp.peer_addr().map_or(IpAddr::from([0, 0, 0, 0]), |address| address.ip());
    ServerName::IpAddress(peer.into())
}

/// TLS settings for a paired host: present the client's certificate, accept
/// only the host's pinned certificate.
fn tls_config(
    client_private_key: &Pem,
    client_certificate: &Pem,
    server_certificate: &Pem,
) -> Result<ClientConfig, Error> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());

    let key = match client_private_key.tag() {
        "RSA PRIVATE KEY" => {
            PrivateKeyDer::Pkcs1(PrivatePkcs1KeyDer::from(client_private_key.contents().to_vec()))
        }
        _ => PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(client_private_key.contents().to_vec())),
    };
    let chain = vec![CertificateDer::from(client_certificate.contents().to_vec())];

    let verifier = PinnedHost {
        certificate: server_certificate.contents().to_vec(),
        algorithms: provider.signature_verification_algorithms,
    };

    let mut config = ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(other)?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(verifier))
        .with_client_auth_cert(chain, key)
        .map_err(other)?;
    // Hosts are reached by address and their certificate names nobody.
    config.enable_sni = false;
    // Every connection shows the certificate again. Apollo asks for one
    // without giving OpenSSL a session id context (Sunshine, whose code that
    // is, most likely too), so OpenSSL answers a resumed session with an
    // `internal_error` alert: every request to a PC after the first fails.
    config.resumption = Resumption::disabled();
    Ok(config)
}

/// Accepts exactly one certificate: the one the host showed during pairing.
#[derive(Debug)]
struct PinnedHost {
    /// The host's certificate, DER encoded.
    certificate: Vec<u8>,
    algorithms: WebPkiSupportedAlgorithms,
}

impl ServerCertVerifier for PinnedHost {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        // Name and dates are deliberately ignored, as every Moonlight client
        // does: identity is the certificate itself. The handshake signature,
        // checked below, proves the peer holds its private key.
        if end_entity.as_ref() == self.certificate.as_slice() {
            Ok(ServerCertVerified::assertion())
        } else {
            Err(rustls::Error::InvalidCertificate(CertificateError::ApplicationVerificationFailure))
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

fn io_error(error: io::Error) -> Error {
    match error.kind() {
        io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock => Error::ConnectionTimeout,
        // rustls reports a refused certificate as `InvalidData`. Keep those
        // details, they tell a wrong host from a network problem.
        io::ErrorKind::InvalidData => Error::Io(error),
        _ => Error::ConnectionFailed,
    }
}

fn invalid(message: &'static str) -> Error {
    Error::Io(io::Error::new(io::ErrorKind::InvalidData, message))
}

fn other(error: impl std::error::Error + Send + Sync + 'static) -> Error {
    Error::Other(Box::new(error))
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::*;

    /// A stream that returns a canned response and records what was written.
    struct Canned {
        response: Cursor<Vec<u8>>,
        written: Vec<u8>,
    }

    impl Canned {
        fn new(response: &[u8]) -> Self {
            Self { response: Cursor::new(response.to_vec()), written: Vec::new() }
        }
    }

    impl Read for Canned {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            self.response.read(buffer)
        }
    }

    impl Write for Canned {
        fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
            self.written.extend_from_slice(buffer);
            Ok(buffer.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    fn body_of(response: &[u8]) -> Result<Vec<u8>, Error> {
        exchange(&mut Canned::new(response), b"GET / HTTP/1.1\r\n\r\n")
    }

    #[test]
    fn reads_a_body_with_a_length() {
        let body = body_of(b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nhelloEXTRA");
        assert_eq!(body.ok(), Some(b"hello".to_vec()));
    }

    #[test]
    fn reads_a_body_until_the_connection_closes() {
        let body = body_of(b"HTTP/1.1 200 OK\r\nServer: test\r\n\r\nhello world");
        assert_eq!(body.ok(), Some(b"hello world".to_vec()));
    }

    #[test]
    fn reads_a_chunked_body() {
        let body = body_of(
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n6;x=y\r\n world\r\n0\r\n\r\n",
        );
        assert_eq!(body.ok(), Some(b"hello world".to_vec()));
    }

    #[test]
    fn a_truncated_body_is_an_error() {
        assert!(body_of(b"HTTP/1.1 200 OK\r\nContent-Length: 50\r\n\r\nshort").is_err());
        assert!(body_of(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhel").is_err());
    }

    #[test]
    fn an_error_status_is_reported() {
        match body_of(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n") {
            Err(Error::StatusCode { code, reason }) => {
                assert_eq!(code, 404);
                assert_eq!(reason, "Not Found");
            }
            other => panic!("expected a status error, got {other:?}"),
        }
    }

    #[test]
    fn garbage_is_an_error() {
        assert!(body_of(b"\x16\x03\x01 not http at all\r\n\r\n").is_err());
        assert!(body_of(b"").is_err());
    }

    #[test]
    fn the_request_is_sent_as_is() {
        let mut stream = Canned::new(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n");
        let request = b"GET /serverinfo?uniqueid=x HTTP/1.1\r\nHost: pc:47989\r\n\r\n";
        assert!(exchange(&mut stream, request).is_ok());
        assert_eq!(stream.written, request);
    }
}
