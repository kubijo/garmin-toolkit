//! Rebuilds embedded migrations when their directory changes.

fn main() {
    println!("cargo:rerun-if-changed=migrations");
}
