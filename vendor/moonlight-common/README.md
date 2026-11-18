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

## Updating

Copy `src/` from a newer commit, apply the changes above again, and compare
its `Cargo.toml` with this one. Once a commit upstream chooses AV1 itself,
Leto can go back to taking the crate with git.
