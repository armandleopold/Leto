# Roadmap

Four steps. Each one ends with something that can be used and tested on
Windows, Android or the web, the three platforms available for manual tests.

## 1. Foundations (this step)

- The repository: Cargo workspace, lint rules, `cargo xtask`, the pipeline.
- A first app that starts on the desktop and in the browser, with the brand.
- The [inventory](inventory.md) of the projects Leto builds on.
- The design of the [return channel](return-channel.md) for microphone and
  camera.

## 2. One client for Windows and the web

- Find hosts on the network, pair with a PIN, list apps, start a stream from a
  Sunshine or Apollo host, built on moonlight-common-rust.
- Video and audio playback, keyboard, mouse and gamepad.
- The same interface in the browser, through a bridge next to the host.
- Simple mode first. Advanced mode for the settings Moonlight users expect.
- Capture of the microphone and the camera, sent over the return channel.

Progress:

- Done: find PCs on the network or by address, pair with a PIN, list the apps
  and their covers, in the desktop app and on the command line.
- Done: start an app, see its picture and hear its sound, leave, resume and
  close it. H.264 in software and stereo sound for now.
- Done: play with the keyboard, the mouse and gamepads. Ctrl+Alt+Shift+Q
  leaves a stream, as in Moonlight, because every other key is the PC's.
