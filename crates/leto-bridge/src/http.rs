//! Just enough HTTP for a browser to fetch the web client and open its
//! WebSocket: one request per connection, answered and closed.

use std::io;

use tokio::io::{AsyncRead, AsyncReadExt as _, AsyncWrite, AsyncWriteExt as _};
use tokio_tungstenite::tungstenite::handshake::derive_accept_key;

/// A request line and headers longer than this are not from a browser
/// fetching Leto.
const HEAD_LIMIT: usize = 16 * 1024;

/// Most headers a request may carry.
const HEADER_LIMIT: usize = 64;

/// The first lines of a request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Head {
    pub method: String,
    /// The path as the browser sent it, query included.
    pub path: String,
    headers: Vec<(String, String)>,
}

impl Head {
    /// The value of a header, whatever the case of its name.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(header, _)| header.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }

    /// Whether a header lists `token` among its comma-separated values.
    fn lists(&self, name: &str, token: &str) -> bool {
        self.header(name).is_some_and(|value| {
            value.split(',').any(|part| part.trim().eq_ignore_ascii_case(token))
        })
    }
}

/// Reads the head of a request, when `bytes` holds all of it. Returns it
/// with the number of bytes it took, `None` when more bytes are needed.
pub fn parse_head(bytes: &[u8]) -> io::Result<Option<(Head, usize)>> {
    let mut headers = [httparse::EMPTY_HEADER; HEADER_LIMIT];
    let mut request = httparse::Request::new(&mut headers);
    let invalid = |reason: String| io::Error::new(io::ErrorKind::InvalidData, reason);
    match request.parse(bytes).map_err(|error| invalid(error.to_string()))? {
        httparse::Status::Partial => Ok(None),
        httparse::Status::Complete(length) => {
            let (Some(method), Some(path)) = (request.method, request.path) else {
                return Err(invalid("a request without a method or a path".to_owned()));
            };
            let headers = request
                .headers
                .iter()
                .map(|header| {
                    let value = String::from_utf8_lossy(header.value).trim().to_owned();
                    (header.name.to_owned(), value)
                })
                .collect();
            Ok(Some((Head { method: method.to_owned(), path: path.to_owned(), headers }, length)))
        }
    }
}

/// Reads the head of the request `stream` starts with. Returns it with
/// whatever was read beyond it.
pub async fn read_head<S: AsyncRead + Unpin>(stream: &mut S) -> io::Result<(Head, Vec<u8>)> {
    let mut bytes = Vec::with_capacity(1024);
    loop {
        if let Some((head, length)) = parse_head(&bytes)? {
            return Ok((head, bytes.split_off(length)));
        }
        if bytes.len() >= HEAD_LIMIT {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "the request is too long"));
        }
        let mut chunk = [0_u8; 2048];
        let read = stream.read(&mut chunk).await?;
        if read == 0 {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        bytes.extend_from_slice(&chunk[..read]);
    }
}

/// An answer that is sent whole, after which the connection closes.
pub struct Answer<'a> {
    pub status: u16,
    pub reason: &'a str,
    pub content_type: &'a str,
    /// More header lines, each ending in `\r\n`.
    pub more: &'a str,
    pub body: &'a [u8],
}

impl<'a> Answer<'a> {
    /// A short message in plain text.
    pub fn text(status: u16, reason: &'a str, body: &'a str) -> Self {
        Self {
            status,
            reason,
            content_type: "text/plain; charset=utf-8",
            more: "",
            body: body.as_bytes(),
        }
    }

    /// Sends the answer. `with_body` is false for `HEAD` requests.
    pub async fn send<S: AsyncWrite + Unpin>(
        &self,
        stream: &mut S,
        with_body: bool,
    ) -> io::Result<()> {
        let head = format!(
            "HTTP/1.1 {} {}\r\nContent-Type: {}\r\nContent-Length: {}\r\n\
             X-Content-Type-Options: nosniff\r\nCache-Control: no-cache\r\n\
             Connection: close\r\n{}\r\n",
            self.status,
            self.reason,
            self.content_type,
            self.body.len(),
            self.more
        );
        stream.write_all(head.as_bytes()).await?;
        if with_body {
            stream.write_all(self.body).await?;
        }
        stream.flush().await?;
        stream.shutdown().await
    }
}

/// Why a request to open the WebSocket is turned down.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoUpgrade {
    /// Not a WebSocket handshake a browser of today makes.
    NotAWebSocket,
    /// The page that asks did not come from this bridge.
    ForeignPage,
}

