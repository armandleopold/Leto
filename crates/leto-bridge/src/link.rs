//! One browser on its WebSocket: let in through the gate, then served by an
//! engine service of its own.

use std::{sync::Arc, time::Duration};

use futures_util::{SinkExt as _, StreamExt as _};
use leto_client::{Client, Service, Shell, engine::Event};
use leto_core::bridge::{Down, Up};
use tokio::{
    io::{AsyncRead, AsyncWrite},
    time::{Instant, MissedTickBehavior, interval, sleep},
};
use tokio_tungstenite::{
    WebSocketStream,
    tungstenite::{Bytes, Message, protocol::WebSocketConfig},
};

use crate::{
    gate::{Gate, PAUSE},
    relay::{Outbox, Parcel, Pictures, Sound},
};

/// How often the bridge asks a browser whether it is still there.
const ASKED_EVERY: Duration = Duration::from_secs(5);

/// A browser that said nothing for this long is gone, whatever the network
/// still pretends: its stream is left.
const GONE_AFTER: Duration = Duration::from_secs(20);

/// The longest message a browser has reason to send. What a person does in
/// one frame is far below it.
const LONGEST_MESSAGE: usize = 64 * 1024;

/// What every browser's link shares.
pub struct Shared {
    pub client: Arc<Client>,
    pub gate: Gate,
    /// What the computer running the bridge calls itself.
    pub name: String,
}

/// The limits of a browser's WebSocket.
pub fn limits() -> WebSocketConfig {
    WebSocketConfig::default()
        .max_message_size(Some(LONGEST_MESSAGE))
        .max_frame_size(Some(LONGEST_MESSAGE))
}

/// What the engine works for on the bridge: one browser.
struct Browser {
    outbox: Arc<Outbox>,
}

impl Shell for Browser {
    type Video = Pictures;
    type Audio = Sound;

    fn tell(&self, event: Event) {
        self.outbox.say(&Down::News(event));
    }

    fn sinks(&self) -> Result<(Pictures, Sound), String> {
        Ok(self.outbox.sinks())
    }
}

/// Serves one browser until it leaves.
pub async fn serve<S>(mut socket: WebSocketStream<S>, shared: Arc<Shared>)
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let (outbox, mut parcels) = Outbox::new();
    // The engine at work for this browser, once it is let in.
    let mut service: Option<Service<Browser>> = None;
    let mut heard = Instant::now();
    let mut asking = interval(ASKED_EVERY);
    asking.set_missed_tick_behavior(MissedTickBehavior::Delay);

    loop {
        tokio::select! {
            message = socket.next() => {
                let Some(Ok(message)) = message else { break };
                heard = Instant::now();
                match message {
                    Message::Text(text) => {
                        let Ok(said) = serde_json::from_str::<Up>(text.as_str()) else {
                            // Not a Leto page, or one of another version.
                            log::debug!("a browser said something the bridge does not understand");
                            break;
                        };
                        hear(said, &shared, &outbox, &mut service);
                    }
                    Message::Close(_) => break,
                    // Answers to "still there?" count as heard, and the
                    // bridge takes nothing else.
                    _ => {}
                }
            }
            parcel = parcels.recv() => {
                // The outbox lives as long as this function: never `None`.
                let Some(parcel) = parcel else { break };
                outbox.sent(&parcel);
                let message = match parcel {
                    Parcel::Text(text) => Message::text(text),
                    Parcel::Picture(bytes) | Parcel::Sound(bytes) => {
                        Message::Binary(Bytes::from(bytes))
                    }
                };
                if socket.send(message).await.is_err() {
                    break;
                }
            }
            _ = asking.tick() => {
                if heard.elapsed() > GONE_AFTER || socket.send(Message::Ping(Bytes::new())).await.is_err() {
                    break;
                }
            }
        }
    }
    // Dropping the service leaves the stream this browser was watching.
    drop(service);
    let _ = socket.close(None).await;
}

/// Acts on what a browser said.
fn hear(
    said: Up,
    shared: &Arc<Shared>,
    outbox: &Arc<Outbox>,
    service: &mut Option<Service<Browser>>,
) {
    let mut welcome = |key: Option<String>| {
        if service.is_none() {
            let browser = Browser { outbox: Arc::clone(outbox) };
            *service = Some(Service::new(Arc::clone(&shared.client), browser));
        }
        outbox.say(&Down::Welcome { key, name: shared.name.clone() });
    };
    match said {
        Up::Hello { key } => {
            if key.is_some_and(|key| shared.gate.knows(&key)) {
                welcome(None);
            } else {
                outbox.say(&Down::CodeNeeded { refused: false });
            }
        }
        Up::Code(code) => match shared.gate.try_code(&code) {
            Some(key) => welcome(Some(key)),
            None => {
                // The refusal takes as long as the gate stays shut: whoever
                // types the next code on reading it is not turned away
                // unheard.
                let outbox = Arc::clone(outbox);
                tokio::spawn(async move {
                    sleep(PAUSE).await;
                    outbox.say(&Down::CodeNeeded { refused: true });
                });
            }
        },
        // Everything else is for browsers the gate let in. One that asks
        // without having been welcomed is told what it has to do first.
        Up::Ask(request) => match service {
            Some(service) => service.ask(request),
            None => outbox.say(&Down::CodeNeeded { refused: false }),
        },
        Up::Input(inputs) => {
            if let Some(service) = service {
                service.input(inputs);
            }
        }
        Up::KeyframeNeeded => outbox.want_keyframe(),
    }
}
