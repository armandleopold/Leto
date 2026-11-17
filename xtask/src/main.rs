//! Project automation, run with `cargo xtask <command>`.
//!
//! The continuous-integration workflow calls these same commands, so a green
//! `cargo xtask ci` locally means a green pipeline.

use std::{
    env, fs,
    path::{Path, PathBuf},
    process::{Command, ExitCode},
};

/// A target Leto must keep compiling for, and the packages checked on it.
struct Target {
    triple: &'static str,
    /// Packages to check, or an empty slice for the whole workspace.
    packages: &'static [&'static str],
}

/// The target of the web client.
const WEB_TARGET: &str = "wasm32-unknown-unknown";

/// Packages with no platform code: they must compile everywhere.
const PORTABLE: &[&str] = &["leto-core", "leto-protocol"];

/// Every non-native target checked by `cargo xtask targets`.
///
/// The graphical app joins the Android and iOS lists when their app shells
/// land. `cargo check` needs no linker or SDK, so all of these run from Linux.
const TARGETS: &[Target] = &[
    Target { triple: "x86_64-pc-windows-msvc", packages: &[] },
    Target { triple: "aarch64-apple-darwin", packages: &[] },
    Target { triple: WEB_TARGET, packages: &["leto", "leto-core", "leto-protocol"] },
    Target { triple: "aarch64-linux-android", packages: PORTABLE },
    Target { triple: "aarch64-apple-ios", packages: PORTABLE },
];

const USAGE: &str = "\
Usage: cargo xtask <command>

Commands:
  ci        Run every check the pipeline runs: fmt, clippy, test, targets
  fmt       Check formatting
  clippy    Lint the workspace, warnings are errors
  test      Run the tests
  targets   Check that the code compiles for every supported target
  web       Build the web client into dist/web
";

fn main() -> ExitCode {
    let command = env::args().nth(1).unwrap_or_default();
    let result = match command.as_str() {
        "ci" => fmt().and_then(|()| clippy()).and_then(|()| test()).and_then(|()| targets()),
        "fmt" => fmt(),
        "clippy" => clippy(),
        "test" => test(),
        "targets" => targets(),
        "web" => web(),
        _ => {
            eprint!("{USAGE}");
            return ExitCode::FAILURE;
        }
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(step) => {
            eprintln!("xtask: `{step}` failed");
            ExitCode::FAILURE
        }
    }
}

fn fmt() -> Result<(), String> {
    cargo(&["fmt", "--all", "--check"])
}

fn clippy() -> Result<(), String> {
    cargo(&["clippy", "--workspace", "--all-targets", "--locked", "--", "-D", "warnings"])
}

fn test() -> Result<(), String> {
    cargo(&["test", "--workspace", "--locked"])
}

fn targets() -> Result<(), String> {
    for target in TARGETS {
        let mut args = vec!["check", "--locked", "--target", target.triple];
        if target.packages.is_empty() {
            args.push("--workspace");
        }
        for package in target.packages {
            args.extend(["--package", package]);
        }
        cargo(&args)?;
    }
    Ok(())
}

/// Builds the web client: the app compiled to WebAssembly, its JavaScript
/// loader, the page that hosts it and the icons, all in `dist/web`.
fn web() -> Result<(), String> {
    let root = workspace_root();
    let out = root.join("dist/web");

    let wanted = locked_version(&root, "wasm-bindgen")?;
    let install = format!("cargo install wasm-bindgen-cli --version {wanted} --locked");
    match installed_wasm_bindgen() {
        Some(found) if found == wanted => {}
        Some(found) => {
            return Err(format!(
                "wasm-bindgen {found} is installed but Cargo.lock needs {wanted}. Run: {install}"
            ));
        }
        None => return Err(format!("wasm-bindgen is not installed. Run: {install}")),
    }

    cargo(&["build", "--locked", "--release", "--target", WEB_TARGET, "--package", "leto"])?;

    if out.exists() {
        fs::remove_dir_all(&out).map_err(|e| format!("clearing {}: {e}", out.display()))?;
    }
    let wasm = root.join("target").join(WEB_TARGET).join("release/leto.wasm");
    let line = "wasm-bindgen --target web --no-typescript";
    eprintln!("xtask: {line}");
    let status = Command::new("wasm-bindgen")
        .args(["--target", "web", "--no-typescript", "--out-dir"])
        .arg(&out)
        .arg(&wasm)
        .status()
        .map_err(|e| format!("{line}: {e}"))?;
    if !status.success() {
        return Err(line.to_owned());
    }

    copy_files(&root.join("apps/leto/web"), &out)?;
    copy_files(&root.join("assets/brand/web"), &out)?;

    eprintln!("xtask: web client ready in {}", out.display());
    eprintln!("xtask: try it with `python3 -m http.server --directory dist/web`");
    Ok(())
}

/// The folder that holds the workspace `Cargo.toml`.
fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask lives one level below the workspace root")
        .to_path_buf()
}

/// Version of `package` pinned in `Cargo.lock`.
fn locked_version(root: &Path, package: &str) -> Result<String, String> {
    let lock =
        fs::read_to_string(root.join("Cargo.lock")).map_err(|e| format!("Cargo.lock: {e}"))?;
    let name_line = format!("name = \"{package}\"");
    let mut lines = lock.lines();
    while let Some(line) = lines.next() {
        if line == name_line {
            let version = lines.next().and_then(|next| next.strip_prefix("version = \""));
            if let Some(version) = version.and_then(|rest| rest.strip_suffix('"')) {
                return Ok(version.to_owned());
            }
        }
    }
    Err(format!("{package} is not in Cargo.lock"))
}

/// Version of the `wasm-bindgen` command on the `PATH`, if there is one.
fn installed_wasm_bindgen() -> Option<String> {
    let output = Command::new("wasm-bindgen").arg("--version").output().ok()?;
    // Prints "wasm-bindgen 0.2.100".
    String::from_utf8(output.stdout).ok()?.split_whitespace().nth(1).map(str::to_owned)
}

/// Copies the files directly inside `from` into `to`.
fn copy_files(from: &Path, to: &Path) -> Result<(), String> {
    let failed = |e: std::io::Error| format!("copying {}: {e}", from.display());
    for entry in fs::read_dir(from).map_err(failed)? {
        let path = entry.map_err(failed)?.path();
        if let (true, Some(name)) = (path.is_file(), path.file_name()) {
            fs::copy(&path, to.join(name)).map_err(failed)?;
        }
    }
    Ok(())
}

/// Runs `cargo` with `args`, returning the command line on failure.
fn cargo(args: &[&str]) -> Result<(), String> {
    let cargo = env::var("CARGO").unwrap_or_else(|_| "cargo".to_owned());
    let line = format!("cargo {}", args.join(" "));
    eprintln!("xtask: {line}");
    let status = Command::new(cargo).args(args).status().map_err(|e| format!("{line}: {e}"))?;
    if status.success() { Ok(()) } else { Err(line) }
}
