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
  leto-core/      Shared types: brand, platforms. No platform code
  leto-protocol/  Wire-level facts of the GameStream protocol. No networking
xtask/            Project automation, run with `cargo xtask`
assets/brand/     Logo, icons and colours
docs/             This documentation
```

Crates planned by the [roadmap](roadmap.md). Names can still change:

| Crate | Purpose | Step |
|---|---|---|
| `leto-client` | A streaming session seen from the client: discovery, pairing, connect, built on moonlight-common-rust | 2 |
| `leto-media` | Decoding and encoding behind one interface per platform, Opus | 2 |
| `leto-return` | The return channel: capture of microphone and camera, their packets, the Leto API | 2 |
| `leto-vdevices` | The virtual microphone and camera on the host. Platform code, `unsafe` allowed | 2 |
| `apps/leto-android` | The Android app, started from Super Artemis | 3 |
| `leto-host` | The host, ported from Apollo | 4 |

## Platforms

| Platform | Client | Host | How it is verified |
|---|---|---|---|
| Windows | Yes | Yes | Tests run on Windows in the pipeline, and by hand |
| Web | Yes | No | Built in the pipeline, and by hand |
| Android | Yes | No | Compile check for now, and by hand once the app exists |
| Linux | Yes | Yes | Tests run on Linux in the pipeline |
| macOS | Yes | Yes | Compile check on every change (with zig as the C compiler), tests on `main` |
| iOS | Yes | No | Compile check only |

The code asks `leto_core::platform::Platform` what the current platform can do
instead of scattering `cfg` attributes through the interface.

## The app

`apps/leto` is an [egui](https://github.com/emilk/egui) app. The same code
draws the native window and, compiled to WebAssembly, the web client
(`cargo xtask web` builds it into `dist/web`). The look is fixed: the "Braise"
palette from `leto_core::brand`, dark, whatever the system theme.

## Dependencies and licences

Leto is GPL-3.0-only because the host is a port of Apollo, which is
GPL-3.0-only (see [inventory.md](inventory.md)). `deny.toml` lists the licences
accepted for dependencies and the pipeline rejects anything else, along with
known security advisories.

## Unsafe code

`unsafe` is denied across the workspace. A crate that has to call the
operating system directly (capture, codecs, virtual devices) opts back in with
`#![allow(unsafe_code)]` at its top, and keeps that code in as few modules as
possible.
