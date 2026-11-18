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
- Next: the same in the browser through the bridge, then capture of the
  microphone and the camera.
- Later in this step: decoding on the graphics card, H.265, HDR, surround
  sound, and the settings of Advanced mode (size, pictures per second, bit
  rate).

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
microphone can be tested end to end against the microphone build of Apollo.

Questions to settle at the start of this step:

- How the web client shows video inside an egui page: WebCodecs into a WebGL
  texture, or a `<video>` element behind the canvas.
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
