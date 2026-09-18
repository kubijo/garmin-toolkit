//! Embeds the icon catalog and locates shared test fixtures.

mod icon_build;

macro_rules! icon_catalog {
    (
        phosphor { $($phosphor_name:ident => $phosphor_svg:expr,)* }
        local { $($local_name:ident => $local_svg:expr,)* }
    ) => {
        const ICONS: &[(&str, &str)] = &[
            $((stringify!($phosphor_name), $phosphor_svg),)*
            $((stringify!($local_name), $local_svg),)*
        ];
    };
}

include!("src/icons/catalog.rs");

fn main() {
    let fixtures = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../infra/javascript/fixtures");
    println!(
        "cargo:rustc-env=GARMIN_MAP_FIXTURES_DIR={}",
        fixtures.display()
    );
    println!("cargo:rerun-if-changed=src/icons/catalog.rs");
    icon_build::write_catalog(ICONS).expect("the icon catalog must be valid");
}
