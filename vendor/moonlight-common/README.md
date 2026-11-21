# moonlight-common, as Leto builds it

This is the protocol crate of
[moonlight-common-rust](https://github.com/MrCreativ3001/moonlight-common-rust)
at commit `5a3aa1f57bf70bb6a06c40bc435a18b2be44a252`, under its own licence,
the GPL 3.0 or later (`LICENSE`). Leto used to take it from that commit with
git; it is copied here because Leto needs a change the commit does not have.

Only the crate itself is here: its source and licence. The C bindings, the
examples and what its tests and examples need were left out of `Cargo.toml`.
It is not a member of Leto's workspace, so Leto's lints and tests leave it
alone, as they did when it came from git.

## Leto's changes

Each is marked with a comment that starts with `Leto:`.

- `src/stream/proto/mod.rs`: AV1 is chosen when the host offers it in its
  DESCRIBE and the client says it decodes it, before H.265 and H.264, as
  moonlight-common-c does. The upstream commit never chose AV1. The rest of
  the crate already carried AV1 pictures.
- `src/stream/proto/sdp/server.rs`: a host offers AV1 with the line
  `a=rtpmap:98 AV1/90000`, which the commit read as an attribute named
  `rtpmap` and so never recognised. It is recognised now, as
  moonlight-common-c does: `AV1/90000` anywhere in that attribute.
- `src/stream/std/driver.rs`: a UDP socket reads on when the system reports
  that a packet it sent met a closed port (a reset connection on Windows, a
  refused one elsewhere), as moonlight-common-c does. The commit stopped the
  whole stream on it.
- `src/stream/proto/audio/payloader.rs`: the recovery packets of encrypted
  sound are as long as its data packets, as Sunshine and Apollo send them.
  The commit made them longer and panicked. Only Leto's stand-in host sends
  sound with this crate.
- `src/stream/proto/control/packet.rs`: the state of a gamepad is sent
  reliably, as moonlight-common-c sends it. The commit sent it unreliable
  and sequenced, so a state lost on the way was never sent again, and a
  button let go stayed held on the PC until the gamepad moved again.
- `src/stream/std/mod.rs`, `src/stream/std/driver.rs` (`last_words`),
  `src/stream/proto/control/stream.rs` (`disconnect_now`): a stream that is
  stopped tells the host that the client leaves, in one packet sent before
  the threads stop, as moonlight-common-c does (`enet_peer_disconnect_now`).
  The commit said nothing, so the host went on streaming to nobody until it
  gave up on the silence, seconds to a minute later.
- `src/stream/std/driver.rs` and `src/stream/std/mod.rs`: the control
  connection is told the time before the application sends over it
  (`stream_mut_now`). The commit only told it when a packet arrived or a
  timeout fired, so a message sent in between was stamped with a time up to
  50 ms old, and the way to the host and back, which is measured from that
  stamp, came out longer whenever something was sent: 9 ms on the loopback
  interface with the mouse moving, where it is under 1 ms.
- `src/stream/proto/enet.rs` (`RoundTrip`, `acknowledged_sent_times`) and
  `src/stream/proto/control/peer.rs`: the way to a peer and back is a mean
  that keeps its fractions, made from the acknowledgements as they arrive.
  ENet's own, which the commit reported, is in whole milliseconds and moves
  by an eighth of each difference, rounded down: once raised, it comes down
  to 7 ms above the truth and stays, so a host 1 ms away read 8 ms. Its
  tests are at the end of `enet.rs`; like the rest of this crate's, they
  do not build here (the crate's test helper needs what Leto leaves out).

## Updating

Copy `src/` from a newer commit, apply the changes above again, and compare
its `Cargo.toml` with this one. Once a commit upstream chooses AV1 itself,
sends the state of a gamepad reliably and stamps what is sent with the time
it is sent at, Leto can go back to taking the crate with git.
