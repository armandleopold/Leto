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
  leto-media/     Pictures and sound: video decoders (H.264 everywhere, H.265 and AV1 by Windows), Opus, the speakers, the microphone, the camera and its encoder. No networking
  leto-system/    What Leto asks of the system itself: every key for a stream, what the screen is like, and a clock that counts finely during a stream. Windows only for now, `unsafe` allowed
  leto-tunnel/    A WireGuard tunnel inside Leto, for its own connections to a PC (boringtun and smoltcp). No administrator rights
  leto-return/    The part of the return channel that is Leto's own: the packets of the camera and the API it is asked on. No networking
  leto-bridge/    Lets browsers use the client engine: serves the web client and relays streams to it
  leto-testhost/  A stand-in for a Sunshine or Apollo host, for tests and for trying a client
xtask/            Project automation, run with `cargo xtask`
assets/brand/     Logo, icons and colours
docs/             This documentation
vendor/
  moonlight-common/  moonlight-common-rust at one commit, with the change Leto needs (AV1). Not a workspace member
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
(`cargo xtask web` builds it into `dist/web`).

The look is the "Braise" palette from `leto_core::brand`, by night and by day
(`theme.rs`): a warm black ground with ivory text, or the same turned over,
ivory under warm black, with the orange accent a little deeper so that it
shows on ivory. Leto wears the one the system asks for and changes with it
while it runs: Windows tells the app, and a browser tells the page
(`prefers-color-scheme`). Linux tells neither, so the app is dark there
unless `LETO_THEME=light` is in its environment. A stream is always shown by
night, whatever the system asks for: black surrounds its pictures, and what
is drawn over them has to be read on them. A test checks that what is
written can be read on both grounds.

It is made of three parts that do not reach into each other:

- `state.rs` decides what is on screen and what happens next. It draws
  nothing and opens no connection, so it is tested like any other logic.
