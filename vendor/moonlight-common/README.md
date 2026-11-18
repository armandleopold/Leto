# moonlight-common, as Leto builds it

This is the protocol crate of
[moonlight-common-rust](https://github.com/MrCreativ3001/moonlight-common-rust)
at commit `5a3aa1f57bf70bb6a06c40bc435a18b2be44a252`, under its own licence,
the GPL 3.0 or later (`LICENSE`). Leto used to take it from that commit with
git; it is copied here so that Leto can change it.

Only the crate itself is here: its source and licence. The C bindings, the
examples and what its tests and examples need were left out of `Cargo.toml`.
It is not a member of Leto's workspace, so Leto's lints and tests leave it
alone, as they did when it came from git.

## Leto's changes

None yet: this is the commit as it is. A change made here is marked with a
comment that starts with `Leto:`, and listed here.

## Updating

Copy `src/` from a newer commit, apply the changes above again, and compare
its `Cargo.toml` with this one. Once Leto needs no change of its own, it
can go back to taking the crate with git.
