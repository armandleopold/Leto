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

Very early. The repository, the checks and a first window exist. Streaming does
not work yet.

| Step | State |
|---|---|
| 1. Foundations | In progress |
| 2. One client for Windows and the web | Not started |
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

You need [Rust](https://rustup.rs). The right version installs itself.

```sh
cargo run -p leto                 # the app
cargo run -p leto-cli -- about    # the command line
cargo xtask web                   # the web client, into dist/web
```

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
