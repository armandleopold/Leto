//! The commands about PCs: find them, pair, list their apps.

use std::time::Duration;

use leto_client::{Client, HostSummary, Pin, Reachability, join_address};

use crate::to_json;

/// How long to listen when a PC is named that Leto has not saved.
const QUICK_SEARCH: Duration = Duration::from_secs(2);

pub fn list(client: &Client, wait: u64, json: bool) -> Result<String, String> {
    client.refresh();
    if let Err(error) = client.discover(Duration::from_secs(wait)) {
        // The saved PCs are still worth showing.
        eprintln!("warning: {error}");
    }
    let hosts = client.hosts();
    if json {
        return to_json(&hosts);
    }
    if hosts.is_empty() {
        return Ok("No PC found. If yours is on, add it with `leto-cli add <address>`.".to_owned());
    }
    let mut lines =
        vec![format!("{:<20} {:<21} {:<19} {}", "NAME", "ADDRESS", "SOFTWARE", "STATE")];
    for host in &hosts {
        lines.push(format!(
            "{:<20} {:<21} {:<19} {}",
            host.name,
            join_address(&host.address, host.port),
            host.kind.name(),
            state(host)
        ));
    }
    Ok(lines.join("\n"))
}

pub fn add(client: &Client, address: &str, json: bool) -> Result<String, String> {
    let host = client.add(address).map_err(|error| error.to_string())?;
    if json {
        return to_json(&host);
    }
    let next = if host.paired { "It is paired." } else { "Pair with `leto-cli pair`." };
    Ok(format!("Added {} ({}). {next}", host.name, host.kind.name()))
}

pub fn pair(client: &Client, pc: &str, pin: Option<Pin>, json: bool) -> Result<String, String> {
    let host = find(client, pc, true)?;
    let pin = match pin {
        Some(pin) => pin,
        None => client.new_pin().map_err(|error| error.to_string())?,
    };
    // Shown now: the call below waits for someone to type it on the PC.
    eprintln!("Type this PIN on {}: {pin}", host.name);
    let host = client.pair(&host.id, pin).map_err(|error| error.to_string())?;
    if json {
        return to_json(&host);
    }
    Ok(format!("Paired with {}.", host.name))
}

pub fn apps(client: &Client, pc: &str, json: bool) -> Result<String, String> {
    let host = find(client, pc, false)?;
    let apps = client.apps(&host.id).map_err(|error| error.to_string())?;
    if json {
        return to_json(&apps);
    }
    let mut lines = vec![format!("{:>10}  {}", "ID", "APP")];
    for app in &apps {
        let hdr = if app.hdr { "  (HDR)" } else { "" };
        lines.push(format!("{:>10}  {}{hdr}", app.id, app.title));
    }
    Ok(lines.join("\n"))
}

pub fn forget(client: &Client, pc: &str, json: bool) -> Result<String, String> {
    let host = find(client, pc, false)?;
    // Wake the connection first, so that the PC can be told too.
    client.refresh();
    client.forget(&host.id).map_err(|error| error.to_string())?;
    if json {
        return to_json(&host);
    }
    Ok(format!("Forgot {}.", host.name))
}

/// Finds the PC a person means by a name, an address or an id: among the
/// saved PCs first, then on the network, then, with `or_add`, by taking the
/// text as the address of a PC to add.
fn find(client: &Client, wanted: &str, or_add: bool) -> Result<HostSummary, String> {
    if let Some(found) = pick(&client.hosts(), wanted)? {
        return Ok(found);
    }
    if client.discover(QUICK_SEARCH).is_ok()
        && let Some(found) = pick(&client.hosts(), wanted)?
    {
        return Ok(found);
    }
    if or_add {
        return client.add(wanted).map_err(|error| format!("\"{wanted}\": {error}"));
    }
    Err(format!("Leto knows no PC called \"{wanted}\". `leto-cli pcs` lists the ones it knows."))
}