/// Checks a request to open the WebSocket and returns the lines that accept
/// it.
///
/// Only pages this bridge served may open it: a browser says which page
/// asks in `Origin`, and a page from anywhere else on the web is turned
/// down before it can say a word.
pub fn accept_upgrade(head: &Head) -> Result<String, NoUpgrade> {
    let websocket = head.method == "GET"
        && head.lists("Connection", "upgrade")
        && head.lists("Upgrade", "websocket")
        && head.header("Sec-WebSocket-Version") == Some("13");
    let Some(key) = head.header("Sec-WebSocket-Key").filter(|_| websocket) else {
        return Err(NoUpgrade::NotAWebSocket);
    };
    let ours = match (head.header("Origin"), head.header("Host")) {
        (Some(origin), Some(host)) => {
            origin.strip_prefix("https://").is_some_and(|origin| origin.eq_ignore_ascii_case(host))
        }
        _ => false,
    };
    if !ours {
        return Err(NoUpgrade::ForeignPage);
    }
    Ok(format!(
        "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\
         Sec-WebSocket-Accept: {}\r\n\r\n",
        derive_accept_key(key.as_bytes())
    ))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    const UPGRADE: &str = "GET /link HTTP/1.1\r\nHost: 192.168.1.20:5386\r\n\
        Connection: keep-alive, Upgrade\r\nUpgrade: websocket\r\n\
        Origin: https://192.168.1.20:5386\r\nSec-WebSocket-Version: 13\r\n\
        Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\r\n";

    fn head(text: &str) -> Head {
        parse_head(text.as_bytes()).unwrap().unwrap().0
    }

    #[test]
    fn a_head_is_read_once_it_is_all_there() {
        let text = "GET /leto.js?v=1 HTTP/1.1\r\nhost: salon:5386\r\nAccept: */*\r\n\r\nrest";
        assert_eq!(parse_head(&text.as_bytes()[..30]).unwrap(), None);
        let (head, length) = parse_head(text.as_bytes()).unwrap().unwrap();
        assert_eq!(head.method, "GET");
        assert_eq!(head.path, "/leto.js?v=1");
        assert_eq!(head.header("Host"), Some("salon:5386"));
        assert_eq!(head.header("Cookie"), None);
        assert_eq!(&text.as_bytes()[length..], b"rest");
    }

    #[test]
    fn what_is_not_http_is_an_error() {
        assert!(parse_head(b"\x16\x03\x01\x02\x00\x01\x00").is_err());
        assert!(parse_head(b"GET\r\n\r\n").is_err());
    }

    #[tokio::test]
    async fn a_request_that_never_ends_is_cut_off() {
        let endless = vec![b'a'; HEAD_LIMIT * 2];
        let mut stream = &endless[..];
        assert!(read_head(&mut stream).await.is_err());

        let mut cut_short = &b"GET / HTTP/1.1\r\nHost: sal"[..];
        assert!(read_head(&mut cut_short).await.is_err());
    }

    #[tokio::test]
    async fn what_follows_the_head_is_kept() {
        let mut stream = &b"GET / HTTP/1.1\r\nHost: salon\r\n\r\n\x81\x85"[..];
        let (head, rest) = read_head(&mut stream).await.unwrap();
        assert_eq!(head.path, "/");
        assert_eq!(rest, [0x81, 0x85]);
    }

    #[test]
    fn a_page_of_the_bridge_may_open_the_websocket() {
        let accepted = accept_upgrade(&head(UPGRADE)).unwrap();
        // The answer every WebSocket book gives for that key.
        assert!(accepted.contains("Sec-WebSocket-Accept: s3pPLMBiTxaQ9kYGzzhZRbK+xOo=\r\n"));
        assert!(accepted.starts_with("HTTP/1.1 101 "));
    }

    #[test]
    fn a_page_from_elsewhere_may_not() {
        let foreign =
            UPGRADE.replace("Origin: https://192.168.1.20:5386", "Origin: https://evil.example");
        assert_eq!(accept_upgrade(&head(&foreign)), Err(NoUpgrade::ForeignPage));
        // Not even the same address over plain HTTP, or another port of it.
        let plain = UPGRADE.replace("Origin: https://", "Origin: http://");
        assert_eq!(accept_upgrade(&head(&plain)), Err(NoUpgrade::ForeignPage));
        let other_port = UPGRADE
            .replace("Origin: https://192.168.1.20:5386", "Origin: https://192.168.1.20:8443");
        assert_eq!(accept_upgrade(&head(&other_port)), Err(NoUpgrade::ForeignPage));
        // Nor something that is not a browser page at all.
        let no_origin = UPGRADE.replace("Origin: https://192.168.1.20:5386\r\n", "");
        assert_eq!(accept_upgrade(&head(&no_origin)), Err(NoUpgrade::ForeignPage));
    }

    #[test]
    fn an_ordinary_request_is_not_an_upgrade() {
        let plain = head("GET /link HTTP/1.1\r\nHost: salon:5386\r\n\r\n");
        assert_eq!(accept_upgrade(&plain), Err(NoUpgrade::NotAWebSocket));
        let old = UPGRADE.replace("Sec-WebSocket-Version: 13", "Sec-WebSocket-Version: 8");
        assert_eq!(accept_upgrade(&head(&old)), Err(NoUpgrade::NotAWebSocket));
    }

    #[tokio::test]
    async fn an_answer_says_how_long_it_is_and_closes() {
        let mut sent = Vec::new();
        Answer::text(404, "Not Found", "Nothing here.").send(&mut sent, true).await.unwrap();
        let text = String::from_utf8(sent).unwrap();
        assert!(text.starts_with("HTTP/1.1 404 Not Found\r\n"));
        assert!(text.contains("Content-Length: 13\r\n"));
        assert!(text.contains("Connection: close\r\n"));
        assert!(text.ends_with("\r\n\r\nNothing here."));

        let mut head_only = Vec::new();
        Answer::text(200, "OK", "Hello.").send(&mut head_only, false).await.unwrap();
        assert!(String::from_utf8(head_only).unwrap().ends_with("\r\n\r\n"));
    }
}
