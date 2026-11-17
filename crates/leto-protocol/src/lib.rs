//! The wire protocol Leto speaks.
//!
//! Leto stays compatible with the Moonlight/GameStream protocol as implemented
//! by Sunshine and Apollo, so existing clients and hosts keep working. On top
//! of it, Leto adds a small number of extensions, negotiated so that a peer
//! that does not know them is never affected. See `docs/return-channel.md`.
//!
//! This crate holds protocol facts shared by the host and the clients. It has
//! no I/O and compiles on every platform Leto targets.

pub mod ports;

/// mDNS service type a host advertises so clients can find it on the LAN.
pub const MDNS_SERVICE_TYPE: &str = "_nvstream._tcp";
