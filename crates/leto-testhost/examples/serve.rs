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

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::{
    io::{self, BufRead},
    process::ExitCode,
    time::Duration,
};

use image::{ImageFormat, Rgb, RgbImage};
use leto_core::model::Pin;
use leto_testhost::{App, FakeHost, HOST_ID, Options, permission};

/// How many inputs the "inputs" command prints, counted from the last.
const SHOWN_INPUTS: usize = 24;

const USAGE: &str = "\
Usage: serve [--name NAME] [--id ID] [--port PORT] [--announce] [--apollo]

  --name NAME   The name the host gives itself (default: Salon)
  --id ID       The unique id it reports (default: a fixed one)
  --port PORT   The port to listen on (default: any free port)
  --announce    Listen on every interface and announce the host on the
                network, so that clients find it on their own
  --apollo      Behave as an Apollo host that lets a newly paired device
                list the apps and watch, and nothing else
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