fn pick(hosts: &[HostSummary], wanted: &str) -> Result<Option<HostSummary>, String> {
    let wanted = wanted.trim();
    let mut matches = hosts.iter().filter(|host| {
        host.id.0 == wanted
            || host.name.to_lowercase() == wanted.to_lowercase()
            || host.address == wanted
            || join_address(&host.address, host.port) == wanted
    });
    match (matches.next(), matches.next()) {
        (Some(_), Some(_)) => {
            Err(format!("several PCs match \"{wanted}\". Use the address or the id instead."))
        }
        (found, _) => Ok(found.cloned()),
    }
}

fn state(host: &HostSummary) -> &'static str {
    match (host.reachability, host.paired, host.busy) {
        (Reachability::Offline, ..) => "offline",
        (_, false, _) => "not paired",
        (_, true, true) => "paired, an app is running",
        (_, true, false) => "paired",
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use std::{fs, path::PathBuf};

    use leto_testhost::{FakeHost, HOST_NAME};

    use super::*;

    /// A client with its settings in a folder of its own, removed afterwards.
    struct Settings(PathBuf);

    impl Settings {
        fn new(test: &str) -> Self {
            let path = std::env::temp_dir().join(format!("leto-cli-{}-{test}", std::process::id()));
            let _ = fs::remove_dir_all(&path);
            Self(path)
        }

        fn client(&self) -> Client {
            Client::open(&self.0).unwrap()
        }
    }

    impl Drop for Settings {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn a_pc_is_added_paired_listed_and_forgotten() {
        let settings = Settings::new("whole");
        let host = FakeHost::start();
        let pin: Pin = "4821".parse().unwrap();
        host.type_pin(pin);

        // Pairing by address adds the PC on the way.
        let paired = pair(&settings.client(), &host.address(), Some(pin), false).unwrap();
        assert_eq!(paired, format!("Paired with {HOST_NAME}."));

        // A later run finds it by name, whatever the letter case.
        let client = settings.client();
        let listed = apps(&client, "salon", false).unwrap();
        assert!(listed.contains("Desktop  (HDR)"), "{listed}");
        assert!(listed.contains("Steam Big Picture"), "{listed}");

        let json = apps(&client, HOST_NAME, true).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed[0]["title"], "Desktop");
        assert_eq!(parsed[0]["hdr"], true);

        assert_eq!(forget(&client, HOST_NAME, false).unwrap(), format!("Forgot {HOST_NAME}."));
        assert_eq!(host.paired_clients(), 0);
        assert!(settings.client().hosts().is_empty());
    }

    #[test]
    fn an_unpaired_pc_says_what_to_do() {
        let settings = Settings::new("unpaired");
        let host = FakeHost::start();
        let client = settings.client();

        let added = add(&client, &host.address(), false).unwrap();
        assert_eq!(added, "Added Salon (Sunshine). Pair with `leto-cli pair`.");

        let error = apps(&client, HOST_NAME, false).unwrap_err();
        assert_eq!(error, "this device is not paired with the PC");
    }

    #[test]
    fn a_wrong_pin_is_reported() {
        let settings = Settings::new("wrong-pin");
        let host = FakeHost::start();
        host.type_pin("1111".parse().unwrap());

        let error =
            pair(&settings.client(), &host.address(), "2222".parse().ok(), false).unwrap_err();
        assert_eq!(error, "the PIN did not match");
    }

    #[test]
    fn the_state_column_reads_naturally() {
        let settings = Settings::new("state");
        let host = FakeHost::start();
        let client = settings.client();
        let mut summary = client.add(&host.address()).unwrap();

        assert_eq!(state(&summary), "not paired");
        summary.paired = true;
        assert_eq!(state(&summary), "paired");
        summary.busy = true;
        assert_eq!(state(&summary), "paired, an app is running");
        summary.reachability = Reachability::Offline;
        assert_eq!(state(&summary), "offline");
    }
}
