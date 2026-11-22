use gallery::prelude::*;
use garmin_ui::progress_indicator;
use garmin_ui::progress_indicator::{Props, Status, Step};

scene_meta! { title: "Components / Feedback / Progress indicator" }

const LABELS: [&str; 5] = ["Choose files", "Review", "Transfer", "Validate", "Finish"];
const DETAILS: [&str; 5] = [
    "3 files selected",
    "Check the destination",
    "Copy the files",
    "Check the file sizes",
    "Close the transfer",
];

#[scene(default)]
fn overview(ctx: &mut SceneCtx<'_>, ui: &mut Ui, globals: &crate::Globals) {
    let current = ctx.buttons("current step", &LABELS, 1);
    let invalid = ctx.toggle("validation error", true);
    let disabled = ctx.toggle("disable final step", true);
    let details = ctx.toggle("descriptions", true);
    let count = ctx.buttons("loading steps", &["Three", "Five", "Seven"], 1);
    let width = ctx.slider("width", 736.0, 280.0, 1040.0, 8.0);
    let intl = globals.intl();

    globals.example(ctx, ui, "Step states", width, |ui| {
        let steps = std::array::from_fn::<_, 5, _>(|i| {
            let status = if disabled && i == 4 {
                Status::Disabled
            } else if invalid && i == 3 {
                Status::Invalid
            } else {
                status(i, current)
            };
            let detail = if status == Status::Invalid {
                "File sizes differ"
            } else {
                DETAILS[i]
            };
            Step {
                label: LABELS[i],
                description: details.then_some(detail),
                status,
            }
        });
        let _ = progress_indicator::show(
            ui,
            &intl,
            &Props {
                steps: &steps,
                interactive: false,
            },
        );
    });

    globals.example(ctx, ui, "Interactive", width, |ui| {
        let id = ui.id().with("current-step");
        let selected = ui.data(|data| data.get_temp::<usize>(id)).unwrap_or(1);
        let interactive = std::array::from_fn::<_, 5, _>(|i| Step {
            label: LABELS[i],
            description: None,
            status: if disabled && i == 4 {
                Status::Disabled
            } else {
                status(i, selected)
            },
        });
        if let Some(next) = progress_indicator::show(
            ui,
            &intl,
            &Props {
                steps: &interactive,
                interactive: true,
            },
        ) {
            ui.data_mut(|data| data.insert_temp(id, next));
        }
    });

    globals.example(ctx, ui, "Complete", width, |ui| {
        let complete = LABELS.map(|label| Step {
            label,
            description: None,
            status: Status::Complete,
        });
        let _ = progress_indicator::show(
            ui,
            &intl,
            &Props {
                steps: &complete,
                interactive: false,
            },
        );
    });

    globals.example(ctx, ui, "Loading", width, |ui| {
        ui.style_mut().animation_time = 0.1;
        progress_indicator::skeleton(ui, [3, 5, 7][count]);
    });
}

fn status(index: usize, current: usize) -> Status {
    match index.cmp(&current) {
        std::cmp::Ordering::Less => Status::Complete,
        std::cmp::Ordering::Equal => Status::Current,
        std::cmp::Ordering::Greater => Status::Upcoming,
    }
}
