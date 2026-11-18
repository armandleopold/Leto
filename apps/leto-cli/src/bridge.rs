//! The `bridge` command: serve the web client to browsers, and reach the
//! PCs for them, until stopped.

use std::{
    net::{IpAddr, Ipv4Addr},
    path::{Path, PathBuf},
    sync::Arc,
    thread,
};

use leto_bridge::{Bridge, Options, computer_name, gate::spaced, local_address};
use leto_client::Client;
use serde::Serialize;

use crate::to_json;

/// What the bridge says when it starts.
#[derive(Debug, Serialize)]
struct Started {
    /// Where browsers open it, the address for other devices first.
    addresses: Vec<String>,
    /// The certificate's SHA-256 fingerprint, the way browsers show it.
    fingerprint: String,
    /// The code the first browser has to be given.
    code: String,
    /// Browsers that need no code any more.
    known_browsers: usize,
    /// The web client is there to serve.
    web_client_found: bool,
}

/// A code drawn after the first.
#[derive(Debug, Serialize)]
struct NewCode<'a> {
    code: &'a str,
}

pub struct Asked<'a> {
    pub port: u16,
    /// Where `cargo xtask web` built the web client.
    pub web: &'a Path,
    /// Only for browsers on this computer.
    pub local: bool,
    pub forget_browsers: bool,
}

/// Runs the bridge with its files in `folder`. Returns only if it cannot
/// start.
pub fn run(
    client: Client,
    folder: PathBuf,
    asked: &Asked<'_>,
    json: bool,
) -> Result<String, String> {
    let interface = if asked.local { Ipv4Addr::LOCALHOST } else { Ipv4Addr::UNSPECIFIED };
    let options = Options {
        folder,
        pages: asked.web.to_owned(),
        interface: IpAddr::V4(interface),
        port: asked.port,
    };
    let announce = Box::new(move |code: &str| {
        if json {
            // One object per line, for whoever reads this with a program.
            if let Ok(line) = serde_json::to_string(&NewCode { code }) {
                println!("{line}");
            }
        } else {
            println!("\nThe next new browser needs this code:\n\n    {}\n", spaced(code));
        }
    });
    let bridge = Bridge::start(Arc::new(client), options, announce)
        .map_err(|error| format!("the bridge could not start on port {}: {error}", asked.port))?;
    if asked.forget_browsers {
        bridge.gate().forget_browsers().map_err(|error| error.to_string())?;
    }

    let started = Started {
        addresses: addresses(bridge.port(), asked.local),
        fingerprint: bridge.fingerprint().map_err(|error| error.to_string())?,
        code: bridge.code(),
        known_browsers: bridge.gate().browsers(),
        web_client_found: asked.web.join("index.html").is_file(),
    };
    println!("{}", if json { to_json(&started)? } else { welcome(&started, asked.web) });

    // The bridge works on threads of its own until the program is stopped.
    loop {
        thread::park();
    }
}

/// Where browsers open the bridge.
fn addresses(port: u16, local: bool) -> Vec<String> {
    let mut addresses = Vec::new();
    if !local {
        if let Some(address) = local_address() {
            addresses.push(format!("https://{address}:{port}"));
        }
        let name = computer_name();
        if !name.is_empty() && name != "localhost" {
            addresses.push(format!("https://{name}:{port}"));
        }
    }
    addresses.push(format!("https://localhost:{port}"));
    addresses
}

fn welcome(started: &Started, web: &Path) -> String {
    let mut lines =
        vec!["Leto's bridge is running. Open it in a browser:".to_owned(), String::new()];
    let last = started.addresses.len() - 1;
    for (index, address) in started.addresses.iter().enumerate() {
        let note = if index == last { "   (on this computer)" } else { "" };
        lines.push(format!("    {address}{note}"));
    }
    lines.push(String::new());
    lines.push(
        "The browser will warn that nobody vouches for this address. That is expected on a home \
         network: accept it once. To be sure it is this bridge, compare the certificate's \
         SHA-256 fingerprint:"
            .to_owned(),
    );
    lines.push(String::new());
    lines.push(format!("    {}", started.fingerprint));
    lines.push(String::new());
    let known = match started.known_browsers {
        0 => "A browser that opens the bridge for the first time needs this code:".to_owned(),
        1 => "1 browser is known. A new one needs this code:".to_owned(),
        known => format!("{known} browsers are known. A new one needs this code:"),
    };
    lines.push(known);
    lines.push(String::new());
    lines.push(format!("    {}", spaced(&started.code)));
    lines.push(String::new());
    if !started.web_client_found {
        lines.push(format!(
            "There is no web client in {} yet. Build it with `cargo xtask web`: the bridge \
             serves it as soon as it is there.",
            web.display()
        ));
        lines.push(String::new());
    }
    lines.push("Stop the bridge with Ctrl+C.".to_owned());
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_address_for_this_computer_comes_last() {
        let all = addresses(5386, false);
        assert_eq!(all.last().map(String::as_str), Some("https://localhost:5386"));
        assert_eq!(addresses(8443, true), ["https://localhost:8443"]);
    }

    #[test]
    fn the_welcome_says_where_to_go_and_which_code_to_type() {
        let started = Started {
            addresses: vec!["https://192.168.1.20:5386".into(), "https://localhost:5386".into()],
            fingerprint: "AB:CD".into(),
            code: "12345678".into(),
            known_browsers: 0,
            web_client_found: false,
        };
        let text = welcome(&started, Path::new("dist/web"));
        assert!(text.contains("    https://192.168.1.20:5386\n"));
        assert!(text.contains("    https://localhost:5386   (on this computer)\n"));
        assert!(text.contains("    1234 5678\n"));
        assert!(text.contains("    AB:CD\n"));
        assert!(text.contains("no web client in dist/web yet"));

        let ready = Started { known_browsers: 2, web_client_found: true, ..started };
        let text = welcome(&ready, Path::new("dist/web"));
        assert!(text.contains("2 browsers are known."));
        assert!(!text.contains("no web client"));
    }
}
