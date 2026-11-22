//! What a client and a host say to each other before a camera flows.
//!
//! A host that takes a camera answers HTTPS requests on the port of Leto's
//! API, thirteen above its base port. Both sides present the certificates
//! they exchanged when they were paired, so only a paired device can ask and
//! only the PC it knows can answer. Three requests, with JSON bodies:
//!
//! - `GET /leto/v1/info` is answered with an [`Info`]: what the host takes.
//! - `POST /leto/v1/return-channel` carries an [`Ask`], and is answered with
//!   an [`Answer`]: where to send, and the key to seal the packets with.
//!   One device sends at a time: asking again replaces what was agreed.
//! - `DELETE /leto/v1/return-channel?keyId=7` says that the device stopped
//!   sending what the key of that number sealed. A host leaves alone what
//!   was agreed since: a goodbye may arrive after the ask that follows it.
//!   Without a number, it ends whatever the device sends.
//!
//! A host that will not answers with a status and nothing else: 403 when
//! its owner does not let this device send, 409 while another device is
//! sending, 400 for an ask it cannot serve.
//!
//! A reader ignores the fields it does not know, so that either side can
//! say more later.

use serde::{Deserialize, Serialize};

use crate::camera::Key;

/// The version of this API.
pub const VERSION: u32 = 1;

pub const INFO_PATH: &str = "/leto/v1/info";
pub const RETURN_CHANNEL_PATH: &str = "/leto/v1/return-channel";

/// The name, in the query of a goodbye, of the number of the key that
/// sealed what stops.
const KEY_ID: &str = "keyId";

/// The path and query that say that what the key of this number sealed
/// stopped.
pub fn goodbye(key_id: u32) -> String {
    format!("{RETURN_CHANNEL_PATH}?{KEY_ID}={key_id}")
}

/// The number of the key in the `query` of a goodbye, when it names one.
pub fn goodbye_key(query: &str) -> Option<u32> {
    query
        .split('&')
        .filter_map(|pair| pair.split_once('='))
        .find(|(name, _)| *name == KEY_ID)
        .and_then(|(_, number)| number.parse().ok())
}

/// The one way pictures are encoded so far.
pub const H264: &str = "h264";

/// What a host says about itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Info {
    /// The [`VERSION`] the host speaks.
    pub api: u32,
    pub return_channel: Offer,
}

/// What a host takes from the device that watches it.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Offer {
    /// The microphone can be asked for here too.
    #[serde(default)]
    pub microphone: bool,
    /// The camera, when the host takes one.
    #[serde(default)]
    pub camera: Option<CameraOffer>,
}

/// The camera a host takes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CameraOffer {
    /// How pictures may be encoded, by the names of this module.
    pub codecs: Vec<String>,
    pub max_width: u32,
    pub max_height: u32,
    /// Pictures a second.
    pub max_fps: u32,
}

/// What a device would like to send.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Ask {
    #[serde(default)]
    pub microphone: bool,
    #[serde(default)]
    pub camera: Option<CameraAsk>,
}

/// The camera a device would like to send.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CameraAsk {
    pub codec: String,
    pub width: u32,
    pub height: u32,
    /// Pictures a second.
    pub fps: u32,
    pub bitrate_kbps: u32,
}

/// What a host agrees to take.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Answer {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub camera: Option<Grant>,
    /// The microphone, sent as `crate::voice` describes it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub microphone: Option<Grant>,
}

/// Where a camera or a microphone goes, and what seals it.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Grant {
    /// The UDP port the packets go to, at the address of the API.
    pub port: u16,
    /// The sixteen bytes of the key, as thirty-two hexadecimal digits.
    pub key: String,
    pub key_id: u32,
}

impl Grant {
    pub fn new(port: u16, key: &Key) -> Self {
        let digits = key.bytes.iter().map(|byte| format!("{byte:02x}")).collect();
        Self { port, key: digits, key_id: key.id }
    }

    /// The key to seal the packets with, when what the host sent is one.
    pub fn key(&self) -> Option<Key> {
        // Digits only: a number may otherwise be written with a sign.
        if self.key.len() != 32 || !self.key.bytes().all(|digit| digit.is_ascii_hexdigit()) {
            return None;
        }
        let mut bytes = [0u8; 16];
        for (byte, digits) in bytes.iter_mut().zip(self.key.as_bytes().chunks(2)) {
            let digits = std::str::from_utf8(digits).ok()?;
            *byte = u8::from_str_radix(digits, 16).ok()?;
        }
        Some(Key { bytes, id: self.key_id })
    }
}

