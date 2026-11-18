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
- Done: the same in the browser. `leto-cli bridge` serves the web client and
  relays the stream, which the browser decodes itself. See
  [architecture.md](architecture.md#the-web-client-and-its-bridge).
- Done: the microphone, from the desktop app, to a PC that takes one (the
  microphone build of Apollo, Foundation Sunshine). One switch in the bar
  over the picture, off at the start of every stream. See
  [architecture.md](architecture.md#the-microphone).
- Next: the microphone from the browser, then capture of the camera.
- Later in this step: decoding on the graphics card, H.265, HDR, surround
  sound, and the settings of Advanced mode (size, pictures per second, bit
  rate).

What the web client does not do yet:

- Gamepads whose layout the browser does not know. It names the buttons of
  the usual ones ("standard" in its words), and Leto leaves the others out
  instead of guessing which button is which.
- Starting the bridge from the app. It is a command for now, and the code it
  asks a new browser for is printed in its terminal.
- Reaching the bridge from outside the home network. It is built for the
  network it runs on.
- H.265 and AV1: like the desktop app, it asks PCs for H.264.
- It was checked in Chromium only. Firefox and Safari have the same decoders
  in their recent versions, and nothing here was tried in them.
- The web client is one file of 5 MB that the bridge sends as it is, without
  compressing it.

What the microphone does not do yet:

- It was never sent to a real host. The stand-in host follows the Apollo
  microphone build and Foundation Sunshine as read in their source, and what
  it hears was checked there. The microphone itself was a made-up one on
  Linux: no real microphone was captured, on any system.
- It is the system's default microphone, as the system gives it: no choice of
  device, no noise suppression, no echo cancellation. With loudspeakers
  instead of a headset, the PC hears its own sound again.
- Sent to a stock Apollo or Sunshine it would go nowhere, so the switch is
  dimmed there. The receiver that changes this is the companion of step 4.
- Nothing remembers that it was on: it is off at the start of every stream.
- The web client has no switch for it yet.

What input does not do yet:

- The numeric keypad is sent as the row of digits above the letters, because
  egui reports both as the same keys. On a PC with a French layout its digits
  therefore come out as the signs of that row. Fixing it means reading the
  window's key events below egui.
- Keys the system keeps for itself stay on this device: the Windows key,
  Alt+Tab, Alt+F4, Ctrl+Alt+Del.
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

Questions still open:

- Which hardware decoder the Windows client uses first (Media Foundation or
  Direct3D 11 video). Software decoding with OpenH264 is in place as the
  path that works everywhere.
- AV1, which moonlight-common-rust does not support yet.

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
