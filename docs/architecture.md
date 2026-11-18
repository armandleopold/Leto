# Architecture

Leto is one product: the host that shares a PC, and the clients that reach it
from a desktop, a phone or a browser. This document explains how the code is
organised so that it stays one product. It follows the structure of
[Craft Anything](https://github.com/storytold/craft-anything).

## Rules

1. **One engine, several shells.** Everything Leto can do lives in libraries
   under `crates/`. The programs under `apps/` are thin: they turn clicks,
   command-line arguments or web requests into calls to those libraries. A
   feature that exists only in one shell is a bug in the design.
2. **Simple by default, Advanced on demand.** The interface opens in Simple
   mode: a list of PCs, one button to connect, a few switches. Everything a
   streaming enthusiast expects (bitrate, codecs, ports, encoders) exists, but
   behind an Advanced switch, and every setting has a default that works.
3. **Compatible with what exists.** A Moonlight or Artemis client can use a
   Leto host. A Leto client can use a Sunshine or Apollo host. Leto's own
   additions are negotiated on the side and never required.
4. **Every platform compiles, always.** Leto targets Windows, Linux, macOS,
   Android, iOS and the web, and only three of them can be tested by hand.
   The automated checks keep the others compiling on every change.
5. **The same checks everywhere.** `cargo xtask ci` on a laptop runs what the
   pipeline runs.

## Layout

```
apps/
  leto/           The graphical app (egui): desktop, and the browser through WebAssembly
  leto-cli/       The command line: the same engine, for scripts and diagnostics
crates/
  leto-core/      Shared types: brand, platforms, what the interface shows. No platform code
  leto-protocol/  Wire-level facts of the GameStream protocol. No networking
  leto-client/    The client engine: find a PC, pair, list its apps, stream one. Built on moonlight-common-rust
  leto-media/     Pictures and sound: H.264 and Opus decoders, the speakers. No networking
  leto-testhost/  A stand-in for a Sunshine or Apollo host, for tests and for trying a client
xtask/            Project automation, run with `cargo xtask`
assets/brand/     Logo, icons and colours
docs/             This documentation
```

Crates planned by the [roadmap](roadmap.md). Names can still change:

| Crate | Purpose | Step |
|---|---|---|
| `leto-return` | The return channel: capture of microphone and camera, their packets, the Leto API | 2 |
| `leto-vdevices` | The virtual microphone and camera on the host. Platform code, `unsafe` allowed | 4 |
| `apps/leto-android` | The Android app, started from Super Artemis | 3 |
| `leto-host` | The host, ported from Apollo | 4 |

## Platforms

| Platform | Client | Host | How it is verified |
|---|---|---|---|
| Windows | Yes | Yes | Tests run on Windows in the pipeline, and by hand |
| Web | Yes | No | Built in the pipeline, and by hand |
| Android | Yes | No | Compile check for now, and by hand once the app exists |
| Linux | Yes | Yes | Tests run on Linux in the pipeline |
| macOS | Yes | Yes | Compile check on every change (with zig as the C and C++ compiler), tests on `main` |
| iOS | Yes | No | Compile check only |

The code asks `leto_core::platform::Platform` what the current platform can do
instead of scattering `cfg` attributes through the interface.

## The app

`apps/leto` is an [egui](https://github.com/emilk/egui) app. The same code
draws the native window and, compiled to WebAssembly, the web client
(`cargo xtask web` builds it into `dist/web`). The look is fixed: the "Braise"
palette from `leto_core::brand`, dark, whatever the system theme.

It is made of three parts that do not reach into each other:

- `state.rs` decides what is on screen and what happens next. It draws
  nothing and opens no connection, so it is tested like any other logic.
- `backend/` does the work. On the desktop it runs `leto-client` on worker
  threads and reports back with events. A browser page cannot open sockets, so
  the web build has a backend that only explains what is missing, until the
  bridge of step 2 exists.
- `ui/` draws the screens from the state and tells the state what the person
  did.

## A stream

Starting an app goes through the same three parts, and through two crates
that know nothing of each other:

1. `leto-client` asks the PC to start the app, or to go back to it when it is
   already running, and connects the stream (`Client::play`). The video,
   the sound and the control messages are encrypted with a key made for
   that stream. What arrives is handed over still encoded, to a `VideoSink`
   and an `AudioSink`, on threads that belong to the stream.
2. The app's backend gives it two sinks built on `leto-media`. The video sink
   decodes each picture and keeps only the latest one for the window. The
   audio sink decodes each packet and queues it for the speakers.
3. The window picks the latest picture up when it redraws and shows it as
   large as fits, undistorted.

Leaving a stream keeps the app running on the PC, like closing Moonlight. The
apps screen then offers to resume it or to close it.

What this first version does, and what it leaves for later:

- Video is H.264, decoded in software by OpenH264 and converted to RGB on the
  processor. It works on every desktop with nothing to install. Decoding on
  the graphics card, H.265 and AV1 come later, behind the same sink.
- Leto asks the PC for BT.601 colours in limited range, in standard dynamic
  range. HDR comes with the decoders that can show it.
- Sound is stereo Opus, decoded in Rust and played on the system's default
  output. Between the network and the speakers sits a queue of a few tens of
  milliseconds: it waits for a small cushion before playing, fills up again
  after running dry, and skips ahead when sound piles up, so that sound never
  drifts behind the picture.
- Keyboard, mouse and gamepad are not sent yet.

The stand-in host streams for real: it speaks RTSP, encrypts, and sends a
moving test card as H.264 and a melody as Opus, so the whole path is tested
on every change without a gaming PC. The card is made to be judged at a
glance: a sun passing behind a moon shows stutter, a marker crossing the top
edge once a second shows dropped pictures, and colour bars show wrong colours.

## Trust between a client and a PC

Pairing gives each side the other's certificate. From then on the client only
talks to a PC that presents exactly that certificate, and the PC only answers
a client it knows. `leto-client` has its own HTTPS transport for this, because
the ones shipped with moonlight-common-rust accept any certificate. A PC that
was reinstalled therefore has to be paired again, and something else on the
network cannot pass for it.

## Dependencies and licences

Leto is GPL-3.0-only because the host is a port of Apollo, which is
GPL-3.0-only (see [inventory.md](inventory.md)). `deny.toml` lists the licences
accepted for dependencies and the pipeline rejects anything else, along with
known security advisories. An advisory can only be accepted in `deny.toml`
with a comment that says why it does not apply and when to remove it.

## Unsafe code

`unsafe` is denied across the workspace. A crate that has to call the
operating system directly (capture, codecs, virtual devices) opts back in with
`#![allow(unsafe_code)]` at its top, and keeps that code in as few modules as
possible.