- `backend/` does the work, and reports back with events. On the desktop it
  runs `leto-client` on worker threads. A browser page cannot open sockets, so
  the web build asks the bridge that served the page, which runs the same
  engine: see [The web client and its bridge](#the-web-client-and-its-bridge).
- `ui/` draws the screens from the state and tells the state what the person
  did.

On Windows the window has no title bar of the system's: Leto's own top bar
takes its place (`ui/frame.rs`). Dragged, it moves the window, with the
system's snapping to the sides; double-clicked, it maximizes the window or
gives the screen back. Its top right corner holds the buttons that minimize,
maximize and close it, as tall as the bar, and the window's edges resize it.
egui gives such a window the system's shadow. A stream in a window, rather
than full screen, has the same three buttons at the end of its bar, which
moves the window too; its edges are the PC's, for the mouse. `leto.exe`
carries the app's icon and name as Windows resources, made by
`apps/leto/build.rs` from `assets/brand/windows/leto.ico`: Explorer, its
search and the Task Manager show them before the app runs. Elsewhere the
system draws the frame.

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
   decodes each picture and keeps only the latest one for the window. A
   picture that newer ones already wait behind is decoded, since they build
   on it, but not copied out of the decoder: the engine tells the sink how
   many wait (a `Leto:` change in the vendored `moonlight-common`, as
   moonlight-common-c's `LiGetPendingVideoFrames`). A decoder slower than
   the stream, as at 144 pictures a second in 3840 × 1080, then skips
   pictures instead of falling further and further behind, which the mouse
   pointer shows first; one still goes through every 100 ms. The audio sink
   decodes each packet and queues it for the speakers.
3. The window picks the latest picture up when it redraws and shows it as
   large as fits, undistorted.

Leaving a stream keeps the app running on the PC, like closing Moonlight. The
apps screen then offers to resume it or to close it.

A stream that loses its PC while the person watches it is not left. The engine
says so (`Event::Reconnecting`) and asks the PC for the stream again, after 1,
2 and 3 seconds and then every 5, until it plays, the person leaves it, or the
PC answers in a way that asking again will not change (`Event::PlayFailed`).
A PC that does not answer, or answers as one still busy with the stream it
lost, is asked again (`service.rs`, `passing`). The window keeps the last
picture and puts "Connection lost. Reconnecting…" over it, from the moment
the engine says so or once three seconds pass without a new picture,
whichever comes first: a PC that goes silent can take the connection ten
seconds or more to notice. The camera sent to the PC stops with the stream
and starts again when it is back. The stand-in host's `drop` breaks a stream
and `busy` makes it refuse new ones, which is how the tests lose a PC. A
browser that loses its bridge still ends the stream: only the bridge asks a
PC again.

A stream is made for the screen it is shown on. When the person picks an
app, the window says what its screen is like (`window::screen`): its pixels
and its pace as Windows reports them (`leto_system::display`), elsewhere as
egui does at 60 pictures per second. The request carries it (`Request::Play::screen`), and the engine asks for a
stream of that size and pace (`StreamSettings::for_screen`), within what
H.264 carries, with a bit rate that grows with the pixels from Moonlight's
20 Mbit/s for Full HD at 60. It also asks the PC for a screen of its own in
that mode (`virtualDisplay`), which Apollo makes for the stream. It never
sends Apollo's `scaleFactor`: that is not the size of text but a factor on
the screen's size, so 125 for a laptop that draws text at 125 % made a
2400 × 1500 screen and stream for a 1920 × 1200 one. How large text is on
the PC's new screen is the PC's setting, which Windows keeps for that screen.
A screen the owner set for the device on Apollo's side wins. Sunshine ignores the question and shows its own screen. A browser says
its screen's size but not its pace: it gets 60, lying down.

Those are the defaults, and the settings change them, after Moonlight's
(`settings.rs`, drawn by `ui/settings.rs` behind the cog on the list of PCs).
The person may choose the stream's size and pace instead of the screen's, its
bit rate (from 0.5 to 150 Mbit/s, as in Moonlight), no screen of its own on
the PC, the PC's speakers playing too, no optimized game settings, the app
closed on the PC when the stream is left, no full screen when it connects,
V-Sync off, when the system's shortcuts go to the PC (always, in full screen
only, never), the PC's pointer drawn in the picture instead of this device's,
the mouse buttons swapped, the wheel reversed, A and B and X
and Y swapped on gamepads, Leto silent in the background, the statistics
from the start, decoding on the processor only, and a codec of their own. What the PC must know travels with the request
(`Request::Play::choices`, `StreamSettings::chosen`); the rest stays in the
app. The settings are kept as JSON: `settings.json` beside the paired PCs on
the desktop, the page's local storage in a browser. A choice that is missing,
as after an update, is its default, and a file that cannot be read is all
defaults. A change is saved at once and counts from the next stream; V-Sync
counts from Leto's next start, since the window is made with it. A browser
offers no full screen on connect, no V-Sync and no capture of the system's
shortcuts, which it does not let a page have.

The window fills the screen as the stream connects, and gives it back when
the stream is left. Ctrl+Alt+Shift+S shows statistics in a corner, as in
Moonlight: what the PC sends, what arrives, what is lost, decoded, skipped to
keep up and shown per second, how long a picture takes to decode, how much of
that goes to copying it out of the decoder (from the graphics card, waiting
for the card included), how long the PC takes to make it, and the time to the
PC and back (`stats.rs`, counted by the backend).

The work follows the person's attention. While another window has the
keyboard the video sink still decodes every picture, since the next ones
build on it, but hands only one a second on to be drawn, which is most of
the work; the window then draws once a second. While the window is out of sight
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

- Video is AV1, H.265 or H.264. The PC is offered the codecs this device
  decodes on its graphics card, and H.264 always, which every PC makes; the
  PC takes the best of them it encodes, AV1 first (`Settings::codecs`,
  `Choices::codecs`, `StreamSettings::codecs`). Choosing AV1 needed a change
  to moonlight-common-rust, which never chose it: Leto builds a copy of it
  with that change (`vendor/moonlight-common`, see its README).
- On Windows, the pictures are decoded by Windows' own Media Foundation
  decoders, given a Direct3D 11 device so that the graphics card does the work
  (`leto-media/src/video/windows.rs`). Windows has the H.264 one, and the
  Store's video extensions bring H.265 and AV1. What this device decodes is
  found once, in the background as Leto starts: each decoder is started at
  1280 × 720, and the card has to name the codec among its decoder profiles to
  count as decoding it. The decoded picture comes back from the card in NV12,
  as planes of brightness and colour.
- Decoded pictures stay in planes (`leto_media::Planes`), from Windows'
  decoders and OpenH264 alike, and go back to the graphics card as they are
  when the window draws: `leto_media::Card` puts them in two textures and a
  small program on the card turns them into pixels, BT.601 in limited range
  as `convert.rs` has it (`ui/card.rs`, through egui's paint callback). A
  colour serves two pixels across and two down, and the PC's encoder puts it
  where the left one of the two is; a texture has it between the two. The
  program reads the colours half a pixel to the right for that: read at the
  same place as the brightness, every edge of colour is drawn half a pixel
  off and smeared. On a pattern of hard edges of colour, with the colours
  placed as Sunshine places them, that takes a third off the largest error
  and a sixth off the average one. The processor's `convert.rs` gives each
  colour to its four pixels unmixed, and is not changed. That
  is less than half the bytes of the pixels, and the processor no longer
  spends 6 ms a picture at 1920 × 1200 making pixels. It needs OpenGL 3.1 or
  OpenGL ES 3: where the card offers less, or turns the program down, the
  processor makes the pixels as before. With V-Sync on, each drawing first
  waits until the card is done with the one before (`glFinish`), so that a
  picture never waits behind others to reach the screen: the driver would
  otherwise queue two or three, each a refresh later. The picture still goes
  from the card to memory and back: drawing it from where the decoder left
  it would need Direct3D and OpenGL to share it.
- H.264 is decoded by OpenH264 on the processor where the graphics card does
  not decode it, on every other system, and from the moment Windows fails
  three pictures in a row. It works on every desktop with nothing to install.
  A browser decodes what it gets with WebCodecs, and the bridge asks PCs for
  H.264 alone for it: it reads what H.264 pictures are made of to tell the
  browser, and does not know how for the others yet.
- The video decoder (automatic, or software only) and the codec (automatic,
  or one of the three) are settings, and the statistics say which codec
  arrived and who decodes it. A codec this device turns out not to decode at
  the size the PC sends leaves the stream without pictures, and the screen
  says why instead of waiting.
- Leto asks the PC for BT.601 colours in limited range, in standard dynamic
  range. HDR comes with the decoders that can show it.
- A stream waits a few milliseconds at a time in many places: the engine
  for packets, the decoder for the graphics card, the sound for its queue.
  Windows wakes such a wait on the ticks of its clock, 15.6 ms apart unless
  a program asks for finer ones, which is two pictures of a fast stream. So
  each `Session` asks for ticks a millisecond apart for as long as it lives
  (`leto_system::clock`), as Moonlight does through SDL. Other systems count
  finely by themselves.
- Sound is stereo Opus, decoded in Rust and played on the system's default
  output. Between the network and the speakers sits a queue of a few tens of
  milliseconds: it waits for a small cushion before playing, fills up again
  after running dry, and skips ahead when sound piles up, so that sound never
  drifts behind the picture. A stream whose first second of sound did not
  decode shows "No sound" in its bar and says why in the log, as speakers
  that did not open do. A stream that has had no sound at all does not:
  Sunshine and Apollo send nothing while the PC plays nothing, as a real
  Apollo showed, and nothing tells that apart from sound lost on the way.
  The log notes it after five seconds, for whoever reads it.

The stand-in host streams for real: it speaks RTSP, encrypts, and sends a
moving test card as H.264 and a melody as Opus, so the whole path is tested
on every change without a gaming PC. The card is made to be judged at a
glance: a sun passing behind a moon shows stutter, a marker crossing the top
edge once a second shows dropped pictures, and colour bars show wrong colours.
It can also say it encodes H.265 and AV1, the way Sunshine and Apollo say it,
and reports which codec the client asked for, though its pictures stay H.264:
that is how the choice of codec is tested.

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
- **The pointer on the picture is this device's own**, which moves the
  moment the mouse does, rather than the PC's, which shows a round trip and
  a picture later. Sunshine, Apollo and Foundation Sunshine leave their
  pointer out of the pictures when they hear Ctrl+Alt+Shift+N, and put it
  back the next time: Leto sends that as the stream connects
  (`Controls::switch_pc_pointer`), and again for the game mouse, which has
  no pointer here, and before leaving, closing the window included (it
  waits `CLOSES_AFTER_LEAVING` for that). Only the PC's pictures change, not
  its own screen. The switch is the host's, global and never reset by
  itself, so Leto remembers for each PC whether it hides its pointer, until
  Leto quits: a stream that ends without being left, or a host that
  restarts, can leave the two out of step, with two pointers or none, and
  Ctrl+Alt+Shift+N, which Leto keeps no shortcut on, puts them back in step.
  A PC that refuses the keyboard or the mouse, or GameStream, draws its own
  pointer, and so does a person who turns "This device's pointer" off. The
  pointer here is an arrow whatever the PC shows: the protocol does not say
  the PC's shapes. Leaving lets the last keys go out first (`LAST_WORDS` in
  `Session::stop`): stopping the stream drops what still waits to be sent,
  which the stand-in host showed for almost half of the streams left.
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
  ahead of Windows with a low-level hook, and keeps every key from going
  further while Leto's window is in front. Windows waits for the hook's
  answer only so long, then hands the key on anyway and removes the hook
  without a word, so the hook has a thread of its own at a high priority,
  only decides and waits for no other thread: another thread tells the
  window. Windows asks the hook set last first, so Leto sets it each time
  its window comes in front, again when a key gets past it to the window or
  reaches it late, and every second besides. The new one is set first, and
  the old one goes half a second later, since Windows may be handing it a
  key at that moment. On some computers Windows never calls the hook at all, security
  software perhaps seeing to it: the keys then reach Leto's window, which
  takes each from its messages for the stream before acting on it, so that
  Alt+F4 does not close Leto, and Windows is asked to leave aside the
  shortcuts programs registered (`RIDEV_NOHOTKEYS`), which it does for a
  program that reads the keyboard itself: the Start menu is Explorer's, as
  are Windows+E, Windows+D and the rest. Alt+Tab and Alt+Esc never reach a
  window: Windows sees them as the Tab or Esc goes down with Alt held. So
  each time an Alt reaches the window for the stream, Leto tells Windows
  that it came up again, with a key of its own making (`SendInput`, marked
  as Leto's): the stream still holds Alt, Windows sees a plain Tab, and the
  window gets it for the PC. That Alt coming up reaches the window too,
  where its mark has it dropped, and the low-level hook lets it go its way.
  Held down, Alt repeats, and Windows hears it go down again, so each repeat
  is let go of too. For a Tab that comes before that, Leto also registers
  Alt+Tab and Alt+Esc as its own shortcuts while the stream has the keyboard
  (`RegisterHotKey`), which a program in front may do, and Windows then
  tells the window's thread of them instead of switching windows. The log
  says when Windows did not let Leto have them, another program having them
  already, or did not take the Alt Leto let go of. The log counts the keys, how
  long they waited, how many came late and how many reached the window,
  never which. Keys are named by their scan code, the numeric keypad
  included. A key goes back to whoever got it go down, so nothing stays held
  on either side, and the Ctrl that AltGr makes up is dropped: the PC makes
  up its own. Ctrl+Alt+Del and Windows+L stay with Windows, which no program
  can take. Leto's own
  shortcuts are found by the letter the layout prints on the key, which
  Windows tells even when Ctrl and Alt held together, AltGr on a French
  layout, change what the key types. The person can keep the system's
  shortcuts for Windows in the settings, always or while Leto does not fill
  the screen: the hook is then not set, and the other keys still go to the
  PC through the window.
- eframe would turn Ctrl+C, Ctrl+X and Ctrl+V into clipboard events and keep
  those keys from the PC, so that is turned off, and `clipboard.rs` does it
  for Leto's own text fields when no stream has the keyboard.
- **Gamepads** are followed by one thread of the backend (`backend/gamepad.rs`,
  on [gilrs](https://gitlab.com/gilrs-project/gilrs)). Each stream hears of
  every gamepad once, then of each change, sent reliably as Moonlight sends
  it: a change that was lost would leave a button held on the PC until the
  next one. Buttons are named by place: A is
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

- **A screen of its own.** "Camera, microphone and sound", reached from the
  list of PCs, shows what the camera sees and how loud the microphone is,
  with no PC involved. Both are off at every visit, each has its switch, and
  leaving the screen lets go of both. While one captures, its icon is lit and
  a line under it says so.
- **Which camera, microphone and sound output.** On the desktop the same
  screen has a list for each, for a device that has several: a laptop's own
  camera and one on USB, a headset beside the loudspeakers. A device is
  chosen by the name the system gives it, kept with the settings
  (`DeviceChoices`), and "Restore the defaults" leaves it alone. The first
  choice of each list leaves it to Leto (the first camera with colour) or to
  the system (its default microphone and output), and so does a device that
  is not there when it is wanted. The lists are made when the screen opens
  and again when one is opened, the cameras on their own thread
  (`leto_media::look_for_cameras`). A camera or microphone at work changes at
  once, the sound output at the next stream, and a short test sound tells
  where sound goes. A browser asks the person itself and has no lists.
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

## The ways to a PC

A PC can often be reached in more than one way: on the local network,
through a VPN, by a name. `leto-client` keeps up to four addresses for each
PC: the ones it was added by or found at on the network, and the one the PC
gives itself. Sunshine and Apollo tell which of their addresses a question
came in on, which, asked from beyond a forwarded port, is their address on
their own network.

A PC known only through a VPN tells its address there, and its address
nearby has to come from somewhere else. One way is the announcement the PC
makes on the local network, which a firewall can keep from Leto. The other
is its name: a PC says the name of the computer it runs on, and this
device's own system often knows where that name leads, from the router or
by asking the network as Windows and macOS do for the names of computers.
So while Leto listens for announcements it also asks the system, once a
minute at most, for the name of each PC it keeps and for that name with
`.local` (`discovery::by_name`). Only an address of a local network is
asked at, since a name can mean something else on the internet, and only a
name that can be a computer's. An address where the PC answers, and proves
who it is when it is paired, joins the others.

Neither worked on the first laptop it was tried on: nothing was announced
to Leto there and the name led nowhere. So a third way needs nothing from
the network but what a stream needs. While a paired PC answers, but not at
an address of a local network this device is on, Leto looks around on that
network (`Client::look_around_on`): it tries a connection on the PC's port
at each address around its own, the 253 that share its first three numbers
at most, for a third of a second, all at once (`discovery::listening`).
What takes one is asked who it is, and only a PC that proves to be one Leto
paired with is kept, at that address too. Each network is looked around on
once while Leto runs. It is some 250 connections tried on whatever local
network the device is on, at home or not, which a network that is not the
person's own may notice: that is its cost.

Each time it looks for a PC, it asks at every address at once and uses the
first that answers: the nearest way there, the one with the shortest round
trip. Moonlight prefers a PC's address on the local network for the same
reason. The other addresses stay for when that one stops answering, for
example away from home. A paired PC must prove who it is at each address; an
answer that does not prove it is only taken when no address does better, and
then shows the PC as not paired, as before. An address where another PC now
answers adds that PC to the list. The addresses are kept in `client.json`,
the one that answered last as `address` and the others as `other_addresses`.

## Through a WireGuard tunnel

Away from home, a PC is safest reached through a VPN rather than through
ports opened on the router. With WireGuard that used to mean the WireGuard
app, connected by hand before Leto. Leto can now open the tunnel itself:
the person gives it a WireGuard file, as their server or the WireGuard app
writes one, in the settings (pasted, or dropped on the window) or with
`leto-cli tunnel use <file>`. `leto-cli tunnel` shows it, and `tunnel
remove` takes it away.

The system's WireGuard makes a network card of its own, which takes
administrator rights, and carries everything. Leto's tunnel
(`crates/leto-tunnel`) is a UDP socket like any other and carries only
Leto's own connections to a PC. WireGuard itself is boringtun, which only
turns packets into packets; TCP inside the tunnel (pairing, the list of
apps, starting a stream) is smoltcp; UDP, the pictures, the sound and the
input of a stream, is written by hand to stay out of a stack's way.

The rest of Leto, and moonlight-common-rust, open ordinary sockets. So each
address of a PC that the tunnel leads to (its `AllowedIPs`) gets a *door*: a
local address, `127.77.0.1` for the first, that leads through the tunnel to
that address, port for port. A door listens on the ports a host uses, all
derived from its base port as Sunshine and Apollo do: what connects to
`127.77.0.1:47989` reaches `10.8.0.3:47989`. The door is one more way in
[the race for the nearest](#the-ways-to-a-pc), and starts 150 ms after the
direct ways: at home the local network still wins, and with the system's
WireGuard connected the PC is still reached through it. A PC reached
through Leto's tunnel says "WireGuard" in the list, and its stream asks for
video packets of 1024 bytes, as Moonlight does for a PC far away, so that
each fits in a WireGuard packet.

The tunnel opens the first time a PC it leads to is looked for, and tries
again ten seconds after a failure. The settings show whether the other end
answers, this device's address in the tunnel and its public key, which the
server must know. The peer's name is looked up again when it stops
answering, since an address at home changes.

The file holds a private key. It is kept as `wireguard.conf` beside
`client.json`, which holds the pairing key, and as private: readable by its
owner alone on systems with Unix permissions, in the person's profile on
Windows. Its text never reaches a log, and a browser
cannot give or remove one through the bridge: the tunnel belongs to the
device the engine runs on.

What it does not do, and what was tried:

- It met one real WireGuard server, on 2026-11-21: the Windows app reached
  a PC through the WireGuard of a Raspberry Pi at home, of which the PC is
  a peer too, and the person who tried it reports that it works well. Which
  of pictures, sound and input were looked at is not known, and no other
  server was tried, a router's among them. In the tests two of Leto's own
  tunnel ends talk to each other, and a whole session goes through them to
  the stand-in host: finding it, pairing, the apps, pictures, sound and
  input.
- Leto should have a peer of its own on the server. The same keys in use at
  the same time by the WireGuard app, on this device or another, take the
  tunnel from each other every few seconds.
- One file, with one peer, and IPv4 inside the tunnel. `MTU`,
  `PersistentKeepalive` and a pre-shared key are followed; `DNS`, scripts
  and routing lines mean nothing to a tunnel that carries Leto alone, and
  are ignored.
- Doors need local addresses other than `127.0.0.1`. Windows, Linux and
  Android have all of `127.0.0.0/8`; macOS and iOS only have `127.0.0.1`
  unless more are added by hand, so there the tunnel cannot open a door
  yet.
- The packet that wakes a PC goes through the tunnel too, to the PC's own
  address, since a broadcast does not cross WireGuard. That wakes it only
  while the device the tunnel ends on still knows which card has that
  address, which routers forget after a while, and never when the tunnel
  ends on the sleeping PC itself. A PC that is a peer of the server, and
  known to Leto by its address inside WireGuard, is in that case: that
  address is the PC's own end of a tunnel, asleep with it. See below.
- Not in the browser: the web client reaches PCs through the bridge, which
  uses the tunnel of the device it runs on.

## Waking a PC

A PC that sleeps or is off can be woken by its network card, which still
listens for a "magic packet": six bytes of `0xFF`, then the card's address
sixteen times (Wake-on-LAN, `leto_client::wake`). Sunshine and Apollo tell
that address only to a device they paired with, over HTTPS, and zeros to
anyone else, so `leto-client` learns it when pairing and again each time it
reaches the paired PC, and keeps it in `client.json` as `card`. A PC added
but never paired cannot be woken.

Choosing an offline PC that Leto can wake, in the app, or `leto-cli wake`,
sends the packet and then asks the PC every two seconds until it answers,
for two minutes, sending it again every ten seconds in case one was lost.
The PC then opens as if it had been on. A sleeping PC answers nothing, so
the packet goes everywhere it could land, as Moonlight does: to each of the
PC's addresses, to every local network this device is on as a broadcast,
and to the IPv6 address every device on a link listens to, each on ports 9
and 47009 and on the ports of the streaming software, moved with the PC's
own port.

What it needs on the PC: Wake-on-LAN turned on in its firmware and in the
settings of its network card, and a wired connection, as few Wi-Fi cards
wake. The packet reaches it on the same network. From elsewhere it needs a
router that forwards one of those ports to the PC's network as a broadcast,
or a device at home that is awake and passes it on: a tunnel that ends on
the sleeping PC itself is asleep with it.

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

During a stream the app writes a line every ten seconds with what its
statistics show: pictures arrived, decoded and skipped in a second, the bit
rate, the loss, how long a picture took to decode and how much of that to
leave the decoder, how long the PC took to make one, the way to the PC and
back, and on how many pixels of the screen the pictures are drawn: one for
each of theirs, or more or fewer, which makes them less sharp. A log of a
stream that felt slow or looked soft says where to look.

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
each call with the reason it is sound. `leto-media` opts in for one module
only, its Windows video decoder (`video/windows.rs`), which calls Media
Foundation and Direct3D 11.
