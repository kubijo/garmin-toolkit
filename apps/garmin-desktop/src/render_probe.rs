//! Headless execution of the real demo UI on an explicitly selected hardware GPU.
//!
//! This development host deliberately omits native presentation. Child windows are embedded.

use std::{
    fs, io,
    path::PathBuf,
    time::{Duration, Instant},
};

use clap::Parser as _;
use eframe::{App as _, egui};
use futures_lite::future::block_on;
use garmin_ui::automation;
use garmin_ui::automation::Driver;
use serde_json::json;

mod deadline;
mod recording;
mod renderer;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

#[derive(clap::Parser)]
struct Options {
    /// New directory for reports and frames; existing paths are rejected.
    #[arg(long)]
    output: PathBuf,
    /// Hardware adapter name substring. Defaults to the sole AMD hardware adapter.
    #[arg(long)]
    adapter: Option<String>,
    /// Built-in scenario. Repeat to run several; omit to control through the HTTP API.
    #[arg(long, value_parser = scenario)]
    scenario: Vec<String>,
    /// Run budget including initialization; a watchdog exits after ten additional cleanup seconds.
    #[arg(long, default_value_t = 120, value_parser = clap::value_parser!(u64).range(1..=900))]
    seconds: u64,
    /// Capture every N rendered frames; zero disables periodic recording.
    #[arg(long, default_value_t = 60)]
    capture_every: u64,
    /// Maximum frames written to disk, including control API screenshots.
    #[arg(long, default_value_t = 120, value_parser = clap::value_parser!(u64).range(1..=600))]
    max_captures: u64,
}

fn scenario(value: &str) -> std::result::Result<String, String> {
    if automation::SCENARIOS.contains(&value) {
        Ok(value.into())
    } else {
        Err(format!(
            "expected one of {}",
            automation::SCENARIOS.join(", ")
        ))
    }
}

/// Run the bounded, opt-in rendering probe with command-line options.
///
/// # Errors
/// Returns adapter, initialization, rendering, recording, timeout, or scenario failures.
pub fn run() -> Result<()> {
    let options = Options::parse();
    let started = Instant::now();
    let _deadline = deadline::Deadline::start(Duration::from_secs(options.seconds + 10))?;
    let mut renderer = renderer::Renderer::new(options.adapter.as_deref())?;
    fs::create_dir(&options.output)?;
    let data = tempfile::tempdir()?;
    let context = egui::Context::default();
    crate::native::forbid(&context);
    context.set_embed_viewports(true);
    context.enable_accesskit();
    garmin_ui::install(&context);
    let logs = garmin_logging::Store::open(data.path().join("logs"), "render-probe")?;
    crate::developer::install(
        &context,
        crate::Options {
            ui_automation: true,
            control_server: true,
        },
        logs.clone(),
    )?;
    let adapter = renderer.state.adapter.get_info();
    let description = format!(
        "Headless render probe (4x MSAA, CPU captures)\nAdapter: {}\nBackend: {:?}\nDriver: {} {}",
        adapter.name, adapter.backend, adapter.driver, adapter.driver_info
    );
    eprintln!("{description}");
    garmin_ui::developer::state(&context)
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .debug = description;
    let deployment = block_on(crate::mode::open(data.path()))?;
    let map =
        garmin_ui::activity::install_wgpu_map(&renderer.state, u32::from(crate::MULTISAMPLING));
    let mut app = crate::view::Desktop::new(
        std::sync::Arc::clone(&deployment),
        garmin_i18n::Translations::bundled()?,
        context.clone(),
        crate::device_backend::open(data.path())?,
        data.path(),
        map,
        crate::profiling::RuntimeMetricsRecorder::default(),
    )?;
    let outcome = (|| {
        let mut recording = recording::Recording::new(&options.output, options.max_captures)?;
        let outcome = drive(
            &options,
            &context,
            &mut app,
            &mut renderer,
            &mut recording,
            started,
        );
        let recorded = recording.finish();
        write_summary(&options, &adapter, &context, &outcome, &recorded)?;
        outcome?;
        recorded?;
        Ok(())
    })();
    app.on_exit();
    drop(app);
    block_on(deployment.close());
    logs.flush();
    outcome
}

fn write_summary(
    options: &Options,
    adapter: &eframe::wgpu::AdapterInfo,
    context: &egui::Context,
    outcome: &Result<()>,
    recorded: &Result<recording::Summary>,
) -> Result<()> {
    let report = json!({
        "adapter": {"name": adapter.name, "vendor": adapter.vendor, "device": adapter.device,
            "backend": format!("{:?}", adapter.backend), "driver": adapter.driver,
            "driver_info": adapter.driver_info},
        "presentation": "offscreen", "msaa_samples": crate::MULTISAMPLING,
        "requested_scenarios": options.scenario,
        "last_automation_report": context.plugin::<Driver>().lock().report().cloned(),
        "performance_eligible": false, "error": outcome.as_ref().err().map(ToString::to_string),
        "recording": recorded.as_ref().ok(),
        "recording_error": recorded.as_ref().err().map(ToString::to_string),
        "limitations": ["no native presentation", "embedded child windows", "demo devices only"]
    });
    fs::write(
        options.output.join("summary.json"),
        serde_json::to_vec_pretty(&report)?,
    )?;
    Ok(())
}

