<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="assets/brand/png/leto-logo.png" />
    <img src="assets/brand/png/leto-logo-on-light.png" alt="Leto" width="301" />
  </picture>
</p>

# Leto

Remote desktop and game streaming, unified. One app, written in Rust, for the
PC you share and for every device you reach it from: desktop, phone and
browser.

Leto is named after the mother of Apollo and Artemis. It starts from the
projects of the same names, [Apollo](https://github.com/ClassicOldSong/Apollo)
and [Artemis](https://github.com/ClassicOldSong/moonlight-android), and stays
compatible with [Moonlight](https://moonlight-stream.org) clients and
Sunshine hosts.

## Status

Very early. The app finds PCs that run Apollo or Sunshine, pairs with them,
lists their apps and streams one: you see its picture, hear its sound and
play with the keyboard and the mouse of your device. It does so as a desktop
app, with gamepads too, and in a browser, through a small bridge that runs on
your network. Both send the microphone of your device to a PC that takes
one, which Apollo and Sunshine only do in special builds for now. Both show
what your camera sees and whether your microphone hears you, on a screen of
their own. Both also send the camera to a PC that takes one, which no Apollo
or Sunshine does: that needs Leto on the PC, which is step 4.
Nothing has been tried against a real PC yet, only against the stand-in host
of the tests, and no real camera has been tried either.

| Step | State |
|---|---|
| 1. Foundations | Done |
| 2. One client for Windows and the web | In progress |
| 3. The Android client | Not started |
| 4. The host, ported from Apollo | Not started |

## Goals

- **One app** instead of a host program, a desktop client, a mobile client and
  a web client that each look and behave differently.
- **Simple first.** It should work without opening a settings page.
- **Microphone and camera to the host.** The microphone and the camera of the
  device in your hand appear on the PC as a normal microphone and webcam.
- **Compatible.** Existing Moonlight and Artemis clients work with a Leto
  host, and Leto works with existing Sunshine and Apollo hosts.

## Try it

You need [Rust](https://rustup.rs) and a C++ compiler (the Visual Studio
Build Tools on Windows). The right Rust version installs itself. Installing
[NASM](https://www.nasm.us) too makes video decoding several times faster.

```sh
cargo run -p leto                 # the app
cargo run -p leto-cli -- pcs      # the command line: the PCs on your network
cargo xtask web                   # the web client, into dist/web
```

The app looks for your PC on the network. Choose it, type the PIN the app
shows in Apollo or Sunshine on the PC, then choose an app to stream it.

To use Leto from a browser, on this computer or another device of your
network, build the web client and start its bridge:

```sh
cargo xtask web                   # once, and after each change
cargo run -p leto-cli -- bridge   # prints the address to open, and a code
```

Open the address in the browser. It warns that the certificate is the
bridge's own: go on. Then type the code the bridge printed. The browser is
asked once.

Something went wrong? `leto-cli about` prints the folder of Leto's logs. Send
that folder, which holds no secret, and never the folder around it, which
holds this device's key.

## Documentation

- [Architecture](docs/architecture.md): how the code is organised.
- [Roadmap](docs/roadmap.md): the four steps.
- [Inventory](docs/inventory.md): the projects Leto builds on and what it
  takes from each.
- [Return channel](docs/return-channel.md): the design for microphone and
  camera forwarding.
- [AGENTS.md](AGENTS.md): how to work in this repository.

## Licence

GPL-3.0-only, like Apollo and Sunshine. See [LICENSE](LICENSE).
