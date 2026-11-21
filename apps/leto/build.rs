//! Gives `leto.exe` its icon and its name: Windows shows them in Explorer,
//! in its search and in the Task Manager, before the app has run. The
//! window's own icon is set as the app starts (`src/main.rs`).

use std::{env, error::Error, fs, path::Path};

/// The app's name, as `leto_core::brand::NAME` says it.
const NAME: &str = "Leto";

fn main() -> Result<(), Box<dyn Error>> {
    println!("cargo:rerun-if-changed=build.rs");
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return Ok(());
    }

    let icon =
        Path::new(&env::var("CARGO_MANIFEST_DIR")?).join("../../assets/brand/windows/leto.ico");
    println!("cargo:rerun-if-changed={}", icon.display());
    // A resource compiler reads `\` in a string as the start of an escape.
    let icon = icon.to_str().ok_or("the icon's path is not text")?.replace('\\', "/");

    let version = env::var("CARGO_PKG_VERSION")?;
    let numbers = ["MAJOR", "MINOR", "PATCH"]
        .map(|part| env::var(format!("CARGO_PKG_VERSION_{part}")))
        .into_iter()
        .collect::<Result<Vec<_>, _>>()?
        .join(",");
    let description = format!(
        r#"// Made by build.rs.
1 ICON "{icon}"

1 VERSIONINFO
FILEVERSION {numbers},0
PRODUCTVERSION {numbers},0
FILEOS 0x40004
FILETYPE 0x1
BEGIN
  BLOCK "StringFileInfo"
  BEGIN
    BLOCK "040904B0"
    BEGIN
      VALUE "FileDescription", "{NAME}"
      VALUE "FileVersion", "{version}"
      VALUE "InternalName", "leto"
      VALUE "OriginalFilename", "leto.exe"
      VALUE "ProductName", "{NAME}"
      VALUE "ProductVersion", "{version}"
    END
  END
  BLOCK "VarFileInfo"
  BEGIN
    VALUE "Translation", 0x409, 1200
  END
END
"#
    );
    let script = Path::new(&env::var("OUT_DIR")?).join("leto.rc");
    fs::write(&script, description)?;
    // Without the icon Windows would show a blank one: no compiler for it
    // stops the build rather than leave it out.
    embed_resource::compile(&script, embed_resource::NONE)
        .manifest_required()
        .map_err(|failure| failure.to_string())?;
    Ok(())
}
