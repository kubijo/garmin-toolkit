use egui_kittest::{
    Harness,
    kittest::{NodeT as _, Queryable as _},
};
use garmin_i18n::{Language, Translations};
use garmin_ui::progress_indicator::{self, Props, Status, Step};

#[test]
fn only_interactive_enabled_steps_request_navigation() {
    let steps = [
        Step {
            label: "Source",
            description: None,
            status: Status::Complete,
        },
        Step {
            label: "Destination",
            description: Some("Choose a folder"),
            status: Status::Current,
        },
        Step {
            label: "Transfer",
            description: None,
            status: Status::Disabled,
        },
    ];
    let intl = Translations::bundled()
        .expect("bundled translations")
        .formatter(Language::English)
        .expect("English formatter");
    for (width, interactive) in [(720.0, false), (280.0, true), (720.0, true)] {
        let mut installed = false;
        let mut view = Harness::builder().with_size([width, 280.0]).build_ui_state(
            |ui, selected| {
                if !installed {
                    garmin_ui::install(ui.ctx());
                    installed = true;
                    ui.ctx().request_repaint();
                    return;
                }
                let choice = progress_indicator::show(
                    ui,
                    &intl,
                    &Props {
                        steps: &steps,
                        interactive,
                    },
                );
                if choice.is_some() {
                    *selected = choice;
                }
            },
            None,
        );
        view.run();
        let first = view.get_by_label("Source").rect();
        let next = view.get_by_label("Destination").rect();
        if width < 300.0 {
            assert!(next.top() >= first.bottom());
        } else {
            assert!(next.left() >= first.right());
        }
        assert_eq!(
            view.get_by_label("Destination")
                .accesskit_node()
                .description()
                .as_deref(),
            Some("Current step. Choose a folder")
        );
        if interactive {
            assert!(view.get_by_label("Transfer").accesskit_node().is_disabled());
        }
        view.get_by_label("Source").click();
        view.run();
        assert_eq!(*view.state(), interactive.then_some(0));
        view.get_by_label("Transfer").click();
        view.run();
        assert_eq!(*view.state(), interactive.then_some(0));
    }
}
