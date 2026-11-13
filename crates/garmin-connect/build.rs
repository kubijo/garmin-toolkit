use openapi_to_rust::{
    CodeGenerator, GeneratorConfig, SchemaAnalyzer,
    config::{BuildersSection, ClientSection},
};
use std::{env, fs, path::PathBuf};

#[path = "../../infra/garmin-connect/schema.rs"]
mod schema;

fn main() {
    let input = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../infra/garmin-connect");
    let mut analyzer =
        SchemaAnalyzer::new(schema::binary_download(schema::load(&input))).expect("Garmin schema");
    let mut analysis = analyzer.analyze().expect("analyzable schema");
    let out = PathBuf::from(env::var_os("OUT_DIR").expect("Cargo output"));
    let generator = CodeGenerator::new(GeneratorConfig {
        output_dir: out.join("api"),
        enable_sse_client: false,
        enable_async_client: true,
        tracing_enabled: false,
        builders: BuildersSection {
            enabled: true,
            ..BuildersSection::default()
        },
        client: Some(ClientSection {
            operations: vec![
                "GET /userprofile-service/userprofile/settings".into(),
                "GET /activitylist-service/activities/search/activities".into(),
                "GET /activity-service/activity/{activityId}".into(),
                "GET /download-service/files/activity/{activityId}".into(),
            ],
            prune_models: true,
        }),
        ..GeneratorConfig::default()
    });
    let result = generator
        .generate_all(&mut analysis)
        .expect("generate Personal API client");
    generator
        .write_files(&result)
        .expect("write generated Rust");
    let module = out.join("api/mod.rs");
    fs::write(out.join("modules.rs"), indoc::formatdoc! {r#"
        #[allow(dead_code, unused_imports, clippy::all, clippy::pedantic, clippy::allow_attributes, clippy::allow_attributes_without_reason,
            reason = "Upstream generated code; authored transport is linted separately")]
        #[path = {module:?}]
        mod api;
        "#}).expect("module mount");
}
