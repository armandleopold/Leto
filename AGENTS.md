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
cargo run -p leto-cli -- pcs
cargo xtask ci             # everything the pipeline checks
cargo xtask web            # the web client, into dist/web
cargo run -p leto-cli -- bridge   # serves the web client to browsers
```

Building needs a C++ compiler, for the H.264 codec (OpenH264, built from
source). With [NASM](https://www.nasm.us) on the `PATH` the codec is built
with its fast assembly routines; without it the build still works and
decoding is several times slower. On Linux, sound needs the ALSA headers,
gamepads need the udev headers and the camera needs libclang
(`libasound2-dev`, `libudev-dev` and `libclang-dev` on Debian and Ubuntu).

No gaming PC at hand? This keeps a stand-in host running on this machine. The
app finds it, and you type the PIN the app shows into its terminal. It streams
a test card and a melody, and draws on the card what your keyboard, mouse and
gamepad do. Type `close` to close the running app as its owner would, `drop`
to break the stream, or `inputs` to list what it received:

```sh
cargo run -p leto-testhost --example serve -- --port 47989 --announce
```

With `--apollo` it behaves as an Apollo that already knows another device: a
newly paired one may list the apps and watch, and nothing else. Type `allow`
to give it every permission, as on Apollo's PIN page, and `restrict` to take
them back.

The stand-in host takes a microphone, like the microphone build of Apollo. A
meter on the right of its test card rises with what the client sends, and
`voice` saves the last ten seconds it heard to `leto-voice.wav`, to listen
to. With `--no-microphone` it takes none, like a stock Sunshine or Apollo.

It takes a camera too, as Leto's own host will and as no Sunshine or Apollo
does: what a client sends shows in a corner of its test card. `camera` says
what arrived and saves the last picture to `leto-camera.png`, `camera off`
refuses new ones as an owner would, and `camera on` takes them again. With
`--no-camera` it takes none.

To check a stream without the window, `leto-cli play` streams an app for a few
seconds, decodes it and reports what arrived, and `leto-cli quit` closes it:

```sh
cargo run -p leto-cli -- play Salon Desktop --seconds 10
```

It also says whether the PC takes a microphone and a camera, and with
`--microphone` and `--camera` it sends this device's meanwhile.

The web client needs its bridge: `cargo xtask web` builds the client, then
`leto-cli bridge` serves it on port 5386 and prints the address to open and
the code a new browser has to be given. The address is HTTPS with a
certificate the bridge made itself, so the browser warns once. With `--local`
only this computer can reach it. The bridge keeps its certificate and the
browsers it knows in a `bridge` folder next to the client's settings.

To check the browser's microphone without one, start Chromium with
`--use-fake-device-for-media-stream --use-fake-ui-for-media-stream
--use-file-for-fake-audio-capture=<a WAV file>`: the file plays as the
microphone, and `voice` on the stand-in host saves what arrived. The browser
treats a steady tone as noise and turns it down, so use a voice, or a tone
only to check the pitch. The same two first flags give the browser a made-up
camera.

The desktop app has a made-up camera too: with `LETO_CAMERA=card` in its
environment, the screen "Camera and microphone" shows a test card instead of
the system's camera, read and converted as a camera's pictures are. The same
card is what the app and `leto-cli play --camera` then send to a PC.

`cargo xtask ci` runs, in order: `fmt`, `clippy` (warnings are errors), `test`,
and `targets` (a compile check for macOS, the web, Android and iOS). Each is
also a command of its own, for example `cargo xtask clippy`. Run
`cargo xtask ci` before opening a pull request.

The toolchain version is pinned in `rust-toolchain.toml`, and rustup installs
it together with the targets on first use. Two commands need one more tool
each, and say how to install it when it is missing:

- `cargo xtask web` needs `wasm-bindgen`.
- The macOS part of `cargo xtask targets` needs zig (`pipx install ziglang`),
  as the C and C++ compiler for the TLS code and the H.264 codec. Without zig
  that part is skipped on your machine. The pipeline always runs it.

The Windows build is not part of `targets`: the Windows job of the pipeline
builds and tests it for real.

## Where things go

- `crates/`: everything Leto can do. Libraries, tested, no user interface.
- `apps/`: thin shells over the crates. If a shell contains logic another
  shell would need, move that logic into a crate.
- In `apps/leto`, what the app does goes in `state.rs` with a test, the work
  goes in `backend/`, and `ui/` only draws. See `docs/architecture.md`.
- `leto-client` hands a stream over still encoded and `leto-media` decodes
  it. Neither depends on the other, outside the tests of `leto-client`: the
  app's backend joins them. The camera goes the other way under the same
  rule: `leto-media` encodes it and hands it to a `Carrier`, which the app
  makes out of the engine.
- What a client and a host say about the camera, and its packets, are in
  `leto-return`, which does no networking. The client, the stand-in host and
  later the host all use it: change it there, not at one end.
- `leto-bridge` relays a stream to a browser still encoded, and the browser
  decodes it. What the two say to each other is in `leto_core::bridge`:
  change both ends together.
- Code of the web build that needs no browser goes outside
  `apps/leto/src/backend/web/`, where the desktop tests run it. Nothing runs
  a browser in the pipeline.
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
- **A paired PC is recognised by its certificate.** Never add a way to talk to
  a paired PC that skips that check.
- **A browser has to be known to the bridge.** Never add a way to ask the
  bridge for anything, or to send it input, that skips its gate.
- **A log tells no secret.** People send their logs to whoever helps them.
  Never log a PIN, a key, a bridge code, or the query of a request (during
  pairing it carries the challenges). See `leto_client::logs`.
- **Code ported from another project** says so in a comment at the top of the
  module: which project, which file, which commit.
- Write code, comments, documentation and commit messages in English.

## Testing by hand

Only Windows, Android and the web can be tested by hand at the moment. A change
to Linux, macOS or iOS code must at least keep `cargo xtask targets` green, and
should say in the pull request that it was not run on a real device.
