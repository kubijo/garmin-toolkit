use std::{env, error::Error, io, path::PathBuf, process::Command};

fn main() -> Result<(), Box<dyn Error>> {
    let source = "translations/cs.json";
    println!("cargo:rerun-if-changed={source}");
    let output =
        PathBuf::from(env::var_os("OUT_DIR").ok_or("Cargo did not set OUT_DIR")?).join("cs.json");
    let status = Command::new("formatjs")
        .args(["compile", "--format", "lokalise", "--out-file"])
        .arg(output)
        .arg(source)
        .status()
        .map_err(|error| {
            io::Error::new(
                error.kind(),
                format!("could not run FormatJS; use the pinned development shell: {error}"),
            )
        })?;
    if !status.success() {
        return Err(format!("FormatJS catalog compilation failed: {status}").into());
    }
    Ok(())
}
