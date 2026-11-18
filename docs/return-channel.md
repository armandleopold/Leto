# The return channel: microphone and camera, from client to host

Status: **design draft**. Nothing here is implemented yet. The microphone part
follows an existing protocol; the camera part is new and marked as such.

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

A full Leto host also announces it in `serverinfo` with one extra element,
`<LetoApiPort>`. Moonlight clients ignore elements they do not know, which is
how Apollo already adds its own.

Draft messages (JSON):

- `GET /leto/v1/info`: versions and capabilities, for example
  `{"api":1,"returnChannel":{"microphone":true,"camera":{"codecs":["h264"],"maxWidth":1920,"maxHeight":1080,"maxFps":30}}}`.
- `POST /leto/v1/return-channel`: the client asks for what it wants
  (`microphone`, `camera` with size, frame rate and bitrate). The host answers
  with the UDP ports, a fresh AES-128 key and a key id for this session. The
  host applies the per-client permissions here.
- `DELETE /leto/v1/return-channel`: the client stops sending.

A Leto client that reaches this API uses it for both microphone and camera.
The microphone packets keep the format above, so the host has a single
microphone receiver for Leto clients and for Super Artemis style clients.

### Camera packets

UDP from client to host, on the base port + 13.

| Offset | Size | Field | Value |
|---|---|---|---|
| 0 | 1 | flags | bit 0: key frame |
| 1 | 1 | packet type | `0x62` |
| 2 | 2 | sequence number | little endian |
| 4 | 4 | timestamp | 90 kHz clock, little endian |
| 8 | 4 | frame index | little endian, starts at 0, never wraps within a session |
| 12 | 2 | fragment index | little endian |
| 14 | 2 | fragment count | little endian |
| 16 | n | payload | fragment of one H.264 access unit (Annex B), then a 16-byte tag |

- **Codec:** H.264, because every client has a hardware encoder for it
  (MediaCodec on Android, Media Foundation on Windows, WebRTC in browsers) and
  Windows decodes it without extra software. The `codecs` list leaves room for
  H.265 and AV1.
- **Default:** 1280 × 720 at 30 frames per second, about 2.5 Mbit/s.
- **Encryption:** AES-128-GCM per packet with the key from the Leto API. The
  nonce is built from the frame index and the fragment index, which never
  repeat within a session. (The 16-bit sequence number would wrap after a few
  minutes and must not be used for this.)
- **Loss:** version 1 has no forward error correction. An incomplete frame is
  dropped, the host keeps showing the last good one, and the client sends a key
  frame every 2 seconds. On-demand key frames and the Reed-Solomon scheme the
  video stream already uses come later if tests over a poor link need them.

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

Windows 10 does not have this API and is not a target for the camera. On Linux
the equivalent is a PipeWire camera node or `v4l2loopback`. macOS needs a
signed camera extension and is out of scope until someone can test it.

### Capture on the clients

| Client | Microphone | Camera |
|---|---|---|
| Windows | WASAPI capture, Opus encoder | Media Foundation capture and hardware H.264 encoder |
| Android | Already in Super Artemis (`AudioRecord`, native Opus) | CameraX into a `MediaCodec` H.264 encoder, front or back camera |
| Web | `getUserMedia`, sent as a WebRTC audio track (already in moonlight-web-stream) | `getUserMedia`, sent as a WebRTC video track |

A browser cannot send UDP packets or present the pairing certificate, so on
the web the bridge does it: the browser sends WebRTC tracks to the bridge, and
the bridge turns them into the packets above. When the bridge is the Leto host
itself, the frames go straight to the virtual devices.

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
   before it ships.
2. **Reading Apollo's key** in companion mode is what avoids a second pairing.
   The alternative is a separate, explicit pairing with the companion.
3. **Default permissions** for a newly paired device: allowed or blocked until
   the host owner ticks a box.
4. **Camera when idle:** remove the device, or keep it with a "camera off"
   card so apps keep their camera selection.
5. **A Leto audio driver** to remove the Steam or VB-CABLE requirement.
6. **Proposing the camera extension upstream** once it has run for a while, so
   other clients and hosts can adopt it.
