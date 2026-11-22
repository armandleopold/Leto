//! `cargo xtask android`: the Android app as an APK, in `dist/android`.
//!
//! The app is the Rust library `apps/leto-android` in a NativeActivity, so
//! there is no Java to compile and no Gradle: the NDK builds the library,
//! and the SDK's build tools (`aapt2`, `zipalign`, `apksigner`) wrap it.
//!
//! Needs the Android SDK (`ANDROID_HOME`) with build tools, a platform and
//! the NDK, Java for `apksigner`, and `zip`. GitHub's Ubuntu runners have
//! them all.

use std::{
    env, fs,
    path::{Path, PathBuf},
    process::Command,
};

use crate::{cargo_with, workspace_root};

/// What the app is built for, each in the APK: Android takes the first its
/// processor runs.
const TARGETS: [Target; 2] = [
    // Nearly every Android phone sold since 2017.
    Target { triple: "aarch64-linux-android", abi: "arm64-v8a", clang: "aarch64-linux-android" },
    // Televisions and their sticks, as most Fire TVs: 64-bit processors
    // running a 32-bit Android.
    Target {
        triple: "armv7-linux-androideabi",
        abi: "armeabi-v7a",
        clang: "armv7a-linux-androideabi",
    },
];

/// The target the Android code is linted for.
const LINTED: &Target = &TARGETS[0];

/// One processor the app is built for.
struct Target {
    /// Rust's name for it.
    triple: &'static str,
    /// Where its libraries go inside an APK.
    abi: &'static str,
    /// The start of the NDK's compilers for it, before the Android version.
    clang: &'static str,
}

impl Target {
    /// The NDK's folder of libraries for it, which differs from Rust's name
    /// on 32-bit ARM.
    fn sysroot(&self) -> &'static str {
        if self.abi == "armeabi-v7a" { "arm-linux-androideabi" } else { self.triple }
    }
}
/// The oldest Android the app runs on: 8.0, the first with AAudio, which
/// the sound plays through.
const MIN_SDK: u32 = 26;
/// The Android version the app is tested against.
const TARGET_SDK: u32 = 35;

const SDK_HELP: &str =
    "set ANDROID_HOME to an Android SDK with build tools, a platform and the NDK";

/// The folders of the Android SDK and NDK on this machine.
pub struct Sdk {
    home: PathBuf,
    ndk: PathBuf,
}

impl Sdk {
    /// Finds the SDK from `ANDROID_HOME` (or `ANDROID_SDK_ROOT`), and the
    /// NDK from `ANDROID_NDK_HOME` or the newest one inside the SDK.
    pub fn find() -> Result<Self, String> {
        let home = env::var_os("ANDROID_HOME")
            .or_else(|| env::var_os("ANDROID_SDK_ROOT"))
            .map(PathBuf::from)
            .filter(|path| path.is_dir())
            .ok_or_else(|| SDK_HELP.to_owned())?;
        let ndk = env::var_os("ANDROID_NDK_HOME")
            .or_else(|| env::var_os("ANDROID_NDK_ROOT"))
            .map(PathBuf::from)
            .filter(|path| path.is_dir())
            .or_else(|| newest(&home.join("ndk")))
            .ok_or_else(|| format!("no Android NDK found: {SDK_HELP}"))?;
        Ok(Self { home, ndk })
    }

    /// The NDK's compilers for `target`, as the environment variables the
    /// `cc` crate and Cargo read.
    fn compilers(&self, target: &Target) -> Vec<(String, PathBuf)> {
        let clang = self.tool(&format!("{}{MIN_SDK}-clang", target.clang));
        let suffix = target.triple.replace('-', "_");
        vec![
            (format!("CC_{suffix}"), clang.clone()),
            (format!("CXX_{suffix}"), self.tool(&format!("{}{MIN_SDK}-clang++", target.clang))),
            (format!("AR_{suffix}"), self.tool("llvm-ar")),
            (format!("CARGO_TARGET_{}_LINKER", suffix.to_uppercase()), clang),
        ]
    }