fn drive(
    options: &Options,
    context: &egui::Context,
    app: &mut crate::view::Desktop,
    renderer: &mut renderer::Renderer,
    recording: &mut recording::Recording,
    started: Instant,
) -> Result<()> {
    let mut frame = eframe::Frame::_new_kittest();
    frame.wgpu_render_state = Some(renderer.state.clone());
    let mut size = egui::vec2(1100.0, 720.0);
    let mut events = Vec::new();
    let mut frame_number = 0_u64;
    let mut scenarios = Scenarios::default();
    while started.elapsed() < Duration::from_secs(options.seconds) {
        let tick = Instant::now();
        let mut input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
            time: Some(started.elapsed().as_secs_f64()),
            focused: true,
            events: std::mem::take(&mut events),
            ..Default::default()
        };
        let viewport = input.viewports.entry(egui::ViewportId::ROOT).or_default();
        viewport.native_pixels_per_point = Some(1.0);
        viewport.inner_rect = input.screen_rect;
        viewport.focused = Some(true);
        let mut output = context.run_ui(input, |ui| app.ui(ui, &mut frame));
        crate::native::check(context)?;
        let screenshots = match viewport_commands(&output, &mut size) {
            Ok(screenshots) => screenshots,
            Err(error) => {
                output.textures_delta.clear();
                return Err(error);
            }
        };
        let capture = !screenshots.is_empty()
            || (options.capture_every != 0
                && frame_number.is_multiple_of(options.capture_every)
                && recording.has_capacity());
        let clear_color = app.clear_color(&context.global_style().visuals);
        let pixels = renderer.draw(context, &mut output, capture, clear_color)?;
        if let Some(pixels) = pixels {
            for user_data in screenshots {
                events.push(egui::Event::Screenshot {
                    viewport_id: egui::ViewportId::ROOT,
                    user_data,
                    image: std::sync::Arc::new(pixels.clone()),
                });
            }
            recording.offer(
                frame_number,
                tick.duration_since(started).as_secs_f64(),
                pixels,
            )?;
        }
        if scenarios.advance(options, context)? {
            return Ok(());
        }
        frame_number += 1;
        std::thread::sleep(Duration::from_secs_f64(1.0 / 60.0).saturating_sub(tick.elapsed()));
    }
    if options.scenario.is_empty() {
        return Ok(());
    }
    let report = context.plugin::<Driver>().lock().report().cloned();
    fs::write(
        options.output.join("timeout.json"),
        serde_json::to_vec_pretty(&report)?,
    )?;
    Err(io::Error::new(
        io::ErrorKind::TimedOut,
        "render probe exceeded its time limit",
    )
    .into())
}

#[derive(Default)]
struct Scenarios {
    next: usize,
    active: bool,
}

impl Scenarios {
    fn advance(&mut self, options: &Options, context: &egui::Context) -> Result<bool> {
        if self.active {
            let report = context.plugin::<Driver>().lock().report().cloned();
            let releasing = automation::command(context, "status", &serde_json::Value::Null)
                .map_err(io::Error::other)?["needs_background_frames"]
                .as_bool()
                .unwrap_or(false);
            if let Some(report) = report.filter(|report| report.state != "running" && !releasing) {
                fs::write(
                    options
                        .output
                        .join(format!("scenario-{:02}.json", self.next)),
                    serde_json::to_vec_pretty(&report)?,
                )?;
                if report.state != "passed" {
                    return Err(io::Error::other(format!(
                        "{}: {} ({:?})",
                        report.scenario, report.state, report.failure
                    ))
                    .into());
                }
                eprintln!(
                    "{}: {}/{} passed",
                    report.scenario, report.completed, report.total
                );
                self.next += 1;
                self.active = false;
                return Ok(self.next == options.scenario.len());
            }
        } else if let Some(name) = options.scenario.get(self.next) {
            // Start after the first rendered frame has populated the semantic tree.
            automation::command(
                context,
                "start",
                &json!({
                    "argument": name, "run_in_background": true
                }),
            )
            .map_err(io::Error::other)?;
            self.active = true;
        }
        Ok(false)
    }
}

fn viewport_commands(
    output: &egui::FullOutput,
    size: &mut egui::Vec2,
) -> Result<Vec<egui::UserData>> {
    let mut screenshots = Vec::new();
    if let Some(viewport) = output.viewport_output.get(&egui::ViewportId::ROOT) {
        for command in &viewport.commands {
            match command {
                egui::ViewportCommand::InnerSize(requested) => {
                    if !requested.is_finite()
                        || requested.min_elem() < 1.0
                        || requested.max_elem() > 2048.0
                    {
                        return Err(io::Error::other(
                            "probe viewport must be within 1..=2048 pixels",
                        )
                        .into());
                    }
                    *size = *requested;
                }
                egui::ViewportCommand::Screenshot(data) => screenshots.push(data.clone()),
                egui::ViewportCommand::Close => {
                    return Err(io::Error::other("probe window closed").into());
                }
                _ => {}
            }
        }
    }
    Ok(screenshots)
}
