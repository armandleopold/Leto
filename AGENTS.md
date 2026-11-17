# Working on Leto

Guide for anyone changing this repository, people and coding agents alike.

## What Leto is

One app for remote desktop and game streaming, written in Rust: the host that
shares a PC and the clients for desktop, mobile and the web. It stays
compatible with the Moonlight protocol and starts from Apollo, Artemis and
Super Artemis. Read `docs/architecture.md` first, then `docs/roadmap.md`.

## Commands

```sh
cargo run -p leto          # the app
cargo run -p leto-cli -- about
cargo xtask ci             # everything the pipeline checks
cargo xtask web            # the web client, into dist/web
```

`cargo xtask ci` runs, in order: `fmt`, `clippy` (warnings are errors), `test`,
and `targets` (a compile check for Windows, macOS, the web, Android and iOS).
Each is also a command of its own, for example `cargo xtask clippy`. Run
`cargo xtask ci` before opening a pull request.

The toolchain version is pinned in `rust-toolchain.toml`, and rustup installs
it together with the targets on first use. `cargo xtask web` also needs
`wasm-bindgen`, and prints the exact install command when it is missing.

## Where things go

- `crates/`: everything Leto can do. Libraries, tested, no user interface.
- `apps/`: thin shells over the crates. If a shell contains logic another
  shell would need, move that logic into a crate.
- `xtask/`: automation. Add a command there instead of a shell script.
- `docs/`: decisions and designs. Update them in the same pull request as the
  code they describe.
- `assets/brand/`: logo and colours. The colours also live in
  `crates/leto-core/src/brand.rs`. Change both together.

## Rules

- **Simple by default.** A new setting needs a default that works and belongs
  behind Advanced mode unless most people must touch it.
- **Stay compatible.** Do not change a GameStream message in a way a Moonlight
  or Artemis client, or a Sunshine or Apollo host, would not understand. Leto's
  own features are negotiated separately. See `docs/return-channel.md`.
- **Every target compiles.** Code that only makes sense on one platform goes
  behind `cfg` in a platform module, with a fallback that compiles elsewhere.
- **No `unsafe`** outside crates that opted in with `#![allow(unsafe_code)]`.
- **No `unwrap`, `todo!` or `dbg!`** in committed code. Use `expect` with a
  message that says why it cannot fail, or return an error.
- **Dependencies** must pass `cargo deny check` (licences in `deny.toml`).
  Leto is GPL-3.0-only.
- **Code ported from another project** says so in a comment at the top of the
  module: which project, which file, which commit.
- Write code, comments, documentation and commit messages in English.

## Testing by hand

Only Windows, Android and the web can be tested by hand at the moment. A change
to Linux, macOS or iOS code must at least keep `cargo xtask targets` green, and
should say in the pull request that it was not run on a real device.
