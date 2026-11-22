//! The commands about Leto's own WireGuard tunnel: show it, give it a
//! file, take it away.

use std::{fs, io::Read, path::Path};

use leto_client::{Client, TunnelSummary};

use crate::to_json;

/// The largest file taken as a WireGuard file. They are a few hundred
/// bytes.
const LARGEST_FILE: u64 = 64 * 1024;

pub fn show(client: &Client, json: bool) -> Result<String, String> {
    let tunnel = client.tunnel();
    if json {
        return to_json(&tunnel);
    }
    Ok(match tunnel {
        Some(tunnel) => said(&tunnel),
        None => "Leto has no WireGuard tunnel. Give it one with `leto-cli tunnel use <file>`."
            .to_owned(),
    })
}

/// Uses the WireGuard file at `file`, or the one typed in when it is `-`.
pub fn use_file(client: &Client, file: &Path, json: bool) -> Result<String, String> {
    // The text holds a private key: it is never printed, in an error
    // either.
    let text = read(file)?;
    let tunnel = client.use_tunnel(&text).map_err(|error| error.to_string())?;
    if json {
        return to_json(&tunnel);
    }
    Ok(format!("Leto uses this tunnel from now on.\n{}", said(&tunnel)))
}

pub fn remove(client: &Client, json: bool) -> Result<String, String> {
    client.remove_tunnel().map_err(|error| error.to_string())?;
    if json {
        return to_json(&client.tunnel());
    }
    Ok("Leto has no WireGuard tunnel any more.".to_owned())
}

fn read(file: &Path) -> Result<String, String> {
    let mut text = String::new();
    let read = if file == Path::new("-") {
        std::io::stdin().take(LARGEST_FILE + 1).read_to_string(&mut text)
    } else {
        fs::File::open(file).and_then(|file| file.take(LARGEST_FILE + 1).read_to_string(&mut text))
    };
    match read {
        Ok(_) if text.len() as u64 <= LARGEST_FILE => Ok(text),
        Ok(_) => Err(format!("{} is too large to be a WireGuard file", file.display())),
        Err(error) => Err(format!("{} could not be read: {error}", file.display())),
    }
}

/// The tunnel in plain words. Its public key is no secret: the other end
/// must be told it.
fn said(tunnel: &TunnelSummary) -> String {
    let mut lines = Vec::new();
    if !tunnel.address.is_empty() {
        lines.push(format!("This device:  {}", tunnel.address));
        lines.push(format!("Leads to:     {}", tunnel.covers.join(", ")));
        let end = tunnel.endpoint.as_deref().unwrap_or("comes to this device");
        lines.push(format!("Other end:    {end}"));
        lines.push(format!("Public key:   {}", tunnel.public_key));
    }
    let state = match (&tunnel.problem, tunnel.running, tunnel.handshake) {
        (Some(problem), ..) => format!("does not work: {problem}"),
        (None, true, Some(ago)) => format!("open, the other end answered {ago} s ago"),
        (None, true, None) => match &tunnel.silence {
            Some(why) => format!("does not open: {why}"),
            None => "open, the other end has not answered yet".to_owned(),
        },
        (None, false, _) => "opens when a PC it leads to is looked for".to_owned(),
    };
    lines.push(format!("State:        {state}"));
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use std::path::PathBuf;

    use super::*;

    /// A private key made of ones, and a public key made of twos: no real
    /// tunnel.
    const FILE: &str = "[Interface]\nPrivateKey = AQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQE=\n\
                        Address = 10.8.0.4/24\n\n[Peer]\nPublicKey = \
                        AgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgI=\nEndpoint = \
                        192.0.2.7:51820\nAllowedIPs = 10.8.0.0/24\n";

    /// A folder of its own for the settings and the file, removed
    /// afterwards.
    struct Folder(PathBuf);

    impl Folder {
        fn new(test: &str) -> Self {
            let path = std::env::temp_dir().join(format!("leto-cli-{}-{test}", std::process::id()));
            let _ = fs::remove_dir_all(&path);
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }

        fn client(&self) -> Client {
            Client::open(&self.0.join("settings")).unwrap()
        }

        fn file(&self, text: &str) -> PathBuf {
            let path = self.0.join("home.conf");
            fs::write(&path, text).unwrap();
            path
        }
    }

    impl Drop for Folder {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn a_tunnel_is_given_shown_kept_and_removed_and_its_key_is_never_printed() {
        let folder = Folder::new("tunnel");
        let none = show(&folder.client(), false).unwrap();
        assert!(none.starts_with("Leto has no WireGuard tunnel."), "{none}");

        let used = use_file(&folder.client(), &folder.file(FILE), false).unwrap();
        assert!(used.contains("This device:  10.8.0.4"), "{used}");
        assert!(used.contains("Leads to:     10.8.0.0/24"), "{used}");
        assert!(used.contains("Other end:    192.0.2.7:51820"), "{used}");
        assert!(!used.contains("AQEBAQEB"), "{used}");

        // A later run still has it.
        let client = folder.client();
        let shown = show(&client, false).unwrap();
        assert!(shown.contains("opens when a PC it leads to is looked for"), "{shown}");
        let json = show(&client, true).unwrap();
        assert!(!json.contains("AQEBAQEB"), "{json}");
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["address"], "10.8.0.4");

        assert_eq!(remove(&client, false).unwrap(), "Leto has no WireGuard tunnel any more.");
        assert_eq!(folder.client().tunnel(), None);
    }

    #[test]
    fn a_file_that_is_no_wireguard_file_is_refused_without_being_printed() {
        let folder = Folder::new("tunnel-refused");
        let client = folder.client();

        let error = use_file(&client, &folder.file("PrivateKey = secret"), false).unwrap_err();
        assert!(!error.contains("secret"), "{error}");
        assert_eq!(client.tunnel(), None);

        let missing = use_file(&client, &folder.0.join("nowhere.conf"), false).unwrap_err();
        assert!(missing.contains("could not be read"), "{missing}");

        let large = "#".repeat(usize::try_from(LARGEST_FILE).unwrap() + 1);
        let error = use_file(&client, &folder.file(&large), false).unwrap_err();
        assert!(error.ends_with("is too large to be a WireGuard file"), "{error}");
    }
}
