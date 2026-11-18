//! Keeps a stand-in host running, to try a Leto client by hand.
//!
//! ```sh
//! cargo run -p leto-testhost --example serve -- --port 47989 --announce
//! ```
//!
//! Pair from the client, then type the PIN it shows here and press Enter, as
//! you would in the pairing page of a real host. While a client is streaming,
//! type `close` to close the app as its owner would on the host, or `drop`
//! to break the stream and leave the app running.
//!
//! With `--apollo` the host behaves as an Apollo that already knows another
//! device: this one may only list the apps and watch. Type `allow` to give it
//! every permission, as on Apollo's PIN page, and `restrict` to take them
//! back.
//!
//! The host takes the microphone of a client that sends one, like the
//! microphone build of Apollo, and shows how loud it is on the test card.
//! Type `voice` to save what it heard lately as a sound file to listen to.
//! With `--no-microphone` it takes none, like a stock Sunshine or Apollo.
//!
//! It also takes the camera of a client, the way Leto's own host will, and
//! shows it in a corner of the test card. Type `camera` to save the last
//! picture it saw, `camera off` to refuse cameras as the owner of a host
//! could, and `camera on` to take them again. With `--no-camera` it takes
//! none, like every Sunshine and Apollo.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::{
    fs,
    io::{self, BufRead},
    process::ExitCode,
    time::Duration,
};

use image::{ImageFormat, Rgb, RgbImage};
use leto_core::model::Pin;
use leto_testhost::{App, FakeHost, HOST_ID, Options, permission};

/// How many inputs the "inputs" command prints, counted from the last.
const SHOWN_INPUTS: usize = 24;

/// Where the "voice" command saves what the client's microphone sent.
const VOICE_FILE: &str = "leto-voice.wav";
/// Samples per second in what the host heard.
const VOICE_RATE: u32 = 48_000;

/// Where the "camera" command saves the last picture of the client's camera.
const CAMERA_FILE: &str = "leto-camera.png";

const USAGE: &str = "\
Usage: serve [--name NAME] [--id ID] [--port PORT] [--announce] [--apollo]
             [--no-microphone] [--no-camera]

  --name NAME   The name the host gives itself (default: Salon)
  --id ID       The unique id it reports (default: a fixed one)
  --port PORT   The port to listen on (default: any free port)
  --announce    Listen on every interface and announce the host on the
                network, so that clients find it on their own
  --apollo      Behave as an Apollo host that lets a newly paired device
                list the apps and watch, and nothing else
  --no-microphone
                Take no microphone from the client, like a stock Sunshine or
                Apollo
  --no-camera   Take no camera from the client, like every Sunshine and
                Apollo
";