    /// One of the NDK's own tools, as `llvm-strip`.
    fn tool(&self, name: &str) -> PathBuf {
        self.ndk.join("toolchains/llvm/prebuilt").join(host_tag()).join("bin").join(name)
    }

    /// The C++ library the H.264 codec needs beside the app.
    fn cpp_library(&self, target: &Target) -> PathBuf {
        self.ndk
            .join("toolchains/llvm/prebuilt")
            .join(host_tag())
            .join("sysroot/usr/lib")
            .join(target.sysroot())
            .join("libc++_shared.so")
    }

    fn build_tool(&self, name: &str) -> Result<PathBuf, String> {
        let tools = newest(&self.home.join("build-tools"))
            .ok_or_else(|| format!("no build tools in the Android SDK: {SDK_HELP}"))?;
        Ok(tools.join(name))
    }

    fn platform(&self) -> Result<PathBuf, String> {
        let platform = newest(&self.home.join("platforms"))
            .ok_or_else(|| format!("no platform in the Android SDK: {SDK_HELP}"))?;
        Ok(platform.join("android.jar"))
    }
}

/// Builds `dist/android/leto.apk`.
pub fn apk() -> Result<(), String> {
    let sdk = Sdk::find()?;
    let root = workspace_root();
    let work = root.join("target/android");
    let staging = work.join("apk");
    let out = root.join("dist/android");
    for folder in [&staging, &out] {
        if folder.exists() {
            fs::remove_dir_all(folder)
                .map_err(|e| format!("clearing {}: {e}", folder.display()))?;
        }
        fs::create_dir_all(folder).map_err(|e| format!("creating {}: {e}", folder.display()))?;
    }

    // The pipeline's lints, for the code only Android compiles.
    cargo_with(
        &[
            "clippy",
            "--locked",
            "--release",
            "--target",
            LINTED.triple,
            "--package",
            "leto-android",
        ]
        .into_iter()
        .chain(["--package", "leto", "--package", "leto-system", "--package", "leto-media"])
        .chain(["--", "-D", "warnings"])
        .collect::<Vec<_>>(),
        &sdk.compilers(LINTED),
    )?;
    for target in &TARGETS {
        cargo_with(
            &["build", "--locked", "--release", "--target", target.triple]
                .into_iter()
                .chain(["--package", "leto-android"])
                .collect::<Vec<_>>(),
            &sdk.compilers(target),
        )?;

        // The libraries, where Android looks for them.
        let libraries = staging.join("lib").join(target.abi);
        fs::create_dir_all(&libraries)
            .map_err(|e| format!("creating {}: {e}", libraries.display()))?;
        let built = root.join("target").join(target.triple).join("release/libleto_android.so");
        for library in [built, sdk.cpp_library(target)] {
            let name = library.file_name().ok_or("a library without a name")?;
            let copy = libraries.join(name);
            fs::copy(&library, &copy).map_err(|e| format!("copying {}: {e}", library.display()))?;
            // The NDK's C++ library comes with its symbols: half its size.
            // Leto's keeps the names of its functions, which a panic's
            // backtrace in the log is read with.
            let keep =
                if name == "libleto_android.so" { "--strip-debug" } else { "--strip-unneeded" };
            run(Command::new(sdk.tool("llvm-strip")).arg(keep).arg(&copy))?;
        }
    }

    // The manifest and the icon.
    let resources = work.join("resources.zip");
    run(Command::new(sdk.build_tool("aapt2")?)
        .args(["compile", "--dir"])
        .arg(root.join("assets/brand/android/res"))
        .arg("-o")
        .arg(&resources))?;
    let (name, code) = version()?;
    let unsigned = work.join("unsigned.apk");
    run(Command::new(sdk.build_tool("aapt2")?)
        .arg("link")
        .arg("-o")
        .arg(&unsigned)
        .arg("-I")
        .arg(sdk.platform()?)
        .arg("--manifest")
        .arg(root.join("apps/leto-android/android/AndroidManifest.xml"))
        .args(["--min-sdk-version", &MIN_SDK.to_string()])
        .args(["--target-sdk-version", &TARGET_SDK.to_string()])
        .args(["--version-name", &name, "--version-code", &code.to_string()])
        .arg(&resources))?;
    // The libraries stay uncompressed, so Android maps them from the APK
    // as they are (`extractNativeLibs="false"` in the manifest).
    run(Command::new("zip")
        .current_dir(&staging)
        .args(["-0", "-r", "-q"])
        .arg(&unsigned)
        .arg("lib"))?;

    // Libraries on 16 KB pages, as Android 15 asks.
    let aligned = work.join("aligned.apk");
    run(Command::new(sdk.build_tool("zipalign")?)
        .args(["-f", "-P", "16", "4"])
        .arg(&unsigned)
        .arg(&aligned))?;

    let apk = out.join("leto.apk");
    let (keystore, password) = keystore(&work)?;
    run(Command::new(sdk.build_tool("apksigner")?)
        .arg("sign")
        // No `.idsig` beside the APK: that is for installs streamed by adb.
        .args(["--v4-signing-enabled", "false"])
        .arg("--ks")
        .arg(&keystore)
        .args(["--ks-pass", &format!("env:{PASSWORD}")])
        .env(PASSWORD, password)
        .arg("--out")
        .arg(&apk)
        .arg(&aligned))?;

    eprintln!("xtask: Android app ready in {}", apk.display());
    eprintln!("xtask: install it with `adb install -r {}`", apk.display());
    Ok(())
}

