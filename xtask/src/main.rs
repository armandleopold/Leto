//! Project automation, run with `cargo xtask <command>`.
//!
//! The continuous-integration workflow calls these same commands, so a green
//! `cargo xtask ci` locally means a green pipeline.

use std::{
    env, fs, io,
    path::{Path, PathBuf},
    process::{Command, ExitCode, Stdio},
};

/// A target Leto must keep compiling for, and the packages checked on it.
struct Target {
    triple: &'static str,
    /// Packages to check, or an empty slice for the whole workspace.
    packages: &'static [&'static str],
    /// Name zig gives this target, when the check has C or C++ code to
    /// compile.
    zig: Option<&'static str>,
}

/// The target of the web client.
const WEB_TARGET: &str = "wasm32-unknown-unknown";

/// Packages with no platform code: they must compile everywhere.
const PORTABLE: &[&str] = &["leto-core", "leto-protocol"];

/// Every non-native target checked by `cargo xtask targets`.
///
/// `cargo check` needs no linker or SDK, so these run from any machine. The
/// one thing it does need is a C and a C++ compiler for the target: the TLS
/// code under `leto-client` (the `ring` crate) is partly written in C, and
/// the H.264 codec under `leto-media` (OpenH264) in C++. zig provides both
/// for macOS. Windows has no such shortcut, so its build is covered by the
/// Windows job of the pipeline instead.
///
/// The graphical app joins the Android and iOS lists when their app shells
/// land.
const TARGETS: &[Target] = &[
    Target { triple: "aarch64-apple-darwin", packages: &[], zig: Some("aarch64-macos") },
    Target { triple: WEB_TARGET, packages: &["leto", "leto-core", "leto-protocol"], zig: None },
    Target { triple: "aarch64-linux-android", packages: PORTABLE, zig: None },
    Target { triple: "aarch64-apple-ios", packages: PORTABLE, zig: None },
];

/// A copy of this program under a name starting with one of these acts as a C
/// compiler, a C++ compiler or an archiver that hands the work to zig. See
/// [`cross_tool`].
const ZIG_CC: &str = "leto-zig-cc-";
const ZIG_CXX: &str = "leto-zig-cxx-";
const ZIG_AR: &str = "leto-zig-ar";

/// The ways zig may be installed: on its own, or as the `ziglang` Python
/// package, which is how the pipeline gets it.
const ZIG_COMMANDS: &[&[&str]] =
    &[&["zig"], &["python-zig"], &["python3", "-m", "ziglang"], &["python", "-m", "ziglang"]];

const ZIG_HELP: &str = "install zig with `pipx install ziglang`, or from https://ziglang.org";

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
    if let Some(code) = cross_tool() {
        return code;
    }
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
        // A Mac compiles for macOS with its own compiler.
        let zig = target.zig.filter(|_| !cfg!(target_os = "macos"));
        let Some(zig) = zig else {
            cargo(&args)?;
            continue;
        };
        if zig_command().is_none() {
            // The pipeline must not skip a check. A developer machine may.
            if env::var_os("CI").is_some() {
                return Err(format!("the {} check needs zig: {ZIG_HELP}", target.triple));
            }
            eprintln!("xtask: skipping {}: {ZIG_HELP}", target.triple);
            continue;
        }
        cargo_with(&args, &cross_tools(target.triple, zig)?)?;
    }
    Ok(())
}

/// Prepares the compilers and the archiver for `triple`, and returns the
/// environment variables that make the `cc` crate use them.
///
/// Both are copies of this program under another name: see [`cross_tool`].
fn cross_tools(triple: &str, zig: &str) -> Result<Vec<(String, PathBuf)>, String> {
    let exe = env::current_exe().map_err(|e| format!("finding xtask itself: {e}"))?;
    let dir = workspace_root().join("target/xtask");
    fs::create_dir_all(&dir).map_err(|e| format!("creating {}: {e}", dir.display()))?;
    let install = |name: String| {
        let tool = dir.join(name).with_extension(env::consts::EXE_EXTENSION);
        fs::copy(&exe, &tool).map_err(|e| format!("creating {}: {e}", tool.display()))?;
        Ok::<_, String>(tool)
    };
    let suffix = triple.replace('-', "_");
    Ok(vec![
        (format!("CC_{suffix}"), install(format!("{ZIG_CC}{zig}"))?),
        (format!("CXX_{suffix}"), install(format!("{ZIG_CXX}{zig}"))?),
        (format!("AR_{suffix}"), install(ZIG_AR.to_owned())?),
    ])
}

/// When this program runs under one of the tool names, does that tool's job
/// and returns its exit code.
///
/// The `cc` crate takes a compiler as one path, with no room for arguments,
/// so the zig target is part of the file name.
fn cross_tool() -> Option<ExitCode> {
    let exe = env::current_exe().ok()?;
    let name = exe.file_stem()?.to_str()?;
    let mut args = vec![];
    let compiler = [(ZIG_CC, "cc"), (ZIG_CXX, "c++")]
        .into_iter()
        .find_map(|(prefix, language)| Some((language, name.strip_prefix(prefix)?)));
    if let Some((language, target)) = compiler {
        args.extend([language.to_owned(), "-target".to_owned(), target.to_owned()]);
        // Flags the `cc` crate adds for Apple's own compiler. zig takes the
        // target from `-target` and rejects these.
        let mut given = env::args().skip(1);
        while let Some(arg) = given.next() {
            if arg == "-arch" {
                given.next();
            } else if !arg.starts_with("--target=") {
                args.push(arg);
            }
        }
    } else if name == ZIG_AR {
        args.push("ar".to_owned());
        args.extend(env::args().skip(1));
    } else {
        return None;
    }
    let status = zig_command()
        .ok_or_else(|| io::Error::other(ZIG_HELP))
        .and_then(|zig| Command::new(zig[0]).args(&zig[1..]).args(&args).status());
    Some(match status {
        Ok(status) if status.success() => ExitCode::SUCCESS,
        Ok(_) => ExitCode::FAILURE,
        Err(e) => {
            eprintln!("{name}: {e}");
            ExitCode::FAILURE
        }
    })
}

/// The command that starts zig on this machine, if it is installed.
fn zig_command() -> Option<&'static [&'static str]> {
    ZIG_COMMANDS.iter().copied().find(|zig| {
        Command::new(zig[0])
            .args(&zig[1..])
            .arg("version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
    })
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
    eprintln!("xtask: serve it with `cargo run -p leto-cli -- bridge`");
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
    cargo_with(args, &[])
}

/// Runs `cargo` with `args` and extra environment variables.
fn cargo_with(args: &[&str], variables: &[(String, PathBuf)]) -> Result<(), String> {
    let cargo = env::var("CARGO").unwrap_or_else(|_| "cargo".to_owned());
    let line = format!("cargo {}", args.join(" "));
    eprintln!("xtask: {line}");
    let status = Command::new(cargo)
        .args(args)
        .envs(variables.iter().map(|(name, value)| (name, value)))
        .status()
        .map_err(|e| format!("{line}: {e}"))?;
    if status.success() { Ok(()) } else { Err(line) }
}
