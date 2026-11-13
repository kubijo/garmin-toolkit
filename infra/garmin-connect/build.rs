use openapi_to_rust::{CodeGenerator, GeneratorConfig, SchemaAnalyzer};
use serde_json::Value;
use std::{env, fs, path::PathBuf};
mod schema;

fn generate(spec: Value, name: &str) {
    let mut analyzer = SchemaAnalyzer::new(spec).expect("valid OpenAPI document");
    let mut analysis = analyzer.analyze().expect("analyzable Garmin descriptor");
    let generator = CodeGenerator::new(GeneratorConfig {
        output_dir: PathBuf::from(env::var_os("OUT_DIR").unwrap()).join(name),
        module_name: name.into(),
        enable_sse_client: false,
        enable_async_client: true,
        tracing_enabled: false,
        ..GeneratorConfig::default()
    });
    let result = generator
        .generate_all(&mut analysis)
        .expect("generate full Garmin API");
    generator
        .write_files(&result)
        .expect("write Cargo build outputs");
}

fn main() {
    let spec = schema::load(std::path::Path::new(env!("CARGO_MANIFEST_DIR")));
    generate(spec.clone(), "generated");

    // The downloadable snapshot uses `default`; current official docs specify `200`.
    // Keep this correction separate from the byte-exact upstream artifact.
    generate(schema::binary_download(spec), "binary");
    let out = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    fs::write(
        out.join("modules.rs"),
        format!(
            "#[path = {:?}]\npub mod generated;\n#[path = {:?}]\npub mod binary;\n",
            out.join("generated/mod.rs"),
            out.join("binary/mod.rs"),
        ),
    )
    .unwrap();
}
