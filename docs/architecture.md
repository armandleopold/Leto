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
  leto-media/     Pictures and sound: H.264 and Opus decoders, the speakers, the microphone. No networking
  leto-bridge/    Lets browsers use the client engine: serves the web client and relays streams to it
  leto-testhost/  A stand-in for a Sunshine or Apollo host, for tests and for trying a client
xtask/            Project automation, run with `cargo xtask`
assets/brand/     Logo, icons and colours
docs/             This documentation
```

Crates planned by the [roadmap](roadmap.md). Names can still change:

| Crate | Purpose | Step |
|---|---|---|
| `leto-return` | The part of the return channel that is Leto's own: the camera, its packets, the Leto API. The microphone follows an existing protocol and lives in `leto-client` and `leto-media` | 2 |
| `leto-vdevices` | The virtual microphone and camera on the host. Platform code, `unsafe` allowed | 4 |
| `apps/leto-android` | The Android app, started from Super Artemis | 3 |
| `leto-host` | The host, ported from Apollo | 4 |

## Platforms

| Platform | Client | Host | How it is verified |
|---|---|---|---|
| Windows | Yes | Yes | Tests run on Windows in the pipeline, and by hand |
| Web | Yes, through the bridge | No | Built in the pipeline, and by hand |
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
- `backend/` does the work, and reports back with events. On the desktop it
  runs `leto-client` on worker threads. A browser page cannot open sockets, so
  the web build asks the bridge that served the page, which runs the same
  engine: see [The web client and its bridge](#the-web-client-and-its-bridge).
- `ui/` draws the screens from the state and tells the state what the person
  did.

## A stream

What a shell asks of the engine and what it hears back are the same for all
of them: `Request` and `Event` in `leto_core::engine`. `leto_client::Service`
does the asking on worker threads for a `Shell`, which is whoever gets the
news and provides the sinks of a stream. The desktop app is one shell, each
browser on the bridge is another.

Starting an app goes through the three parts of the app, and through two
crates that know nothing of each other:

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

Apollo lets its owner decide what each paired device may do. The first device
paired with it may do everything. Every device paired after that may only
list the apps and watch one that is already running, until the owner ticks
more boxes on Apollo's PIN page, under "Device Management". Apollo reports the
permissions in `serverinfo`, so the engine checks them before it asks, and
says which one is missing (`Error::NotAllowed`) instead of passing on Apollo's
bare "Permission denied".

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

The stand-in host streams for real: it speaks RTSP, encrypts, and sends a
moving test card as H.264 and a melody as Opus, so the whole path is tested
on every change without a gaming PC. The card is made to be judged at a
glance: a sun passing behind a moon shows stutter, a marker crossing the top
edge once a second shows dropped pictures, and colour bars show wrong colours.

## Keyboard, mouse and gamepads

What a person does is described once, in `leto_core::input`: a key by its
Windows code, a mouse movement or position, a wheel turn, the state of a
gamepad. The app produces these and `Session::send` turns them into Moonlight
control messages. In the browser the same app produces them and the bridge
passes them on.

- **Keys are named by where they sit**, not by what the layout prints on
  them: the code sent is the one of the American keyboard for that place, and
  the PC applies its own layout. A French keyboard types French on a French
  PC. A key that is held is sent once, since the PC repeats on its own.
- **Order is kept.** moonlight-common-rust holds mouse movement back for up
  to a millisecond while clicks leave at once, so a click could overtake the
  movement before it. The engine sends movement and the wheel around that
  queue. The app merges movement that follows movement, never across a click.
- **The mouse has two modes.** By default the pointer of the PC goes where
  the pointer of this device is on the picture. The "game mouse" sends how
  far the mouse moved instead, with the pointer kept inside the window, for
  games that turn the view with it.
- **While a stream is connected the keyboard is the PC's.** The app takes
  the keys away from egui before it acts on them (`raw_input_hook`), Tab and
  Escape included. Four combinations stay on this device, as in Moonlight:
  Ctrl+Alt+Shift with Q leaves, with X fills the screen, with M switches the
  mouse mode, with T turns the microphone on or off. Everything held down is
  let go of when the window loses the keyboard and before leaving, so nothing
  stays stuck on the PC.
- eframe would turn Ctrl+C, Ctrl+X and Ctrl+V into clipboard events and keep
  those keys from the PC, so that is turned off, and `clipboard.rs` does it
  for Leto's own text fields when no stream has the keyboard.
- **Gamepads** are followed by one thread of the backend (`backend/gamepad.rs`,
  on [gilrs](https://gitlab.com/gilrs-project/gilrs)). Each stream hears of
  every gamepad once, then of each change. Buttons are named by place: A is
  the lowest of the four. A browser says nothing when a button moves, so the
  web build looks at the gamepads itself, 125 times a second while a stream is
  connected (`backend/web/gamepad.rs`). What a PC is told is decided once for
  both, in `backend/pads.rs`.
- Apollo decides per device which of keyboard, mouse and gamepad it accepts.
  The engine reports it when the stream connects (`Session::input_allowed`),
  and the bar over the picture says which permission to tick when one is
  missing, again whenever a refused key or click is tried.

The stand-in host records what it receives (`FakeHost::inputs`) and draws it
on its test card: a disc where the pointer is, lit while a button is held, a
square per key held, and a dial for the first gamepad.

## The microphone

A PC can take the microphone of the device that watches it, when its host
knows how: the microphone build of Apollo and Foundation Sunshine today,
Leto's own host later. The protocol is theirs and is described in
[return-channel.md](return-channel.md). The switch is in the bar over the
picture.

- **The engine asks first.** A host says that it takes a microphone in the
  session description of the RTSP handshake. moonlight-common-rust has to
  know before that handshake whether to set the stream up, and only does so
  for a host it knows as Foundation Sunshine. So the engine asks for the
  description itself, on a connection of its own
  (`leto-client/src/describe.rs`), and presents a host that offers a
  microphone to the library under that name, which changes nothing else in
  it. An encrypted request is encrypted with its number: this one has a
  number the handshake that follows never reaches.
- **Never in clear.** The microphone travels encrypted with the key of the
  stream. A host that does not offer that is treated as taking none.
- **What is sent** is what those hosts decode: one channel at 48 kHz, in Opus
  frames of twenty milliseconds. `leto-media` captures the system's default
  input (`Microphone`), mixes its channels into one, brings it to 48 kHz and
  encodes it (`VoiceEncoder`), and `Session::send_microphone` sends each
  frame. The news that a stream is connected says whether the PC takes a
  microphone. In a browser the same frames are made by the browser, and
  reach the engine through the bridge: see
  [the web client](#the-web-client-and-its-bridge).
- **Off until the person turns it on,** in every stream. The system, or the
  browser, is asked for the microphone the first time the switch is turned
  on. While it is on, the switch is lit and fills with what the microphone
  hears, and a small microphone stays in a corner of the picture when the
  bar is away. A PC that takes none has the switch dimmed, with the reason.
  A browser asks the person before it hands a microphone over: until it
  answers, the switch is not lit and the bar says that the PC hears nothing
  yet.
- **Nothing is captured without a listener.** The backend only opens the
  microphone for a connected stream to a PC that takes one, and lets go of
  it when the stream is left, fails or ends, on the engine's thread, without
  waiting for the window. In a browser the microphone belongs to the stream
  the page is showing and goes with it, and one that the browser hands over
  after the switch went off is let go of unused.

The stand-in host takes a microphone the way the Apollo build does: it
decodes what arrives, shows how loud it is on the test card, and saves it as
a sound file when asked (`voice` in its terminal), to be judged by ear.
`leto-cli play --microphone` sends the microphone without the window, and
says whether a PC takes one at all.

Opus is encoded by `opus-rs` and decoded by `opus-decoder`, both in plain
Rust. Three seconds of the encoder's frames, a quiet room with sudden bursts,
were given to libopus 1.4 as a referee: it decodes them all, to the same
samples as `opus-decoder`.

## The web client and its bridge

A browser page cannot open the sockets the Moonlight protocol needs. The
bridge can. It is a library, `leto-bridge`, started with
`leto-cli bridge` for now: it serves the web client that `cargo xtask web`
built and runs the client engine for each browser that opens it. To a PC the
bridge is one more paired device. Every browser on a bridge sees the PCs that
bridge is paired with.

```
browser  <-- HTTPS and a WebSocket -->  bridge  <-- the Moonlight protocol -->  PC
```

- **HTTPS, with a certificate of its own.** Browsers only give a page their
  decoders, their microphone and their camera when it came over HTTPS. The
  bridge makes itself a certificate on first run, for its name and its
  addresses, and renews it before it runs out. No authority vouches for it, so
  a browser warns once and the person goes on. `leto-cli bridge` prints the
  certificate's fingerprint to compare. Plain HTTP on the same port is sent
  to HTTPS.
- **A browser has to be known.** Whoever reaches the bridge reaches the PCs,
  keyboard and mouse included. A new browser is asked for the eight digits the
  bridge shows where it runs, then keeps a key in its own storage. The bridge
  keeps only a hash of that key. A wrong code is answered after two seconds
  during which no code is looked at, five wrong codes draw a new code, and
  each code lets one browser in. The WebSocket is only opened for the page the
  bridge served itself.
- **One language**, in `leto_core::bridge`: the browser sends `Request`s and
  `Input`s, the bridge sends `Event`s, as JSON. Pictures and sound, and the
  microphone the other way, travel as binary messages and stay encoded all
  the way: the bridge never decodes.
- **The browser decodes**, with WebCodecs. The bridge reads the codec of each
  keyframe from its parameters and says it before the first picture.
  Pictures are drawn on a canvas of their own *behind* the app's canvas, and
  the app leaves a hole over it: a picture goes from the decoder to the
  screen without egui drawing a frame. Sound is decoded the same way and
  played piece after piece, 40 ms ahead of the speakers, skipping ahead when
  more than 120 ms piles up.
- **Late is worse than lost.** A WebSocket never loses anything, so a browser
  that falls behind would see the past. The bridge lets 30 pictures wait for
  a browser, then drops what comes until a keyframe finds room, and asks the
  PC for that keyframe. The browser does the same with its decoder, and asks
  the bridge for a keyframe when a picture fails to decode.
- **The browser encodes its microphone**, with WebCodecs too. It captures
  with the echo cancellation, the noise suppression and the gain control it
  has for calls, and brings the result to the rate of the page's sound,
  48 kHz. A script on its sound thread (`web/microphone.js`, an audio
  worklet) cuts that into pieces of twenty milliseconds, its Opus encoder
  makes one packet of each, and the page sends each packet as one message.
  The bridge hands them to the stream that browser is watching. They go the
  way input does: a browser the gate does not know has no engine to take
  them.
- **Input** is the desktop app's code: the same keys by place, the same two
  mouse modes, the same shortcuts. What differs is asked of the browser
  in one module, `window.rs`: the full screen, the pointer for the game
  mouse, and the keys. While a stream has the keyboard, the browser's own
  shortcuts are kept from acting where the browser allows it, and in full
  screen Chrome and Edge also hand over Escape and the system's shortcuts.

The rules that need no browser are plain Rust, tested with the rest: when a
piece of sound plays and which pictures a decoder may be given
(`backend/flow.rs`), the gate, the relay, the HTTP the bridge speaks. The
bridge is tested end to end against the stand-in host with a WebSocket
client. The browser's half was checked by hand in Chromium, against the same
host: nothing runs a browser in the pipeline yet.

## Trust between a client and a PC

Pairing gives each side the other's certificate. From then on the client only
talks to a PC that presents exactly that certificate, and the PC only answers
a client it knows. `leto-client` has its own HTTPS transport for this, because
the ones shipped with moonlight-common-rust accept any certificate. A PC that
was reinstalled therefore has to be paired again, and something else on the
network cannot pass for it.

The bridge adds nothing to this and takes nothing away: it is the paired
device, and the check is the engine's. What it adds is the gate described
above, between itself and browsers.

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
