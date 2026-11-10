fn main() -> Result<(), Box<dyn std::error::Error>> {
    for file in [
        "proto/toolkit_file_access.proto",
        "proto/garmin/gdi_file_access.proto",
        "proto/garmin/gdi_data_types.proto",
    ] {
        println!("cargo:rerun-if-changed={file}");
    }
    buffa_build::Config::new()
        .files(&[
            "proto/toolkit_file_access.proto",
            "proto/garmin/gdi_file_access.proto",
            "proto/garmin/gdi_data_types.proto",
        ])
        .includes(&["proto"])
        .include_file("_generated.rs")
        .compile()?;
    Ok(())
}
