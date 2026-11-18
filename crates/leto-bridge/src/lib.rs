//! The bridge between browsers and PCs.
//!
//! A browser page cannot open the sockets the Moonlight protocol needs. The
//! bridge can: it runs next to the PC, or anywhere on its network, serves
//! the web client over HTTPS and runs the client engine for each browser
//! that opens it. To a PC the bridge is one more paired device. To a browser
//! it is the place the page came from.
//!
//! - [`gate`] decides which browsers may use it.
//! - `identity` is the certificate it shows them.
//! - `pages` serves the web client's files, `http` speaks just enough HTTP.
//! - `link` is one browser on its WebSocket, `relay` the stream on its way
//!   there.
//!
//! What a browser and the bridge say to each other is described in
//! [`leto_core::bridge`].

pub mod gate;
mod http;
mod identity;
mod link;
mod pages;
mod relay;

use std::{
    io,
    net::{IpAddr, SocketAddr},
    path::PathBuf,
    sync::Arc,
    time::{Duration, SystemTime},
};

use leto_client::Client;
use leto_core::bridge::PATH;
use tokio::{
    io::{AsyncRead, AsyncWrite, AsyncWriteExt as _},
    net::{TcpListener, TcpStream},
    runtime::Runtime,
    time::timeout,
};
use tokio_rustls::TlsAcceptor;
use tokio_tungstenite::{WebSocketStream, tungstenite::protocol::Role};

pub use crate::identity::{computer_name, local_address};
use crate::{
    gate::{Announce, Gate},
    http::{Answer, Head, NoUpgrade, accept_upgrade, read_head},
    identity::Identity,
    link::Shared,
    pages::Pages,
};

/// The port the bridge listens on unless told otherwise: L, E, T and O on
/// the keys of a telephone.
pub const DEFAULT_PORT: u16 = 5386;

/// How long a browser may take to say what it wants, once connected.
const PATIENCE: Duration = Duration::from_secs(10);

/// The first byte of every TLS connection: a handshake record.
const TLS_HANDSHAKE: u8 = 0x16;

pub struct Options {
    /// Where the bridge keeps its certificate and the browsers it knows.
    pub folder: PathBuf,
    /// The folder `cargo xtask web` built the web client into.
    pub pages: PathBuf,
    /// The address to listen on: `0.0.0.0` for every device of the network,
    /// `127.0.0.1` for this computer only.
    pub interface: IpAddr,
    /// The port to listen on, or 0 for any free one.
    pub port: u16,
}

/// A running bridge. Dropping it stops it, and leaves the streams browsers
/// were watching through it.
pub struct Bridge {
    runtime: Option<Runtime>,
    address: SocketAddr,
    shared: Arc<Shared>,
    identity: Identity,
}

impl Bridge {
    /// Starts serving browsers with `client` as the engine. `announce` is
    /// called with every code the gate draws after the first, which
    /// [`Bridge::code`] returns.
    pub fn start(client: Arc<Client>, options: Options, announce: Announce) -> io::Result<Self> {
        let identity = Identity::load_or_create(&options.folder, SystemTime::now())?;
        let acceptor = TlsAcceptor::from(identity.tls()?);
        let gate = Gate::open(&options.folder, announce)?;
        let shared = Arc::new(Shared { client, gate, name: computer_name() });
        let pages = Arc::new(Pages::new(&options.pages));

        let listener = std::net::TcpListener::bind((options.interface, options.port))?;
        listener.set_nonblocking(true)?;
        let address = listener.local_addr()?;

        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("leto: bridge")
            .enable_all()
            .build()?;
        let listener = {
            let _inside = runtime.enter();
            TcpListener::from_std(listener)?
        };
        runtime.spawn(listen(listener, acceptor, Arc::clone(&shared), pages));

        Ok(Self { runtime: Some(runtime), address, shared, identity })
    }

    /// The port browsers reach the bridge on.
    pub fn port(&self) -> u16 {
        self.address.port()
    }

    /// The code a new browser has to be given, digits only.
    pub fn code(&self) -> String {
        self.shared.gate.code()
    }

    /// The gate: who is known, and how to forget them.
    pub fn gate(&self) -> &Gate {
        &self.shared.gate
    }

    /// The certificate browsers are shown, as PEM.
    pub fn certificate(&self) -> &str {
        &self.identity.certificate
    }

    /// The certificate's SHA-256 fingerprint, the way browsers show it.
    pub fn fingerprint(&self) -> io::Result<String> {
        self.identity.fingerprint()
    }
}

impl Drop for Bridge {
    fn drop(&mut self) {
        if let Some(runtime) = self.runtime.take() {
            // Links end, their services are dropped, their streams left.
            runtime.shutdown_background();
        }
    }
}

