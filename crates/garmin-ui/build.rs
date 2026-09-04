//! Validates and embeds the icon catalog.

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
    println!("cargo:rerun-if-changed=src/icons/catalog.rs");
    icon_build::write_catalog(ICONS).expect("the icon catalog must be valid");
}
