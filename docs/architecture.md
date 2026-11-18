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
  leto-media/     Pictures and sound: H.264 and Opus decoders, the speakers, the microphone, the camera and its encoder. No networking
  leto-system/    What the app asks of the system itself: every key for a stream, and what the screen is like. Windows only for now, `unsafe` allowed
  leto-return/    The part of the return channel that is Leto's own: the packets of the camera and the API it is asked on. No networking
  leto-bridge/    Lets browsers use the client engine: serves the web client and relays streams to it
  leto-testhost/  A stand-in for a Sunshine or Apollo host, for tests and for trying a client
xtask/            Project automation, run with `cargo xtask`
assets/brand/     Logo, icons and colours
docs/             This documentation
```

Crates planned by the [roadmap](roadmap.md). Names can still change:

| Crate | Purpose | Step |
|---|---|---|
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

A stream is made for the screen it is shown on. When the person picks an
app, the window says what its screen is like (`window::screen`): its pixels
and its pace as Windows reports them (`leto_system::display`), elsewhere as
egui does at 60 pictures per second, and how large the system draws text.
The request carries it (`Request::Play::screen`), and the engine asks for a
stream of that size and pace (`StreamSettings::for_screen`), within what
H.264 carries, with a bit rate that grows with the pixels from Moonlight's
20 Mbit/s for Full HD at 60. It also asks the PC for a screen of its own in
that mode and text size (`virtualDisplay` and `scaleFactor`), which Apollo
makes for the stream. A screen the owner set for the device on Apollo's side
wins. Sunshine ignores the question and shows its own screen. A browser says
its screen's size but not its pace: it gets 60, lying down.

The window fills the screen as the stream connects, and gives it back when
the stream is left. Ctrl+Alt+Shift+S shows statistics in a corner, as in
Moonlight: what the PC sends, what arrives, what is lost, decoded and shown
per second, how long a picture takes to decode and to make on the PC, and the
time to the PC and back (`stats.rs`, counted by the backend).

The work follows the person's attention. While another window has the
keyboard the video sink still decodes every picture, since the next ones
build on it, but turns only one a second into pixels, which is most of the
work; the window then draws once a second. While the window is out of sight
nothing is decoded, and the PC is asked for a keyframe when it is back. What
the PC sends stays the same: the protocol has no way to slow a PC down during
a stream, and Moonlight does not either (it can mute the sound when it loses
the keyboard). A browser slows a page out of sight down by itself.

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
  Escape included. Some combinations stay on this device, as in Moonlight,
  found by the letter the layout prints: Ctrl+Alt+Shift with Q leaves, with X
  fills the screen, with M switches the mouse mode, with T turns the
  microphone on or off, with C the camera, with S the statistics, and with Z
  gives the keyboard and the mouse back to this device until Z again or a
  click on the picture. Everything held down is let go of when the window
  loses the keyboard, when it is given back and before leaving, so nothing
  stays stuck on the PC.
- **On Windows the system's own shortcuts are the PC's too**: the Windows
  key, Alt+Tab, Alt+F4. Windows acts on them before any window hears of them,
  so while a stream has the keyboard `leto-system` listens to the keyboard
  ahead of Windows with a low-level hook, on a thread of its own, and keeps
  every key from going further while Leto's window is in front. Keys are
  named by their scan code, the numeric keypad included. A key goes back to
  whoever got it go down, so nothing stays held on either side, and the Ctrl
  that AltGr makes up is dropped: the PC makes up its own. Ctrl+Alt+Del and
  Windows+L stay with Windows, which no program can take.
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

## The camera

The camera of the device that watches is meant to appear on the PC as a
webcam: see [return-channel.md](return-channel.md). What exists is the side
of the device: capturing it, showing it to the person, and sending it to a PC
that takes one. No Sunshine or Apollo does, and Leto's own host is not written
yet, so the only PC that takes it today is the stand-in host of the tests.

- **A screen of its own.** "Camera and microphone", reached from the list of
  PCs, shows what the camera sees and how loud the microphone is, with no PC
  involved. Both are off at every visit, each has its switch, and leaving the
  screen lets go of both. While one captures, its icon is lit and a line
  under it says so.
- **On the desktop**, `leto-media` reads the camera (`Camera`) through
  nokhwa: Media Foundation on Windows, V4L2 on Linux, nothing on macOS yet.
  It opens the first camera that has colour, because laptops that recognise a
  face list a second one that sees in grey. Among the formats it can read
  (YUYV, NV12, Motion JPEG, grey) it takes the one closest to 1280 by 720 at
  thirty pictures a second, a smooth one before a large one
  (`camera/format.rs`). It turns the pictures into pixels itself
  (`camera/pixels.rs`), as BT.601 in limited range, tolerating rows that a
  system padded and a compressed picture that arrives cut short.
- **One thread for every camera.** Opening a camera takes a moment and
  reading it waits for each picture, so neither happens where the window is
  drawn, and Windows wants its cameras used from the thread that first asked
  about them. One thread opens and reads them, one after the other. The
  window takes the latest picture when it draws, and a camera that was let
  go of before its turn is never opened.
- **In a browser**, the page asks for the camera, plays it in a `<video>`
  that nobody sees and copies it through a canvas into the window, at most
  thirty times a second and no larger than 960 pixels. The preview is then
  drawn by the same code as on the desktop. The microphone of that screen is
  captured exactly as for a PC, encoder included, so that what works there
  works in a stream.
- **Shown as in a mirror,** which is how people expect to see themselves.
  What a PC gets is not mirrored.

There is no choice of camera yet: the first one that fits is used. On Linux a
camera that stops answering without an error holds the thread, and with it
every later camera, until the app is restarted.

### On its way to a PC

- **Asked beside the stream.** GameStream has no word for a camera, so a PC
  that takes one says so on an API of Leto's own, thirteen ports above the
  GameStream one, and agrees there on where the pictures go and on the key
  that seals them. `leto-return` holds what both sides say (`api`) and the
  packets (`camera`): it does no networking, so that the host of step 4 and
  the stand-in host of today read and write the same bytes as the client.
- **Only the PC this device knows is asked.** `leto-client` talks to that API
  with the certificates of the pairing, both ways, as it does for everything
  else that concerns a paired PC (`transport.rs`, `camera.rs`). A PC that
  forgot this device answers it nothing, and something else that answers at
  the address of the PC is not even told what this device would like to send.
- **The question never delays a stream.** The engine asks once the stream
  plays, and tells the answer as news of its own (`Event::Camera`) up to
  three seconds later. Until then the app shows no camera switch at all; a
  PC that takes none gets a dimmed one, with the reason. Only a shell that
  can send a camera has the engine ask (`Shell::sends_camera`): the app
  does, and the bridge for its browsers.
- **Never in clear.** Every packet is sealed with AES-128-GCM under a key the
  PC made for this camera. There is no way to send one unsealed.
- **Encoded in software,** as pictures are decoded: `PictureEncoder` in
  `leto-media` wraps OpenH264. It is given a bit rate to keep (2.5 Mbit/s at
  1280 by 720 and thirty pictures a second, less for smaller pictures),
  leaves pictures out rather than exceed it, and makes one that decodes on
  its own every two seconds. A camera larger than what the PC takes is
  halved until it fits. That rule, with the bit rate, is the engine's
  (`CameraOffer::fit` in `leto_core::engine`): a browser encodes its camera
  itself and follows it too. At 1280 by 720 it takes about 3 ms a picture in
  a release build on the four cores of the machine this was written on.
- **One feed from the camera to whoever carries it.** `CameraFeed` in
  `leto-media` opens the camera, encodes on a thread of its own and hands
  each picture to a `Carrier`, which is the only thing it knows about where
  they go. The app's carrier gives them to the engine
  (`Service::camera_open`, `camera`, `camera_close`); `leto-cli` has one
  that sends them itself. The feed keeps to thirty pictures a second
  whatever the camera delivers.
- **It belongs to the stream.** The engine lets go of the camera when the
  stream is left, fails or ends, and when another one starts, without waiting
  for the window. It asks no PC for one unless a stream plays: not for a
  stream that ended and was not left yet, and not the PC of the stream
  before while the next one starts. The PC is told that it stopped, and by
  which key, so a
  goodbye that arrives late never ends the camera that followed. The cameras
  of an engine are numbered for the same reason: a feed that was let go of
  can neither send through, nor stop, the one that came after it.
- **Off until the person turns it on,** in every stream, with one switch in
  the bar over the picture and Ctrl+Alt+Shift+C. While pictures leave, the
  switch is lit and a small camera stays in a corner of the picture when the
  bar is away. A PC that refuses is explained in the bar: its owner does not
  allow it, another device is sending, or it cannot take these pictures.

The stand-in host takes a camera the way Leto's host will: it answers the
API, opens and decodes what arrives, shows it in a corner of the picture it
streams, counts what it received and lost, and saves the last picture when
asked (`camera` in its terminal). `leto-cli play --camera` sends the camera
without the window, and says whether a PC takes one at all. Both the app and
the command line send a test card instead of a camera when `LETO_CAMERA=card`
is set, which is how a machine without a camera checks the way there.

### From a browser

A page can neither send UDP packets nor show a PC the certificates of the
pairing, so its camera goes through its bridge, which is the device the PC
knows.

- **The browser encodes**, with the H.264 encoder it offers pages
  (WebCodecs): the constrained baseline profile, with start codes, in the
  mode meant for calls, where no picture waits for the next. Size, pictures
  a second and bit rate come from the engine's rule and what the PC said it
  takes, and the browser scales what the camera gives (`backend/web/film.rs`).
  It is asked whether it can before the PC is.
- **A timer takes the pictures**, thirty times a second, from the `<video>`
  the camera plays in. A picture that would only wait is not taken: when the
  encoder has more than two before it, or when more than 256 KiB wait to
  leave for the bridge. Every two seconds a picture decodes on its own, and
  so does the one after a picture that was lost once encoded (`Shutter` in
  `backend/flow.rs`).
- **One more kind of message.** The page says `CameraOn` with what it would
  send and a number, the bridge answers `Camera` with that number, and
  pictures follow as binary messages: when each was taken, whether it
  decodes on its own, and the H.264 of it. One picture is 1 MiB at most,
  which is why a browser's WebSocket takes messages that long. Text stays
  held to 64 KiB, and a browser the gate has not let in is hung up on when
  it sends anything else.
- **The bridge asks the PC, and passes on.** It does what the app's carrier
  does (`Service::camera_open`, `camera`, `camera_close`), for the stream
  that browser is watching, and never looks into a picture
  (`leto-bridge/src/camera.rs`). Asking a PC waits for its answer, so it
  happens on a thread that may wait, one question at a time: what the page
  says meanwhile replaces what it said before, and only its last asking is
  answered. `CameraOff`, leaving the stream or closing the tab stops the
  camera, and the PC is told.
- **Never in clear** here either: the pictures travel inside the TLS of the
  WebSocket to the bridge, and sealed from there to the PC.
- **The same switch.** The page hears what camera the PC takes as the app
  does, and the bar, the shortcut, the lit switch and the refusals are the
  desktop app's code.

A tab nobody looks at sends a picture a second at most, because browsers
slow its timers. The bridge cannot ask a browser for a picture that decodes
on its own: a PC that lost one waits for the next, two seconds at most.

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
  microphone and the camera the other way, travel as binary messages and
  stay encoded all the way: the bridge never decodes.
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
piece of sound plays, which pictures a decoder may be given and which
pictures of the camera are taken (`backend/flow.rs`), the gate, the relay,
the HTTP the bridge speaks. The
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

Every HTTPS connection to a PC is a full TLS handshake, in which the client
shows its certificate again: the transport never resumes a session. Apollo
asks for the client's certificate without giving OpenSSL a session id
context, and OpenSSL answers a resumed session with an `internal_error`
alert, so every request after the first would fail (Sunshine, whose code
that is, most likely behaves the same). The stand-in host hangs up on a
resumed session for the same reason, so the tests catch a client that tries.

The bridge adds nothing to this and takes nothing away: it is the paired
device, and the check is the engine's. What it adds is the gate described
above, between itself and browsers.

## Logs

The app, the command line and the bridge each write a log in the `logs`
folder beside Leto's settings: `leto.txt`, `leto-cli.txt` and
`leto-bridge.txt` (`leto_client::logs`). `leto-cli about` prints where. Each
run adds to its file; a file that reached 10 MB is set aside as
`*.previous.txt` the next time its program starts. A bug that stops a program
is written there too, which matters on Windows, where the app has no
terminal. Leto's own crates log in detail, other libraries only what they
find worth telling; `RUST_LOG` changes that and also prints to the terminal.

A log names PCs and their addresses, and each request's path, but never a
secret: the folder can be sent as it is. The settings folder around it holds
this device's key and is never to be sent.

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
possible. `leto-system` is one: its `unsafe` is in the modules for Windows,
each call with the reason it is sound.
