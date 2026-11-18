//! Command-line interface to the Leto engine.
//!
//! Every capability of the engine is meant to be reachable from here, so that
//! scripts and agents can drive Leto without the graphical app. Each command
//! prints plain text by default and JSON with `--json`.

mod bridge;
mod pcs;
mod stream;

use std::{path::PathBuf, process::ExitCode};

use clap::{Parser, Subcommand};
use leto_client::{Client, Pin, StreamSettings};
use leto_core::{brand, platform::Platform};
use leto_protocol::ports::{DEFAULT_BASE_PORT, PortMap, Service, Transport};
use serde::Serialize;

#[derive(Parser)]
#[command(name = "leto-cli", version, about = brand::TAGLINE)]
struct Cli {
    /// Print JSON instead of plain text.
    #[arg(long, global = true)]
    json: bool,

    /// Keep Leto's settings in this folder instead of the usual one.
    #[arg(long, global = true, value_name = "FOLDER")]
    settings: Option<PathBuf>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Show the version and what this build can do on this system.
    About,
    /// List the network ports a host uses, for firewall and router rules.
    Ports {
        /// Base port every other port is derived from.
        #[arg(long, default_value_t = DEFAULT_BASE_PORT)]
        base: u16,
    },
    /// List the PCs Leto knows and the ones it finds on the network.
    Pcs {
        /// Seconds to listen for PCs announcing themselves.
        #[arg(long, default_value_t = 2, value_name = "SECONDS")]
        wait: u64,
    },
    /// Add a PC by address, for one the search does not find.
    Add {
        /// IP address or name, with `:port` if the PC does not use 47989.
        address: String,
    },
    /// Pair this device with a PC. Shows a PIN to type on the PC.
    Pair {
        /// Name, address or id of the PC.
        pc: String,
        /// Use this PIN instead of a random one.
        #[arg(long)]
        pin: Option<Pin>,
    },
    /// List the apps a paired PC offers.
    Apps {
        /// Name, address or id of the PC.
        pc: String,
    },
    /// Forget a PC, and ask it to forget this device.
    Forget {
        /// Name, address or id of the PC.
        pc: String,
    },
    /// Stream an app for a moment and report what arrived. Shows no picture.
    Play {
        /// Name, address or id of the PC.
        pc: String,
        /// Title or id of the app.
        app: String,
        /// Seconds to stream for.
        #[arg(long, default_value_t = 5, value_name = "SECONDS")]
        seconds: u64,
        /// Width of the picture to ask for.
        #[arg(long, default_value_t = StreamSettings::default().width)]
        width: u32,
        /// Height of the picture to ask for.
        #[arg(long, default_value_t = StreamSettings::default().height)]
        height: u32,
        /// Pictures per second to ask for.
        #[arg(long, default_value_t = StreamSettings::default().fps)]
        fps: u32,
        /// Video bit rate to ask for, in kilobits per second.
        #[arg(long, default_value_t = StreamSettings::default().bitrate_kbps, value_name = "KBPS")]
        bitrate: u32,
        /// Send the PC this device's microphone meanwhile, if it takes one.
        #[arg(long)]
        microphone: bool,
    },
    /// Close the app running on a PC.
    Quit {
        /// Name, address or id of the PC.
        pc: String,
    },
    /// Serve the web client to browsers and reach the PCs for them, until
    /// stopped. Shows the address to open and the code a new browser needs.
    Bridge {
        /// The port browsers open.
        #[arg(long, default_value_t = leto_bridge::DEFAULT_PORT)]
        port: u16,
        /// The folder `cargo xtask web` built the web client into.
        #[arg(long, default_value = "dist/web", value_name = "FOLDER")]
        web: PathBuf,
        /// Only for browsers on this computer.
        #[arg(long)]
        local: bool,
        /// Forget every browser: each one needs the code again.
        #[arg(long)]
        forget_browsers: bool,
    },
}

#[derive(Serialize)]
struct About {
    name: &'static str,
    version: &'static str,
    homepage: &'static str,
    platform: Option<Platform>,
    can_host: bool,
}

#[derive(Serialize)]
struct PortRow {
    service: Service,
    port: u16,
    transport: Transport,
    baseline: bool,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(&cli) {
        Ok(output) => {
            println!("{output}");
            ExitCode::SUCCESS
        }
        Err(message) => {
            eprintln!("error: {message}");
            ExitCode::FAILURE
        }
    }
}