/// Where `apksigner` reads the keystore's password.
const PASSWORD: &str = "LETO_ANDROID_KEY_PASSWORD";

/// The key the APK is signed with: the keystore in `LETO_ANDROID_KEYSTORE`
/// with its password in `LETO_ANDROID_KEY_PASSWORD`, or else one made for
/// this machine. A phone only installs an update signed with the same key
/// as the version it has, so test builds meant for one phone keep one key.
fn keystore(work: &Path) -> Result<(PathBuf, String), String> {
    if let Some(path) = env::var_os("LETO_ANDROID_KEYSTORE") {
        let password = env::var(PASSWORD)
            .map_err(|_| format!("LETO_ANDROID_KEYSTORE is set but not {PASSWORD}"))?;
        return Ok((PathBuf::from(path), password));
    }
    let path = work.join("test.keystore");
    let password = "leto-test".to_owned();
    if !path.exists() {
        eprintln!("xtask: signing with a test key made for this machine, in {}", path.display());
        run(Command::new("keytool")
            .args(["-genkeypair", "-keystore"])
            .arg(&path)
            .args(["-storepass", &password, "-alias", "leto", "-keyalg", "RSA", "-keysize"])
            .args(["4096", "-validity", "10000", "-dname", "CN=Leto test"]))?;
    }
    Ok((path, password))
}

/// The version name and number Android shows and compares: 0.3.12 is
/// number 312.
fn version() -> Result<(String, u32), String> {
    let name = env!("CARGO_PKG_VERSION").to_owned();
    let parts: Vec<u32> =
        name.split('.').map(|part| part.parse().unwrap_or(0)).chain([0, 0, 0]).take(3).collect();
    let code = (parts[0] * 10_000 + parts[1] * 100 + parts[2]).max(1);
    Ok((name, code))
}

/// The newest of the versioned folders inside `folder`, as `36.1.0` or
/// `android-36`.
fn newest(folder: &Path) -> Option<PathBuf> {
    let key = |path: &PathBuf| -> Vec<u32> {
        let name = path.file_name().and_then(|name| name.to_str()).unwrap_or_default();
        name.split(|c: char| !c.is_ascii_digit()).filter_map(|n| n.parse().ok()).collect()
    };
    fs::read_dir(folder)
        .ok()?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.is_dir())
        .max_by_key(key)
}

/// The NDK's name for the machine it runs on.
fn host_tag() -> &'static str {
    if cfg!(target_os = "macos") {
        "darwin-x86_64"
    } else if cfg!(windows) {
        "windows-x86_64"
    } else {
        "linux-x86_64"
    }
}

fn run(command: &mut Command) -> Result<(), String> {
    let line = format!("{command:?}");
    eprintln!("xtask: {line}");
    let status = command.status().map_err(|e| format!("{line}: {e}"))?;
    if status.success() { Ok(()) } else { Err(line) }
}
