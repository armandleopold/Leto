# The return channel: microphone and camera, from client to host

Status: **design draft** for the host side, of which nothing exists yet. The
client side is there:

- the microphone, in the desktop app and the web client (see
  [architecture.md](architecture.md#the-microphone)): an existing protocol,
  followed as it is;
- the camera, in the desktop app, the command line and the web client (see
  [architecture.md](architecture.md#the-camera)): a protocol of Leto's own,
  described below as the client and the stand-in host of the tests speak it
  (`leto-return`). The web client encodes its camera and hands the pictures
  to its bridge, which speaks that protocol for it.

No Sunshine or Apollo takes a camera, so until Leto's host exists the camera
of a device reaches the stand-in host and nothing else.

## Goal

A classic stream flows one way: screen and sound from the host to the client.
Leto adds the other direction. The microphone and the camera of the device you
hold show up on the host PC as an ordinary microphone and an ordinary webcam,
so any app there (Teams, Discord, a browser, OBS) can use them without knowing
Leto exists.

This is a main acceptance criterion of the project. It has to work from the
three clients that can be tested (Windows, Android, Web) towards a Windows 11
host.

## What exists today

| | Microphone | Camera |
|---|---|---|
| Sunshine, stock Apollo | No | No |
| Apollo microphone fork, "Foundation Sunshine" | Yes, host side | No |
| Moonlight, Artemis | No | No |
| Super Artemis, moonlight-web-stream, moonlight-common-rust | Yes, client side | No |

Sources and commits are in [inventory.md](inventory.md).

## Principles

1. **Compatible first.** A Moonlight or Artemis client must see a Leto host as
   a normal Sunshine or Apollo host, and a Leto client must stream from a
   normal Sunshine or Apollo host. The return channel is extra, never required.
2. **Reuse the microphone protocol that already exists.** Super Artemis and
   moonlight-web-stream already speak it. Leto adopts it unchanged.
3. **Always encrypted.** A microphone or camera stream never travels in clear.
4. **Off until the person turns it on,** with a visible indicator while live.
5. **One switch each in the interface.** No device pickers or codec settings in
   Simple mode.

## Microphone

Leto implements the microphone extension as read in the Apollo microphone fork
(host side) and in moonlight-common-rust (client side).

Negotiation, inside the normal RTSP session:

1. The host advertises the stream in its answer to `DESCRIBE`:

   ```
   m=audio <port> RTP/AVP 96
   a=rtpmap:96 opus/48000/1
   a=fmtp:96 minptime=10;useinbandfec=1
   ```

   The port is the base port + 12 (48001 by default; moonlight-common-rust
   falls back to 47996 when the host gives none). The host also sets the
   microphone bit (`0x08`) in `x-ss-general.encryptionSupported` and
   `encryptionRequested`.
2. The client sends `SETUP streamid=mic/0/0`, then `PLAY`, and enables the
   microphone bit in `x-ss-general.encryptionEnabled`.

Foundation Sunshine sets the microphone bit in `encryptionSupported` and
answers `SETUP streamid=mic` with success even while its microphone is turned
off. The media line is what tells: Leto takes a host to offer a microphone
when the description has both the line and the bit.

moonlight-common-rust decides before the handshake whether to send that
`SETUP`, from the kind of host alone, and a refused `SETUP` fails the whole
stream. Leto therefore reads the description first, with a `DESCRIBE` of its
own on a separate connection: Sunshine and its descendants answer every RTSP
request on a new connection, and a description is only read.

Packets, UDP from client to host, at most 1400 bytes:

| Offset | Size | Field | Value |
|---|---|---|---|
| 0 | 1 | flags | 0 |
| 1 | 1 | packet type | `0x61` |
| 2 | 2 | sequence number | little endian |
| 4 | 4 | timestamp | little endian |
| 8 | 4 | SSRC | `0x12345678`, little endian |
| 12 | n | payload | one Opus frame: 48 kHz, mono, 20 ms (960 samples) |

The payload is encrypted with AES-CBC using the session's remote-input key. The
initialisation vector is the key id plus the sequence number, big endian. The
fork refuses the microphone when the client did not negotiate encryption, and
Leto does the same.

### On the host

Windows offers no user-mode API to create a microphone. An audio device has to
come from a driver. Options:

| Option | Install | Notes |
|---|---|---|
| Steam Streaming Microphone | Comes with Steam's Remote Play drivers | What the Apollo fork uses. Free, signed, common on gaming PCs |
| VB-CABLE or another virtual cable | Separate installer | Works the same way: write to its playback side, apps record from its capture side |
| A Leto audio driver | Shipped with Leto | Best experience, but a kernel driver needs an EV certificate and Microsoft attestation signing |

Version 1: Leto decodes Opus and writes the sound with WASAPI into a virtual
cable. It detects Steam Streaming Microphone and VB-CABLE on its own, and the
interface explains in one screen what to install when neither is found. A Leto
driver is an open question for later.

On Linux the target is a PipeWire or PulseAudio virtual source, which needs no
driver. macOS needs a virtual audio driver too and is out of scope until
someone can test it.

## Camera (new, Leto extension)

### Why not copy the microphone approach

The microphone extension adds a media section to the RTSP session description.
Doing the same for video is risky: Moonlight clients detect codecs by searching
that text (`AV1/90000`, `sprop-parameter-sets`), and a second video section
could confuse clients Leto cannot test. The camera also has needs the RTSP
handshake handles badly: it is switched on and off during a session, and its
resolution can change when a phone rotates.

So the camera is negotiated **beside** the GameStream session, on a small Leto
API, and leaves the GameStream messages untouched.

### Leto API

A Leto host (or the companion described below) listens on HTTPS at the base
port + 13 (48002 by default; offsets up to +12 and +21 are taken, +13 is free
in every project read). The connection uses the certificates exchanged during
normal Moonlight pairing, in both directions, so only paired devices can reach
it and no second pairing is needed.

A client finds out by asking: it sends `GET /leto/v1/info` to the port + 13
of a PC it is paired with, once the stream plays so that the question never
delays it, and waits three seconds at most. Whatever goes wrong there, a
refused connection, another certificate, an answer that is not the one below,
means that the PC takes no camera. (A full Leto host may also announce the API
in `serverinfo` with one extra element, `<LetoApiPort>`: Moonlight clients
ignore elements they do not know, which is how Apollo already adds its own.
Nothing reads it yet.)

Messages, with JSON bodies. A reader ignores the fields it does not know, so
that either side can say more later.

- `GET /leto/v1/info`: what the host takes, for example
  `{"api":1,"returnChannel":{"microphone":true,"camera":{"codecs":["h264"],"maxWidth":1920,"maxHeight":1080,"maxFps":30}}}`.
  No `camera` means that it takes none.
- `POST /leto/v1/return-channel`: the client asks for what it wants, for
  example
  `{"microphone":false,"camera":{"codec":"h264","width":1280,"height":720,"fps":30,"bitrateKbps":2500}}`.
  The host applies the per-client permissions here, and answers with where the
  packets go and what seals them:
  `{"camera":{"port":48002,"key":"<32 hexadecimal digits>","keyId":7}}`. The
  key is a fresh AES-128 key, made for this camera, and the packets go to the
  address that answered.
- `DELETE /leto/v1/return-channel?keyId=7`: the client stopped sending what
  the key of that number sealed. The host leaves alone what it agreed to
  since: a camera turned off and on again at once sends its goodbye and its
  next ask on two connections, and the goodbye may arrive last. Without a
  number, the goodbye ends whatever the device sends.

A host that will not agree answers with a status and nothing else:

| Status | Meaning | What the client says |
|---|---|---|
| 403 | The owner of the PC does not let this device send its camera | "This PC does not let this device send its camera. Its owner can allow it on the PC." |
| 409 | Another device is sending its camera | That another device is, and to try later |
| 400 | The host cannot take pictures of this size, or this many | That it cannot take these pictures |
| 404, 405 | Nothing here takes a camera | The same as a PC that takes none |

One device sends at a time. The same device asking again replaces what was
agreed, with a new key. A host that hears nothing from a camera for five
seconds takes the device to be gone and lets another one in, so a client that
vanished without a goodbye does not keep the place.

The microphone is asked for on the same API by a Leto client that reaches it,
and its packets keep the format above, so the host has a single microphone
receiver for Leto clients and for Super Artemis style clients. This part is
not written yet: the client still asks for the microphone inside the RTSP
session.

### Camera packets

UDP from client to host, to the port of the answer (the base port + 13 by
default), at most 1400 bytes each. Numbers are little endian.

| Offset | Size | Field | Value |
|---|---|---|---|
| 0 | 1 | flags | bit 0: key frame |
| 1 | 1 | packet type | `0x62` |
| 2 | 2 | sequence number | wraps |
| 4 | 4 | timestamp | 90 kHz clock, from the first picture |
| 8 | 4 | frame index | starts at 0, never wraps within a camera |
| 12 | 2 | fragment index | starts at 0 |
| 14 | 2 | fragment count | at most 4096 |
| 16 | n | payload | fragment of one H.264 access unit (Annex B), sealed |
| 16 + n | 16 | tag | the seal of the payload and of the header |

- **Codec:** H.264, because every client has a hardware encoder for it
  (MediaCodec on Android, Media Foundation on Windows, WebCodecs in browsers)
  and Windows decodes it without extra software. The `codecs` list leaves room
  for H.265 and AV1. The desktop client encodes in software for now, with
  OpenH264, as it decodes.
- **Default:** 1280 × 720 at 30 frames per second, about 2.5 Mbit/s. Smaller
  pictures get less in proportion, never under 300 kbit/s; a camera larger
  than what the host takes is halved until it fits.
- **Encryption:** AES-128-GCM per packet with the key of the answer. The
  12-byte nonce is the key id, the frame index and the fragment index, then
  two zero bytes: it never repeats, because a frame index is never used twice
  under one key. (The 16-bit sequence number would wrap after a few minutes
  and must not be used for this.) The 16 bytes of the header are the
  associated data of the seal, so nobody on the way can change what a packet
  says about itself. A host drops what does not open, and what arrives under
  the key of a camera that stopped.
- **Loss:** version 1 has no forward error correction. An incomplete frame is
  dropped with the ones that follow it until the next key frame, since each
  builds on the one before; the host keeps showing the last good one, and the
  client sends a key frame every 2 seconds. On-demand key frames and the
  Reed-Solomon scheme the video stream already uses come later if tests over a
  poor link need them.
- **Late is lost:** a packet that finds no room on its way out within a tenth
  of a second is left out, and the encoder leaves pictures out rather than
  fall behind.

### On the host

Windows 11 can create a camera without a kernel driver, through Media
Foundation's virtual camera API (`MFCreateVirtualCamera`, Windows 11 build
22000 and later):

- Leto ships a small Media Foundation media source as a DLL, registered once
  with administrator rights when Leto is installed.
- While a client sends its camera, the host process creates a session-lifetime
  virtual camera named "Leto Camera". Windows loads the media source inside its
  camera service, and Leto passes decoded frames to it through shared memory.
- Apps see a normal webcam. When no client is sending, the device either
  disappears or shows a "camera off" card (open question below).

This is the riskiest piece of the whole return channel: a custom media source
is unusual code, it runs inside a Windows service, and it has to be written
with `unsafe` COM calls. It gets its own crate and an early prototype.

Until that exists, the app borrows the camera OBS Studio installs: it writes
the decoded pictures, as NV12, in the shared memory OBS's own virtual camera
reads (`OBSVirtualCamVideo`, `leto-system::vcam`). Apps then see them as "OBS
Virtual Camera". It needs OBS installed, and only one program can feed that
camera at a time: while OBS's own virtual camera is started, Leto waits and
tries again every two seconds.

Windows 10 does not have this API and is not a target for the camera. On Linux
the equivalent is a PipeWire camera node or `v4l2loopback`. macOS needs a
signed camera extension and is out of scope until someone can test it.

### Capture on the clients

| Client | Microphone | Camera |
|---|---|---|
| Windows | WASAPI capture, Opus encoder | Media Foundation capture and hardware H.264 encoder |
| Android | Already in Super Artemis (`AudioRecord`, native Opus) | CameraX into a `MediaCodec` H.264 encoder, front or back camera |
| Web | `getUserMedia`, sent as a WebRTC audio track (already in moonlight-web-stream) | `getUserMedia`, sent as a WebRTC video track |

This table is the plan. What exists is in
[architecture.md](architecture.md#the-camera): the desktop app captures with
what each system offers and encodes in software, and the web client sends its
microphone and its camera to the bridge over its WebSocket, encoded by the
browser.

A browser cannot send UDP packets or present the pairing certificate, so on
the web the bridge does it. The browser encodes its camera as H.264 and sends
each picture to the bridge over their WebSocket, inside its TLS. The bridge
asks the PC on the Leto API, as the paired device it is, and turns the
pictures into the packets above without looking into them: see
[architecture.md](architecture.md#from-a-browser). When the bridge is the
Leto host itself, the frames go straight to the virtual devices.

## Companion mode: the return channel next to Apollo

The Leto host will take a long time to replace Apollo. The return channel must
not wait for all of it. The first piece of host code, at the start of step 4,
is therefore a **companion**: a small Leto process that runs beside an
unmodified Apollo and implements only the Leto API, the two receivers and the
two virtual devices.

- The client streams screen and sound from Apollo as usual, and sends
  microphone and camera to the companion.
- The companion finds Apollo's configuration folder and reads, without ever
  writing: the list of paired client certificates (`sunshine_state.json`) and
  the host certificate and key (`credentials/`). It can then present the same
  identity as Apollo and accept exactly the devices Apollo has paired.
- When the Leto host is complete, the companion's code is simply part of it.

The companion exists for the camera: the crate `leto-host`, which the Leto
app runs from its "This PC" screen, and `leto-cli companion` without a
window. Where it had to choose:

- **Which Apollo.** The `config` folder beside the program under
  `Program Files\Apollo` or `Program Files\Sunshine` on Windows,
  `~/.config/apollo` or `~/.config/sunshine` on Linux, or the one named with
  `--apollo`. `sunshine.conf` there says the port and may move the three
  files, as Apollo's own `config.cpp` reads it.
- **Who is paired** is read from `sunshine_state.json` again at every
  request, so a device paired or removed on Apollo's page is let in or kept
  out at once, with no restart.
- **Who may send a camera.** Apollo has no permission for it. The companion
  takes the camera of a device that Apollo lets use both the keyboard and
  the mouse, which is how its owner says a device is trusted with the PC:
  the first device paired has both, and the others once ticked on Apollo's
  page. The others are answered 403. A Sunshine keeps no permissions, and
  every device it is paired with may. Leto's own permission for the camera
  comes with Leto's own list of devices.
- **The private key** is read into the TLS setup and nowhere else: not
  logged, not copied.

## What works with what

| Client | Sunshine, stock Apollo | Apollo with the microphone patch | Apollo + Leto companion | Leto host |
|---|---|---|---|---|
| Moonlight, Artemis | Stream | Stream | Stream | Stream |
| Super Artemis | Stream | Stream + microphone | Stream | Stream + microphone |
| Leto | Stream | Stream + microphone | Stream + microphone + camera | Stream + microphone + camera |

## Permissions and privacy

- The client asks the operating system for microphone and camera access only
  when the person first turns the switch on.
- Both switches are off at the start of every session unless the person chose
  "remember for this host".
- The host has two per-device permissions, "may send microphone" and "may send
  camera", next to the ones Apollo already has. They are checked when the
  return channel opens.
- The host shows which device is sending while a stream is live.

## Targets to measure

| | Target |
|---|---|
| Microphone delay added by Leto, on a local network | under 60 ms |
| Camera delay, lens to host app, on a local network | under 150 ms |
| Camera default | 720p at 30 frames per second |

## Open questions

1. **Port + 13** for the Leto API and the camera: free in the code read, but
   not reserved anywhere. To raise with the Apollo and Sunshine maintainers
   before it ships. A client finds the API by asking there, so something else
   that listens on that port costs a failed handshake, and a firewall that
   lets the question fall without a word costs the three seconds of the
   client's patience, after the stream plays. Neither was tried against a real
   Apollo or Sunshine.
2. **Reading Apollo's key** in companion mode is what avoids a second pairing.
   The alternative is a separate, explicit pairing with the companion.
3. **Default permissions** for a newly paired device: allowed or blocked until
   the host owner ticks a box.
4. **Camera when idle:** remove the device, or keep it with a "camera off"
   card so apps keep their camera selection.
5. **A Leto audio driver** to remove the Steam or VB-CABLE requirement.
6. **Proposing the camera extension upstream** once it has run for a while, so
   other clients and hosts can adopt it.