impl std::fmt::Debug for Grant {
    /// A key is not for logs.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Grant {{ port: {}, key_id: {:#010x} }}", self.port, self.key_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn written<T: Serialize>(value: &T) -> String {
        serde_json::to_string(value).expect("these types have no map keys to refuse")
    }

    #[test]
    fn a_host_says_what_it_takes_as_the_design_writes_it() {
        let info = Info {
            api: VERSION,
            return_channel: Offer {
                microphone: true,
                camera: Some(CameraOffer {
                    codecs: vec![H264.to_owned()],
                    max_width: 1920,
                    max_height: 1080,
                    max_fps: 30,
                }),
            },
        };
        let text = r#"{"api":1,"returnChannel":{"microphone":true,"camera":{"codecs":["h264"],"maxWidth":1920,"maxHeight":1080,"maxFps":30}}}"#;
        assert_eq!(written(&info), text);
        assert_eq!(serde_json::from_str::<Info>(text).ok(), Some(info));
    }

    #[test]
    fn what_is_left_out_is_not_offered_and_what_is_unknown_is_ignored() {
        let text = r#"{"api":2,"returnChannel":{"screens":3},"name":"Salon"}"#;
        let info: Info = serde_json::from_str(text).expect("a later version is still read");
        assert_eq!(info, Info { api: 2, return_channel: Offer::default() });

        let answer: Answer = serde_json::from_str("{}").expect("an answer may be empty");
        assert_eq!(answer.camera, None);
        assert_eq!(answer.microphone, None);
    }

    #[test]
    fn an_ask_and_its_answer_read_back() {
        let ask = Ask {
            microphone: false,
            camera: Some(CameraAsk {
                codec: H264.to_owned(),
                width: 1280,
                height: 720,
                fps: 30,
                bitrate_kbps: 2500,
            }),
        };
        let text = written(&ask);
        assert!(text.contains(r#""bitrateKbps":2500"#), "{text}");
        assert_eq!(serde_json::from_str::<Ask>(&text).ok(), Some(ask));

        let key = Key {
            bytes: *b"\x00\x01\x02\x03\x04\x05\x06\x07\xF8\xF9\xFA\xFB\xFC\xFD\xFE\xFF",
            id: 7,
        };
        let answer = Answer { camera: Some(Grant::new(48002, &key)), microphone: None };
        let text = written(&answer);
        assert_eq!(
            text,
            r#"{"camera":{"port":48002,"key":"0001020304050607f8f9fafbfcfdfeff","keyId":7}}"#
        );
        let read: Answer = serde_json::from_str(&text).expect("what was written reads back");
        assert_eq!(read.camera.and_then(|grant| grant.key()), Some(key));
    }

    #[test]
    fn a_key_that_is_not_one_is_not_used() {
        let grant = |key: &str| Grant { port: 1, key: key.to_owned(), key_id: 0 };
        assert_eq!(grant("").key(), None);
        assert_eq!(grant("0001020304050607f8f9fafbfcfdfe").key(), None);
        assert_eq!(grant("0001020304050607f8f9fafbfcfdfeffff").key(), None);
        assert_eq!(grant("zz01020304050607f8f9fafbfcfdfeff").key(), None);
        assert_eq!(grant("+001020304050607f8f9fafbfcfdfeff").key(), None);
        assert_eq!(grant("ééééééééééééééééé").key(), None);
        assert!(grant("0001020304050607F8F9FAFBFCFDFEFF").key().is_some());
    }

    #[test]
    fn a_goodbye_names_the_key_of_what_stops() {
        assert_eq!(goodbye(7), "/leto/v1/return-channel?keyId=7");
        assert_eq!(goodbye_key("keyId=7"), Some(7));
        assert_eq!(goodbye_key("later=1&keyId=4294967295"), Some(u32::MAX));
        // No number, or none that is one: whatever the device sends.
        assert_eq!(goodbye_key(""), None);
        assert_eq!(goodbye_key("keyId=-1"), None);
        assert_eq!(goodbye_key("keyid=7"), None);
    }

    #[test]
    fn a_grant_does_not_show_its_key_in_logs() {
        let key = Key { bytes: [0xAB; 16], id: 0xC0FFEE };
        let shown = format!("{:?}", Grant::new(48002, &key));
        assert_eq!(shown, "Grant { port: 48002, key_id: 0x00c0ffee }");
    }
}
