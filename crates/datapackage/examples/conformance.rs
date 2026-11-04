//! Machine-readable adapter used by the crate's independent Python compatibility tests.

use std::path::PathBuf;

use clap::Parser;
use datapackage::{Descriptor, Limits, Package, Report, Writer};
use serde::Serialize;

#[derive(Parser)]
struct Args {
    descriptor: PathBuf,
    #[arg(long)]
    roundtrip: Option<PathBuf>,
}

#[derive(Serialize)]
struct Outcome {
    valid: bool,
    #[serde(flatten)]
    report: Report,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let arguments = Args::parse();
    let result = Package::read_directory(&arguments.descriptor, Limits::default());
    let outcome = match result {
        Ok(package) => {
            let report = package.validate();
            if report.valid()
                && let Some(path) = arguments.roundtrip
            {
                let mut writer = Writer::new(std::fs::File::create(path)?);
                for (index, resource) in package.resources.into_iter().enumerate() {
                    let mut descriptor = resource.descriptor;
                    descriptor.path = Some(format!("data/{index}.csv"));
                    descriptor.data = None;
                    writer.add(descriptor, &resource.bytes)?;
                }
                writer.finish(Descriptor {
                    resources: Vec::new(),
                    ..package.descriptor
                })?;
            }
            Outcome {
                valid: report.valid(),
                report,
                error: None,
            }
        }
        Err(error) => Outcome {
            valid: false,
            report: Report::default(),
            error: Some(error.to_string()),
        },
    };
    println!("{}", serde_json::to_string(&outcome)?);
    Ok(())
}
