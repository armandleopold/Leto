//! Command-line interface to the Leto engine.
//!
//! Every capability of the engine is meant to be reachable from here, so that
//! scripts and agents can drive Leto without the graphical app. Each command
//! prints plain text by default and JSON with `--json`.

use std::process::ExitCode;

use clap::{Parser, Subcommand};
use leto_core::{brand, platform::Platform};
use leto_protocol::ports::{DEFAULT_BASE_PORT, PortMap, Service, Transport};
use serde::Serialize;

#[derive(Parser)]
#[command(name = "leto-cli", version, about = brand::TAGLINE)]
struct Cli {
    /// Print JSON instead of plain text.
    #[arg(long, global = true)]
    json: bool,

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
    match cli.command {
        Command::About => about(cli.json),
        Command::Ports { base } => ports(base, cli.json),
    }
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
