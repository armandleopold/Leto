//! The web client's files, as the bridge serves them.
//!
//! They are the handful of files `cargo xtask web` builds into one folder.
//! A request names one of them and nothing else: no folders, no hidden
//! files, nothing a path could be bent into.

use std::{
    fs, io,
    path::{Path, PathBuf},
};

/// The page a browser gets when it asks for the folder itself.
const FRONT_PAGE: &str = "index.html";

pub struct Pages {
    folder: PathBuf,
}

pub struct Page {
    pub content_type: &'static str,
    pub body: Vec<u8>,
}

impl Pages {
    pub fn new(folder: &Path) -> Self {
        Self { folder: folder.to_owned() }
    }

    /// Whether the web client is there to serve.
    pub fn complete(&self) -> bool {
        self.folder.join(FRONT_PAGE).is_file()
    }

    /// The file a request path names, read fresh so that a rebuilt web
    /// client is served without restarting the bridge.
    pub fn get(&self, path: &str) -> io::Result<Option<Page>> {
        let Some(name) = file_name(path) else { return Ok(None) };
        let file = self.folder.join(name);
        if !file.is_file() {
            return Ok(None);
        }
        let body = fs::read(file)?;
        Ok(Some(Page { content_type: content_type(name), body }))
    }
}

/// The file a request path names, when it names one the bridge may serve.
fn file_name(path: &str) -> Option<&str> {
    // What follows a question mark is for the page, not for the bridge.
    let path = path.split(['?', '#']).next().unwrap_or(path);
    let name = path.strip_prefix('/')?;
    if name.is_empty() {
        return Some(FRONT_PAGE);
    }
    let plain =
        name.bytes().all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_'));
    (plain && !name.starts_with('.') && !name.contains("..")).then_some(name)
}

fn content_type(name: &str) -> &'static str {
    match name.rsplit_once('.').map(|(_, extension)| extension) {
        Some("html") => "text/html; charset=utf-8",
        Some("js") => "text/javascript; charset=utf-8",
        // Browsers compile WebAssembly as it arrives only under this name.
        Some("wasm") => "application/wasm",
        Some("css") => "text/css; charset=utf-8",
        Some("json" | "webmanifest") => "application/json",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("ico") => "image/x-icon",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn a_request_names_one_file_of_the_folder_and_nothing_else() {
        assert_eq!(file_name("/"), Some("index.html"));
        assert_eq!(file_name("/leto.js"), Some("leto.js"));
        assert_eq!(file_name("/leto_bg.wasm?v=2"), Some("leto_bg.wasm"));
        assert_eq!(file_name("/icon-maskable-192.png"), Some("icon-maskable-192.png"));

        for sneaky in [
            "",
            "leto.js",
            "/../secret",
            "/..",
            "/a/b.js",
            "/a\\b.js",
            "/.hidden",
            "/%2e%2e/secret",
            "/leto..js",
            "//etc/passwd",
            "/C:secret",
        ] {
            assert_eq!(file_name(sneaky), None, "{sneaky}");
        }
    }

    #[test]
    fn files_are_served_with_the_type_a_browser_needs() {
        let folder = std::env::temp_dir().join(format!("leto-pages-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&folder);
        fs::create_dir_all(folder.join("deeper")).unwrap();
        let pages = Pages::new(&folder);
        assert!(!pages.complete());

        fs::write(folder.join("index.html"), "<!doctype html>").unwrap();
        fs::write(folder.join("leto_bg.wasm"), [0, 0x61, 0x73, 0x6D]).unwrap();
        assert!(pages.complete());

        let front = pages.get("/").unwrap().unwrap();
        assert_eq!(front.content_type, "text/html; charset=utf-8");
        assert_eq!(front.body, b"<!doctype html>");
        assert_eq!(pages.get("/leto_bg.wasm").unwrap().unwrap().content_type, "application/wasm");
        assert!(pages.get("/missing.js").unwrap().is_none());
        assert!(pages.get("/deeper").unwrap().is_none());
        let _ = fs::remove_dir_all(&folder);
    }
}
