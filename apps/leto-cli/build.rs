//! Gives `leto-cli.exe` its name and version, as `leto.exe` has: Windows
//! shows them in the file's properties, and code signing checks them.

use std::{env, error::Error, fs, path::Path};

/// The app's name, as `leto_core::brand::NAME` says it.
const NAME: &str = "Leto";

fn main() -> Result<(), Box<dyn Error>> {
    println!("cargo:rerun-if-changed=build.rs");
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return Ok(());
    }

    let version = env::var("CARGO_PKG_VERSION")?;
    let numbers = ["MAJOR", "MINOR", "PATCH"]
        .map(|part| env::var(format!("CARGO_PKG_VERSION_{part}")))
        .into_iter()
        .collect::<Result<Vec<_>, _>>()?
        .join(",");
    let description = format!(
        r#"// Made by build.rs.
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
      VALUE "FileDescription", "{NAME} command line"
      VALUE "FileVersion", "{version}"
      VALUE "InternalName", "leto-cli"
      VALUE "OriginalFilename", "leto-cli.exe"
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
    let script = Path::new(&env::var("OUT_DIR")?).join("leto-cli.rc");
    fs::write(&script, description)?;
    embed_resource::compile(&script, embed_resource::NONE)
        .manifest_optional()
        .map_err(|failure| failure.to_string())?;
    Ok(())
}