- Done: a stream feels like Moonlight's. It fills the screen as it
  connects; on Windows every key goes to the PC, the Windows key, Alt+Tab
  and Alt+F4 too; Ctrl+Alt+Shift+Z gives the keyboard and the mouse back and
  Ctrl+Alt+Shift+S shows the statistics. The PC is asked for a screen of its
  own just like this device's, in size and pace, which Apollo makes. A window another one hides or covers shows one picture a second, or
  none. See [architecture.md](architecture.md#a-stream).
- Done: the same in the browser. `leto-cli bridge` serves the web client and
  relays the stream, which the browser decodes itself. See
  [architecture.md](architecture.md#the-web-client-and-its-bridge).
- Done: the microphone, from the desktop app and from the browser, to a PC
  that takes one (the microphone build of Apollo, Foundation Sunshine). One
  switch in the bar over the picture, off at the start of every stream. See
  [architecture.md](architecture.md#the-microphone).
- Done: the camera of this device captured and shown to the person, in the
  desktop app (Windows and Linux) and in the browser, on a screen that also
  shows whether the microphone hears. Nothing of the camera is sent yet. See
  [architecture.md](architecture.md#the-camera).
- Done: the camera on its way to a PC that takes one, from the desktop app
  and the command line: asked on Leto's API, encoded as H.264, sealed. One
  switch in the bar over the picture, off at the start of every stream. No
  Sunshine or Apollo takes a camera, so for now it reaches the stand-in host
  only. See [architecture.md](architecture.md#on-its-way-to-a-pc) and
  [return-channel.md](return-channel.md#camera-new-leto-extension).
- Done: the same from the browser. The page encodes its camera with the
  browser's own encoder and hands the pictures to the bridge, which asks the
  PC and passes them on. See
  [architecture.md](architecture.md#from-a-browser).
- Done: choosing the camera, the microphone and the sound output on the
  desktop, where a device has several, on the screen that tries them out.
  The lists were only seen with made-up devices on Linux: what Windows
  names its devices, and whether the choice holds there, was not tried. See
  [architecture.md](architecture.md#the-camera).
- Done: the settings, behind the cog on the list of PCs, after Moonlight's:
  size, pictures per second and bit rate of the stream, full screen, V-Sync,
  the PC's screen of its own, its speakers and game settings, closing the app
  when leaving, the system's shortcuts, the mouse buttons and wheel, gamepad
  buttons, silence in the background and statistics. Leto keeps them. See
  [architecture.md](architecture.md#a-stream).
- Done: decoding on the graphics card on Windows, and AV1 and H.265 with it.
  The PC is offered what the card decodes, AV1 first, then H.265, and H.264
  always; the settings choose the codec and the decoder. See
  [architecture.md](architecture.md#a-stream).
- Later in this step: HDR, surround sound, decoding on the graphics card on
  macOS and Linux, and H.265 and AV1 in the browser.

What the web client does not do yet:

- Gamepads whose layout the browser does not know. It names the buttons of
  the usual ones ("standard" in its words), and Leto leaves the others out
  instead of guessing which button is which.
- Starting the bridge from the app. It is a command for now, and the code it
  asks a new browser for is printed in its terminal.
- Reaching the bridge from outside the home network. It is built for the
  network it runs on.
- H.265 and AV1: the bridge asks PCs for H.264 alone, which it reads to tell
  the browser what the pictures are made of.
- It was checked in Chromium only. Firefox and Safari have the same decoders
  in their recent versions, and nothing here was tried in them.
- The web client is one file of 5 MB that the bridge sends as it is, without
  compressing it.

What the microphone does not do yet:

- It was never sent to a real host. The stand-in host follows the Apollo
  microphone build and Foundation Sunshine as read in their source, and what
  it hears was checked there. The microphone itself was a made-up one, on
  Linux and in Chromium: no real microphone was captured, on any system.
- In the desktop app it is the microphone chosen, or the system's default
  one, as the system gives it: no noise suppression, no echo cancellation.
  With loudspeakers instead of a headset, the PC hears its own sound again.
  A browser is asked for its own echo cancellation and noise suppression,
  and chooses the microphone. Whether that keeps the PC's sound out of a
  real room was not tried.
- In a browser it needs the browser's own sound encoder, which Firefox and
  Safari got later than Chrome and Edge. A browser without it says so when
  the switch is turned on. It was checked in Chromium only.
- Sent to a stock Apollo or Sunshine it would go nowhere, so the switch is
  dimmed there. The receiver that changes this is the companion of step 4.
- Nothing remembers that it was on: it is off at the start of every stream.

What the camera does not do yet:

- It reaches no real PC. The receiver is the companion of step 4, and until
  then the switch is dimmed on every Sunshine and Apollo. What was checked is
  the way to the stand-in host, which decodes the pictures and shows them.
- No real camera was captured, on any system: the container this was written
  in has none, and sends a test card. The Windows capture compiles and passes
  its tests in the pipeline, and was never run by hand.
- How a real Apollo or Sunshine reacts to the question on port + 13 was not
  tried. A PC whose firewall drops it without a word delays the answer by
  three seconds, after the stream plays.
- From a browser it was checked in Chromium only, with the camera Chromium
  makes up for tests. It needs the browser's own H.264 encoder, which
  Firefox and Safari got later than Chrome and Edge, and which may be
  missing where the system has none. A browser without it says so when the
  switch is turned on.
- A tab nobody looks at sends a picture a second at most: browsers slow its
  timers.
- The desktop app encodes in software. A hardware encoder comes with the
  hardware decoder. A browser uses what it has.
- No choice of camera, of size or of bit rate, and nothing remembers that it
  was on: it is off at the start of every stream.
- Nothing is repaired on a poor network: a picture that lost a packet is
  dropped, with the ones after it until the next that decodes on its own, two
  seconds later at most.

What input does not do yet:

- On Linux, on macOS and in a browser, the numeric keypad is sent as the
  row of digits above the letters, because egui reports both as the same
  keys. On a PC with a French layout its digits therefore come out as the
  signs of that row. On Windows Leto reads the keyboard below egui, and the
  keypad is the keypad.
- Outside Windows, keys the system keeps for itself stay on this device: the
  Windows key, Alt+Tab, Alt+F4. On Windows they go to the PC, except
  Ctrl+Alt+Del and Windows+L, which no program can take.
- The keyboard and the mouse are read once per drawn frame, not at the
  mouse's own rate. Fast games may want better.
- Gamepads do not rumble, and their motion sensors and touchpads are not
  sent. Touch screens act as a mouse. Pens are not sent.
- The clipboard is not shared between this device and the PC.

The receiver for microphone and camera on the PC (the companion in
[return-channel.md](return-channel.md)) stays at the start of step 4. In this
step, capture is checked with a preview and a level meter in the app, and the
microphone can be tested end to end against the microphone build of Apollo,
or against the stand-in host, which saves what it heard to listen to.

Questions settled in this step:

- How the web client shows video inside an egui page: WebCodecs, drawn on a
  canvas behind the app's. A WebGL texture would have gone through egui for
  every picture.
- How a browser and the bridge talk: a WebSocket, not WebRTC. The reasons are
  in the [inventory](inventory.md#the-web-client-moonlight-web-stream).
- Which hardware decoder the Windows client uses: Media Foundation's, given a
  Direct3D 11 device. They hide the differences between graphics cards, and
  OpenH264 stays as the H.264 decoder that works everywhere.
- How to get AV1 when moonlight-common-rust never chooses it: a copy of it
  that Leto changed, until the project chooses it itself.

## 3. The Android client

- Start from Super Artemis, renamed and restyled as Leto, with a simpler
  default interface.
- Microphone (already there) and the phone's cameras over the return channel.
- Move protocol code to the shared Rust core where it pays off.

## 4. The host

- Port Apollo to Rust, Windows first, in the order listed in the
  [inventory](inventory.md): pairing, session negotiation, streams, audio,
  video, input, apps, virtual display.
- The companion from step 2 becomes part of the host.
- The Leto app replaces Apollo's web configuration pages.
- Linux and macOS hosts once Windows works end to end.