fn run(cli: &Cli) -> Result<String, String> {
    let json = cli.json;
    match &cli.command {
        Command::About => about(json),
        Command::Ports { base } => ports(*base, json),
        Command::Pcs { wait } => pcs::list(&client(cli)?, *wait, json),
        Command::Add { address } => pcs::add(&client(cli)?, address, json),
        Command::Pair { pc, pin } => pcs::pair(&client(cli)?, pc, *pin, json),
        Command::Apps { pc } => pcs::apps(&client(cli)?, pc, json),
        Command::Forget { pc } => pcs::forget(&client(cli)?, pc, json),
        Command::Play { pc, app, seconds, width, height, fps, bitrate, microphone } => {
            let settings = StreamSettings {
                width: *width,
                height: *height,
                fps: *fps,
                bitrate_kbps: *bitrate,
            };
            stream::play(&client(cli)?, pc, app, *seconds, settings, *microphone, json)
        }
        Command::Quit { pc } => stream::quit(&client(cli)?, pc, json),
        Command::Bridge { port, web, local, forget_browsers } => {
            let asked = bridge::Asked {
                port: *port,
                web,
                local: *local,
                forget_browsers: *forget_browsers,
            };
            // The bridge's own files go next to the engine's.
            let folder = settings_folder(cli)?.join("bridge");
            bridge::run(client(cli)?, folder, &asked, json)
        }
    }
}

/// Where Leto's settings are, as the person asked.
fn settings_folder(cli: &Cli) -> Result<PathBuf, String> {
    match &cli.settings {
        Some(folder) => Ok(folder.clone()),
        None => leto_client::default_directory().map_err(|error| error.to_string()),
    }
}

/// Opens the client engine with the settings the person asked for.
fn client(cli: &Cli) -> Result<Client, String> {
    match &cli.settings {
        Some(folder) => Client::open(folder),
        None => Client::open_default(),
    }
    .map_err(|error| error.to_string())
}

fn about(json: bool) -> Result<String, String> {
    let platform = Platform::current();
    let about = About {
        name: brand::NAME,
        version: leto_core::VERSION,
        homepage: brand::HOMEPAGE,
        platform,
        can_host: platform.is_some_and(Platform::can_host),
    };
    if json {
        return to_json(&about);
    }
    let platform_name = platform.map_or("unsupported platform", Platform::name);
    let role = if about.can_host { "host and client" } else { "client only" };
    Ok(format!(
        "{} {}\n{}\n{}\nPlatform: {platform_name} ({role})",
        about.name,
        about.version,
        brand::TAGLINE,
        about.homepage
    ))
}

fn ports(base: u16, json: bool) -> Result<String, String> {
    let map = PortMap::new(base).map_err(|e| e.to_string())?;
    let rows: Vec<PortRow> = Service::ALL
        .into_iter()
        .map(|service| PortRow {
            service,
            port: map.port(service),
            transport: service.transport(),
            baseline: service.is_baseline(),
        })
        .collect();
    if json {
        return to_json(&rows);
    }
    let mut lines = vec![format!("{:<11} {:>5}  {}", "SERVICE", "PORT", "TRANSPORT")];
    for row in &rows {
        let note = if row.baseline { "" } else { "  (extension)" };
        lines.push(format!("{:<11} {:>5}  {}{note}", row.service.name(), row.port, row.transport));
    }
    Ok(lines.join("\n"))
}

fn to_json<T: Serialize>(value: &T) -> Result<String, String> {
    serde_json::to_string_pretty(value).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_command_line_definition_is_consistent() {
        use clap::CommandFactory as _;
        Cli::command().debug_assert();
    }

    #[test]
    fn ports_lists_every_service_from_the_default_base() {
        let text = ports(DEFAULT_BASE_PORT, false).expect("default base is valid");
        assert_eq!(text.lines().count(), 1 + Service::ALL.len());
        assert!(text.contains("HTTP        47989  TCP"));
        assert!(text.contains("Microphone  48001  UDP  (extension)"));
    }

    #[test]
    fn ports_as_json_is_machine_readable() {
        let json = ports(DEFAULT_BASE_PORT, true).expect("default base is valid");
        let rows: serde_json::Value = serde_json::from_str(&json).expect("valid JSON");
        assert_eq!(rows[1]["service"], "http");
        assert_eq!(rows[1]["port"], 47989);
        assert_eq!(rows[1]["transport"], "tcp");
    }

    #[test]
    fn an_invalid_base_port_is_reported() {
        let error = ports(80, false).expect_err("port 80 is out of range");
        assert!(error.contains("out of range"));
    }
}
