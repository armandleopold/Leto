//! Asking a PC what it offers, before the stream is connected.
//!
//! The Moonlight protocol has no word for "this PC takes a microphone". The
//! hosts that do (the Apollo microphone fork, Foundation Sunshine) say so in
//! the session description they hand out during the RTSP handshake: it has a
//! media line for a second sound stream, the one towards the host.
//!
//! moonlight-common has to be told whether to set that stream up before it
//! starts the handshake, and it decides from the kind of host alone. So Leto
//! asks for the description first, on a connection of its own. That costs
//! one exchange and changes nothing on the host: Sunshine and its
//! descendants answer every RTSP request on a new connection anyway, and a
//! description is only read.

use std::{
    io::{self, Read, Write},
    net::{IpAddr, SocketAddr, TcpStream},
    time::Duration,
};

use moonlight_common::{
    crypto::rustcrypto::RustCryptoBackend,
    stream::{
        AesKey, MoonlightStreamConfig,
        proto::{crypto::CryptoBackend, rtsp::raw::RtspAddr},
    },
};

/// How long the host has to answer. Without an answer the stream starts as
/// it always did, without a microphone.
const PATIENCE: Duration = Duration::from_secs(2);

/// The number of the request. An encrypted request is encrypted with its
/// number, and the handshake that follows counts from one with the same key:
/// this one must be a number it never reaches.
const NUMBER: u32 = 0x4C45_544F;

/// What marks an encrypted RTSP message, in front of its length.
const ENCRYPTED: u32 = 0x8000_0000;
/// Type and length, number, authentication tag.
const HEADER: usize = 4 + 4 + 16;
/// The largest answer read. A description is a few hundred bytes.
const LIMIT: u64 = 64 * 1024;

/// The bit of `x-ss-general.encryptionSupported` for the microphone.
const MICROPHONE: u32 = 0x08;

/// Whether the host of the stream about to be connected takes a microphone.
pub(crate) fn takes_microphone(config: &MoonlightStreamConfig) -> bool {
    match describe(config) {
        Ok(description) => offers_microphone(&description),
        Err(error) => {
            log::debug!("the PC did not describe its stream ahead of time: {error}");
            false
        }
    }
}

/// The session description of the stream the host has prepared.
fn describe(config: &MoonlightStreamConfig) -> io::Result<String> {
    let url = config.rtsp_session_url.as_deref().ok_or_else(|| problem("no RTSP address"))?;
    let target: RtspAddr = url.parse().map_err(|_| problem("an RTSP address Leto cannot read"))?;
    // Hosts name themselves in the address. Where to connect is the address
    // the PC was reached at, as for the rest of the stream.
    let ip: IpAddr = config.address.parse().map_err(|_| problem("no address for the PC"))?;
    let key = target.encrypted.then_some(&config.encryption.aes_key);

    let mut stream =
        TcpStream::connect_timeout(&SocketAddr::new(ip, target.addr.port()), PATIENCE)?;
    stream.set_read_timeout(Some(PATIENCE))?;
    stream.set_write_timeout(Some(PATIENCE))?;
    stream.write_all(&seal(key, &request(&target.to_string(), ip))?)?;
    // The host closes the connection behind its answer.
    let mut answer = Vec::new();
    stream.take(LIMIT).read_to_end(&mut answer)?;

    let answer = open(key, &answer)?;
    let (head, description) = answer.split_once("\r\n\r\n").unwrap_or((answer.as_str(), ""));
    if !head.starts_with("RTSP/1.0 200") {
        return Err(problem(head.lines().next().unwrap_or("an empty answer")));
    }
    Ok(description.to_owned())
}

/// The request for the description, as Moonlight writes it.
fn request(target: &str, host: IpAddr) -> String {
    format!(
        "DESCRIBE {target} RTSP/1.0\r\n\
         CSeq: {NUMBER}\r\n\
         X-GS-ClientVersion: 14\r\n\
         Host: {host}\r\n\
         Accept: application/sdp\r\n\
         If-Modified-Since: Thu, 01 Jan 1970 00:00:00 GMT\r\n\
         \r\n"
    )
}