fn main() -> ExitCode {
    let mut options = Options {
        // Real hosts wait until someone types the PIN.
        pin_patience: Duration::from_secs(300),
        apps: vec![
            app(1, "Desktop", true, [38, 70, 83], [20, 33, 61]),
            app(881_448_767, "Steam Big Picture", false, [27, 40, 56], [102, 192, 244]),
            app(1_093_255_277, "Hades II", true, [120, 28, 40], [245, 150, 60]),
            app(205_433_908, "Blender", false, [234, 118, 0], [60, 60, 60]),
            // An app whose cover is not a picture Leto can show.
            App { id: 7, title: "Notepad".to_owned(), hdr: false, cover: b"not an image".to_vec() },
        ],
        id: HOST_ID.to_owned(),
        ..Options::default()
    };
    let mut announce = false;

    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        let mut value = || args.next().ok_or(());
        let parsed = match arg.as_str() {
            "--name" => value().map(|name| options.name = name),
            "--id" => value().map(|id| options.id = id),
            "--port" => value()
                .and_then(|port| port.parse().map_err(|_| ()))
                .map(|port| options.port = port),
            "--announce" => {
                announce = true;
                Ok(())
            }
            "--apollo" => {
                options.permissions = Some(permission::DEFAULT);
                Ok(())
            }
            "--no-microphone" => {
                options.microphone = false;
                Ok(())
            }
            "--no-camera" => {
                options.camera = None;
                Ok(())
            }
            _ => Err(()),
        };
        if parsed.is_err() {
            eprint!("{USAGE}");
            return ExitCode::FAILURE;
        }
    }
    if announce {
        options.interface = "0.0.0.0".to_owned();
    }

    let name = options.name.clone();
    let host = FakeHost::start_with(options);
    println!("Stand-in host \"{name}\" is listening on port {}.", host.port());
    match host.camera_port() {
        Some(port) => println!("It takes a camera, asked for on port {port}."),
        None => println!("It takes no camera."),
    }

    // Kept alive until the end: dropping it withdraws the announcement.
    let _announcement = announce.then(|| {
        let daemon = mdns_sd::ServiceDaemon::new().expect("mDNS needs a network interface");
        let service = mdns_sd::ServiceInfo::new(
            "_nvstream._tcp.local.",
            &name,
            &format!("{}.local.", name.to_lowercase().replace(' ', "-")),
            "",
            host.port(),
            None,
        )
        .expect("the service description is valid")
        .enable_addr_auto();
        daemon.register(service).expect("the announcement can be registered");
        println!("It announces itself on the network.");
        daemon
    });

    println!("Pair from a client, then type the PIN it shows here and press Enter.");
    println!("Type \"close\" to close the running app, \"drop\" to break the stream.");
    println!("Type \"allow\" or \"restrict\" to change what paired devices may do.");
    println!("Type \"inputs\" to see what the keyboard, mouse and gamepads of the client sent.");
    println!("Type \"voice\" to save what the microphone of the client sent to {VOICE_FILE}.");
    println!("Type \"camera\" to save what the camera of the client shows to {CAMERA_FILE},");
    println!("\"camera off\" to refuse cameras and \"camera on\" to take them again.");
    for line in io::stdin().lock().lines() {
        let Ok(line) = line else { break };
        match line.trim() {
            "close" => {
                host.close_app();
                println!("The app is closed.");
            }
            "drop" => {
                host.drop_stream();
                println!("The stream is broken. The app keeps running.");
            }
            "allow" => {
                host.set_permissions(Some(permission::ALL));
                println!("Paired devices may do everything.");
            }
            "restrict" => {
                host.set_permissions(Some(permission::DEFAULT));
                println!("Paired devices may list the apps and watch.");
            }
            "inputs" => {
                // The most recent ones: a mouse alone sends hundreds.
                let inputs = host.inputs();
                let skipped = inputs.len().saturating_sub(SHOWN_INPUTS);
                println!("{} inputs received in this stream.", inputs.len());
                for input in &inputs[skipped..] {
                    println!("  {input:?}");
                }
            }
            "voice" => {
                let heard = host.heard();
                if heard.is_empty() {
                    println!("The microphone of the client sent nothing in this stream.");
                    continue;
                }
                // As loud as the client's own meter says: 1 for a tone that
                // reaches the limits.
                let last = &heard[heard.len().saturating_sub(VOICE_RATE as usize)..];
                let mean =
                    last.iter().map(|sample| sample * sample).sum::<f32>() / last.len() as f32;
                println!(
                    "Heard {:.1} seconds lately, the last one at {:.0}% of the loudest.",
                    heard.len() as f32 / VOICE_RATE as f32,
                    (mean * 2.0).sqrt().min(1.0) * 100.0
                );
                match fs::write(VOICE_FILE, wav(&heard)) {
                    Ok(()) => {
                        println!("Saved to {VOICE_FILE}: play it to hear what the host heard.")
                    }
                    Err(error) => println!("Could not save {VOICE_FILE}: {error}"),
                }
            }
            "camera" => {
                let report = host.camera_report();
                println!(
                    "{} pictures received, {} of them whole on their own, {} lost; \
                     {} packets, {} refused.",
                    report.pictures, report.key_frames, report.lost, report.packets, report.refused
                );
                let Some(seen) = host.seen() else {
                    println!("No client is sending its camera.");
                    continue;
                };
                let saved = image::save_buffer(
                    CAMERA_FILE,
                    &seen.rgba,
                    u32::try_from(seen.width).expect("a picture is not that wide"),
                    u32::try_from(seen.height).expect("a picture is not that tall"),
                    image::ColorType::Rgba8,
                );
                match saved {
                    Ok(()) => println!(
                        "Saved the last one, {} by {}, to {CAMERA_FILE}.",
                        seen.width, seen.height
                    ),
                    Err(error) => println!("Could not save {CAMERA_FILE}: {error}"),
                }
            }
            "camera off" => {
                host.refuse_camera(true);
                println!("Devices may no longer send their camera.");
            }
            "camera on" => {
                host.refuse_camera(false);
                println!("Devices may send their camera.");
            }
            other => match other.parse::<Pin>() {
                Ok(pin) => {
                    host.type_pin(pin);
                    println!("PIN {pin} entered.");
                }
                Err(error) => println!("{error}"),
            },
        }
    }
    ExitCode::SUCCESS
}

/// One channel of sound as a WAV file: 16 bits a sample, uncompressed.
fn wav(samples: &[f32]) -> Vec<u8> {
    let data = u32::try_from(samples.len() * 2).expect("ten seconds of sound are small");
    let mut file = Vec::with_capacity(44 + samples.len() * 2);
    file.extend_from_slice(b"RIFF");
    file.extend_from_slice(&(36 + data).to_le_bytes());
    file.extend_from_slice(b"WAVEfmt ");
    // A format block of 16 bytes: plain samples, one channel.
    file.extend_from_slice(&16u32.to_le_bytes());
    file.extend_from_slice(&1u16.to_le_bytes());
    file.extend_from_slice(&1u16.to_le_bytes());
    file.extend_from_slice(&VOICE_RATE.to_le_bytes());
    // Bytes per second, bytes per sample, bits per sample.
    file.extend_from_slice(&(VOICE_RATE * 2).to_le_bytes());
    file.extend_from_slice(&2u16.to_le_bytes());
    file.extend_from_slice(&16u16.to_le_bytes());
    file.extend_from_slice(b"data");
    file.extend_from_slice(&data.to_le_bytes());
    for sample in samples {
        let level = (sample.clamp(-1.0, 1.0) * f32::from(i16::MAX)) as i16;
        file.extend_from_slice(&level.to_le_bytes());
    }
    file
}

/// An app with a generated cover: a gradient in the usual box-art shape.
fn app(id: u32, title: &str, hdr: bool, top: [u8; 3], bottom: [u8; 3]) -> App {
    const WIDTH: u32 = 300;
    const HEIGHT: u32 = 450;
    let image = RgbImage::from_fn(WIDTH, HEIGHT, |x, y| {
        // Top colour to bottom colour, a little lighter towards the right.
        let down = y as f32 / HEIGHT as f32;
        let across = x as f32 / WIDTH as f32;
        let mix = |from: u8, to: u8| {
            let mixed = f32::from(from) * (1.0 - down) + f32::from(to) * down;
            (mixed + across * 24.0).clamp(0.0, 255.0) as u8
        };
        Rgb([mix(top[0], bottom[0]), mix(top[1], bottom[1]), mix(top[2], bottom[2])])
    });
    let mut cover = io::Cursor::new(Vec::new());
    image.write_to(&mut cover, ImageFormat::Png).expect("a PNG can be written to memory");
    App { id, title: title.to_owned(), hdr, cover: cover.into_inner() }
}