async fn listen(
    listener: TcpListener,
    acceptor: TlsAcceptor,
    shared: Arc<Shared>,
    pages: Arc<Pages>,
) {
    loop {
        match listener.accept().await {
            Ok((stream, peer)) => {
                let (acceptor, shared, pages) =
                    (acceptor.clone(), Arc::clone(&shared), Arc::clone(&pages));
                tokio::spawn(async move {
                    if let Err(error) = connection(stream, acceptor, shared, pages).await {
                        log::debug!("the connection from {peer} ended: {error}");
                    }
                });
            }
            Err(error) => {
                // Out of file handles, for one: give it a moment.
                log::warn!("the bridge could not take a connection: {error}");
                tokio::time::sleep(Duration::from_millis(200)).await;
            }
        }
    }
}

/// One connection: one request, answered, or one browser's link.
async fn connection(
    stream: TcpStream,
    acceptor: TlsAcceptor,
    shared: Arc<Shared>,
    pages: Arc<Pages>,
) -> io::Result<()> {
    // Pictures and what the person does are small and urgent: never held
    // back to fill a packet.
    stream.set_nodelay(true)?;

    let mut first = [0_u8; 1];
    let peeked = within(stream.peek(&mut first)).await?;
    if peeked == 1 && first[0] != TLS_HANDSHAKE {
        return redirect_to_https(stream).await;
    }

    let mut stream = within(acceptor.accept(stream)).await?;
    let (head, rest) = within(read_head(&mut stream)).await?;
    let path = head.path.split('?').next().unwrap_or_default();

    if path == PATH {
        return match accept_upgrade(&head) {
            Ok(accepted) => {
                stream.write_all(accepted.as_bytes()).await?;
                stream.flush().await?;
                let limits = Some(link::limits());
                let socket =
                    WebSocketStream::from_partially_read(stream, rest, Role::Server, limits).await;
                link::serve(socket, shared).await;
                Ok(())
            }
            Err(NoUpgrade::ForeignPage) => {
                let answer = Answer::text(403, "Forbidden", "This page is not from this bridge.");
                answer.send(&mut stream, true).await
            }
            Err(NoUpgrade::NotAWebSocket) => {
                let answer = Answer::text(400, "Bad Request", "This address is for the Leto app.");
                answer.send(&mut stream, true).await
            }
        };
    }
    serve_page(&mut stream, &head, &pages).await
}

async fn serve_page<S: AsyncRead + AsyncWrite + Unpin>(
    stream: &mut S,
    head: &Head,
    pages: &Pages,
) -> io::Result<()> {
    let with_body = match head.method.as_str() {
        "GET" => true,
        "HEAD" => false,
        _ => {
            let answer = Answer {
                more: "Allow: GET, HEAD\r\n",
                ..Answer::text(405, "Method Not Allowed", "The bridge only hands out pages.")
            };
            return answer.send(stream, true).await;
        }
    };
    match pages.get(&head.path) {
        Ok(Some(page)) => {
            let answer = Answer {
                status: 200,
                reason: "OK",
                content_type: page.content_type,
                more: "",
                body: &page.body,
            };
            answer.send(stream, with_body).await
        }
        Ok(None) if !pages.complete() => {
            let answer = Answer::text(
                503,
                "Service Unavailable",
                "The bridge is running, but the web client is not built. \
                 Run `cargo xtask web`, then load this page again.",
            );
            answer.send(stream, with_body).await
        }
        Ok(None) => Answer::text(404, "Not Found", "Nothing here.").send(stream, with_body).await,
        Err(error) => {
            log::warn!("could not read {}: {error}", head.path);
            let answer =
                Answer::text(500, "Internal Server Error", "The bridge could not read that file.");
            answer.send(stream, with_body).await
        }
    }
}

/// Someone typed the address without `https://`. Send them to the right
/// door instead of leaving them with a browser error.
async fn redirect_to_https(mut stream: TcpStream) -> io::Result<()> {
    let (head, _) = within(read_head(&mut stream)).await?;
    // Only to a name made of what names are made of: nothing a header could
    // smuggle into the answer.
    let host = head.header("Host").filter(|host| {
        !host.is_empty()
            && host.bytes().all(|byte| {
                byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b':' | b'[' | b']')
            })
    });
    let Some(host) = host else {
        return Answer::text(400, "Bad Request", "The bridge speaks HTTPS.")
            .send(&mut stream, true)
            .await;
    };
    let location = format!("Location: https://{host}/\r\n");
    let answer = Answer {
        more: &location,
        ..Answer::text(308, "Permanent Redirect", "The bridge speaks HTTPS.")
    };
    answer.send(&mut stream, true).await
}

/// Gives up on a browser that takes too long over something that should be
/// quick.
async fn within<T, E: Into<io::Error>>(work: impl Future<Output = Result<T, E>>) -> io::Result<T> {
    match timeout(PATIENCE, work).await {
        Ok(result) => result.map_err(Into::into),
        Err(_) => Err(io::ErrorKind::TimedOut.into()),
    }
}