/// `message` as it travels: as it is, or encrypted with the key of the
/// stream for a host that encrypts the handshake.
fn seal(key: Option<&AesKey>, message: &str) -> io::Result<Vec<u8>> {
    let Some(key) = key else { return Ok(message.as_bytes().to_vec()) };
    let length = u32::try_from(message.len()).map_err(|_| problem("a request too long"))?;
    let mut encrypted = vec![0u8; message.len()];
    let mut tag = [0u8; 16];
    RustCryptoBackend
        .encrypt_aes_gcm(&key.0, &nonce(NUMBER, b'C'), message.as_bytes(), &mut encrypted, &mut tag)
        .map_err(|_| problem("the request could not be encrypted"))?;

    let mut sealed = Vec::with_capacity(HEADER + encrypted.len());
    sealed.extend_from_slice(&(length | ENCRYPTED).to_be_bytes());
    sealed.extend_from_slice(&NUMBER.to_be_bytes());
    sealed.extend_from_slice(&tag);
    sealed.extend_from_slice(&encrypted);
    Ok(sealed)
}

/// The text of the host's `answer`.
fn open(key: Option<&AesKey>, answer: &[u8]) -> io::Result<String> {
    let plain = match key {
        None => answer.to_vec(),
        Some(key) => {
            let (header, rest) =
                answer.split_at_checked(HEADER).ok_or_else(|| problem("an answer cut short"))?;
            let word = |at: usize| {
                u32::from_be_bytes([header[at], header[at + 1], header[at + 2], header[at + 3]])
            };
            let (type_and_length, number) = (word(0), word(4));
            if type_and_length & ENCRYPTED == 0 {
                return Err(problem("an answer in clear to an encrypted request"));
            }
            let length = (type_and_length & !ENCRYPTED) as usize;
            let encrypted = rest.get(..length).ok_or_else(|| problem("an answer cut short"))?;
            let mut plain = vec![0u8; length];
            RustCryptoBackend
                .decrypt_aes_gcm(&key.0, &nonce(number, b'H'), encrypted, &header[8..], &mut plain)
                .map_err(|_| problem("an answer that was not written with the stream's key"))?;
            plain
        }
    };
    String::from_utf8(plain).map_err(|_| problem("an answer that is not text"))
}

/// What an RTSP message is encrypted with, besides the key: its number, and
/// who wrote it (`C` for the client, `H` for the host).
fn nonce(number: u32, origin: u8) -> [u8; 12] {
    let mut nonce = [0u8; 12];
    nonce[..4].copy_from_slice(&number.to_le_bytes());
    nonce[10] = origin;
    nonce[11] = b'R';
    nonce
}

/// Whether a session description offers to take a microphone: it has a
/// media line for sound, which the host's own sound never gets, and says the
/// microphone can be encrypted. Leto sends no microphone in clear.
fn offers_microphone(description: &str) -> bool {
    let lines = || description.lines().map(str::trim);
    let offered = lines().any(|line| {
        line.strip_prefix("m=audio ").is_some_and(|rest| rest.split(' ').nth(1) == Some("RTP/AVP"))
    });
    let encrypted = lines()
        .filter_map(|line| line.strip_prefix("a=x-ss-general.encryptionSupported:"))
        .filter_map(|flags| flags.trim().parse::<u32>().ok())
        .any(|flags| flags & MICROPHONE != 0);
    offered && encrypted
}

fn problem(what: &str) -> io::Error {
    io::Error::other(what.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What the Apollo microphone fork answers with its microphone on (at
    /// commit 9979406), shortened.
    const APOLLO_WITH_MICROPHONE: &str = "a=x-ss-general.featureFlags:3\n\
        a=x-ss-general.encryptionSupported:15\n\
        a=x-ss-general.encryptionRequested:9\n\
        sprop-parameter-sets=AAAAAU\n\
        m=audio 48001 RTP/AVP 96\n\
        a=rtpmap:96 opus/48000/1\n\
        a=fmtp:96 minptime=10;useinbandfec=1\n\
        a=fmtp:97 surround-params=21101\n";

    #[test]
    fn a_host_offers_its_microphone_with_a_media_line_and_encryption() {
        assert!(offers_microphone(APOLLO_WITH_MICROPHONE));
        assert!(offers_microphone(&APOLLO_WITH_MICROPHONE.replace('\n', "\r\n")));

        // A stock Sunshine or Apollo: no media line, no microphone bit.
        let stock = "a=x-ss-general.featureFlags:3\n\
            a=x-ss-general.encryptionSupported:7\n\
            a=x-ss-general.encryptionRequested:1\n\
            a=fmtp:97 surround-params=21101\n";
        assert!(!offers_microphone(stock));

        // Foundation Sunshine with its microphone turned off still says it
        // could encrypt one, and has no line for it.
        let turned_off = stock.replace("encryptionSupported:7", "encryptionSupported:15");
        assert!(!offers_microphone(&turned_off));

        // A microphone that would travel in clear is not one Leto uses.
        let in_clear =
            APOLLO_WITH_MICROPHONE.replace("encryptionSupported:15", "encryptionSupported:7");
        assert!(!offers_microphone(&in_clear));
        assert!(!offers_microphone(""));
    }

    #[test]
    fn a_request_is_encrypted_the_way_a_host_reads_it() {
        let key = AesKey([7; 16]);
        let message =
            request("rtspenc://192.168.1.20:48010", "192.168.1.20".parse().expect("an address"));
        assert!(
            message.starts_with(
                "DESCRIBE rtspenc://192.168.1.20:48010 RTSP/1.0\r\nCSeq: 1279611983\r\n"
            )
        );
        assert!(message.ends_with("\r\n\r\n"));

        let sealed = seal(Some(&key), &message).expect("a request can be encrypted");
        assert_eq!(sealed.len(), HEADER + message.len());
        assert_eq!(sealed[..4], (message.len() as u32 | ENCRYPTED).to_be_bytes());
        assert_eq!(sealed[4..8], NUMBER.to_be_bytes());
        // A host decrypts with the number in the header, as written by a
        // client.
        let mut plain = vec![0u8; message.len()];
        RustCryptoBackend
            .decrypt_aes_gcm(
                &key.0,
                &nonce(NUMBER, b'C'),
                &sealed[HEADER..],
                &sealed[8..HEADER],
                &mut plain,
            )
            .expect("the host can read it");
        assert_eq!(plain, message.as_bytes());

        // Without encryption the request travels as it is.
        assert_eq!(seal(None, &message).expect("nothing to do"), message.as_bytes());
    }

    #[test]
    fn an_answer_is_read_only_when_it_was_written_with_the_key() {
        let key = AesKey([7; 16]);
        let answer = "RTSP/1.0 200 OK\r\nCSeq: 1279611983\r\n\r\na=x-ss-general.featureFlags:0\n";
        let mut encrypted = vec![0u8; answer.len()];
        let mut tag = [0u8; 16];
        RustCryptoBackend
            .encrypt_aes_gcm(&key.0, &nonce(1, b'H'), answer.as_bytes(), &mut encrypted, &mut tag)
            .expect("the answer can be encrypted");
        let mut sealed = Vec::new();
        sealed.extend_from_slice(&(answer.len() as u32 | ENCRYPTED).to_be_bytes());
        sealed.extend_from_slice(&1u32.to_be_bytes());
        sealed.extend_from_slice(&tag);
        sealed.extend_from_slice(&encrypted);

        assert_eq!(open(Some(&key), &sealed).expect("the answer opens"), answer);
        assert!(open(Some(&AesKey([8; 16])), &sealed).is_err());
        assert!(open(Some(&key), &sealed[..sealed.len() - 1]).is_err());
        assert!(open(Some(&key), &sealed[..10]).is_err());
        // An answer in clear where an encrypted one is due is not trusted.
        assert!(open(Some(&key), answer.as_bytes()).is_err());
        assert_eq!(open(None, answer.as_bytes()).expect("clear text"), answer);
    }
}
