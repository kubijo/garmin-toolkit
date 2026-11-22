use egui_kittest::{
    Harness,
    kittest::{By, NodeT as _, Queryable as _},
};
use gallery::egui::{
    self, CursorIcon, Key,
    accesskit::{Role, Toggled},
};
use garmin_ui::{checkbox, radio::Choice, radio_group, toggle};

#[test]
fn binary_controls_share_the_track_and_label_hit_area() {
    for (label, role) in [
        ("Include maps", Role::CheckBox),
        ("Show surface", Role::Switch),
    ] {
        let mut view = Harness::builder().build_ui_state(
            |ui, state: &mut (bool, bool)| {
                ui.add_enabled_ui(state.1, |ui| match role {
                    Role::Switch => toggle::show(ui, &mut state.0, label),
                    _ => checkbox::show(ui, &mut state.0, label),
                });
            },
            (false, true),
        );
        view.run();
        assert_eq!(view.get_by_label(label).accesskit_node().role(), role);
        let bounds = view.get_by_label(label).rect();
        for point in [
            bounds.left_center() + egui::vec2(8.0, 0.0),
            bounds.right_center() - egui::vec2(4.0, 0.0),
        ] {
            view.hover_at(point);
            view.run();
            assert_eq!(
                view.output().platform_output.cursor_icon,
                CursorIcon::PointingHand
            );
            let before = view.state().0;
            view.drag_at(point);
            view.drop_at(point);
            view.run();
            assert_ne!(view.state().0, before);
        }
        view.get_by_label(label).focus();
        view.run();
        view.key_press(Key::Space);
        view.run();
        assert_eq!(
            view.get_by_label(label).accesskit_node().data().toggled(),
            Some(Toggled::True)
        );
        view.state_mut().1 = false;
        view.run();
        view.get_by_label(label).hover();
        view.run();
        assert_eq!(
            view.output().platform_output.cursor_icon,
            CursorIcon::NotAllowed
        );
        view.get_by_label(label).click();
        view.run();
        assert!(view.state().0);
    }
}

#[test]
fn activating_a_partial_selection_selects_all() {
    let mut view = Harness::builder().build_ui_state(
        |ui, state: &mut Option<bool>| {
            checkbox::show_mixed(ui, state, "All files");
        },
        None,
    );
    view.run();
    assert_eq!(
        view.get_by_label("All files")
            .accesskit_node()
            .data()
            .toggled(),
        Some(Toggled::Mixed)
    );
    view.get_by_label("All files").click();
    view.run();
    assert_eq!(*view.state(), Some(true));
    assert_eq!(
        view.get_by_label("All files")
            .accesskit_node()
            .data()
            .toggled(),
        Some(Toggled::True)
    );
}

#[test]
fn radio_group_arrows_wrap_skip_disabled_and_move_focus() {
    let mut view = Harness::builder().build_ui_state(
        |ui, value| {
            if let Some(change) = radio_group::show(
                ui,
                *value,
                &[
                    Choice::new("First", 1, "first"),
                    Choice::new("Unavailable", 2, "second").enabled(false),
                    Choice::new("Third", 3, "third"),
                ],
                radio_group::Props {
                    label: "Options",
                    helper: None,
                    enabled: true,
                },
            ) {
                *value = change;
            }
        },
        1,
    );
    view.run();
    view.get(By::new().role(Role::RadioGroup));
    view.get_by_label("First").focus();
    view.run();
    for (key, label, expected) in [
        (Key::ArrowRight, "Third", 3),
        (Key::ArrowDown, "First", 1),
        (Key::ArrowLeft, "Third", 3),
    ] {
        view.key_press(key);
        view.run();
        assert_eq!(*view.state(), expected);
        assert!(view.get_by_label(label).accesskit_node().is_focused());
    }
}

#[test]
fn action_rows_own_the_gap_and_keep_the_primary_action_at_the_requested_edge() {
    use garmin_ui::{
        action_row::{self, Placement},
        button::{Kind, Props},
    };
    for placement in [Placement::Leading, Placement::Trailing] {
        for width in [176.0, 480.0] {
            let mut view = Harness::builder().with_size([width, 240.0]).build_ui_state(
                |ui, action| {
                    ui.spacing_mut().item_spacing = egui::vec2(23.0, 19.0);
                    if let Some(chosen) = action_row::show(
                        ui,
                        placement,
                        [
                            ("review", 1, Props::new("Review selection", Kind::Primary)),
                            ("edit", 2, Props::new("Edit selection", Kind::Secondary)),
                        ],
                    ) {
                        *action = chosen;
                    }
                },
                0,
            );
            view.run();
            let first = view.get_by_label("Review selection").rect();
            let second = view.get_by_label("Edit selection").rect();
            let gap = if width < 200.0 {
                assert_eq!(first.x_range(), second.x_range());
                second.top() - first.bottom()
            } else {
                assert_eq!(first.y_range(), second.y_range());
                match placement {
                    Placement::Leading => second.left() - first.right(),
                    Placement::Trailing => first.left() - second.right(),
                }
            };
            assert!((gap - 8.0).abs() < 0.1, "visible gap was {gap}");
            view.get_by_label("Edit selection").click();
            view.run();
            assert_eq!(*view.state(), 2);
        }
    }
}

#[test]
fn numeric_slider_tracks_have_a_name_pointer_and_keyboard_input() {
    use garmin_ui::slider;
    for editable in [true, false] {
        let mut view = Harness::builder().build_ui_state(
            |ui, state: &mut (i32, bool)| {
                ui.spacing_mut().interact_size.y = 40.0;
                ui.add_enabled_ui(state.1, |ui| {
                    if editable {
                        slider::editable(ui, "Zoom", &mut state.0, 0..=10)
                    } else {
                        slider::track(ui, "Zoom", &mut state.0, 0..=10)
                    }
                });
            },
            (4, true),
        );
        view.run();
        let track = || By::new().role(Role::Slider).label("Zoom");
        let bounds = view.get(track()).rect();
        assert!(bounds.height() >= 24.0);
        assert!(
            view.output().shapes.iter().any(|shape| {
                matches!(&shape.shape, egui::Shape::Circle(thumb)
                if bounds.contains(thumb.center) && thumb.radius <= 10.0)
            }),
            "expected a compact round thumb inside the slider hit area"
        );
        view.get(track()).hover();
        view.run();
        assert_eq!(
            view.output().platform_output.cursor_icon,
            CursorIcon::PointingHand
        );
        view.get(track()).focus();
        view.run();
        view.key_press(Key::ArrowRight);
        view.run();
        assert_eq!(view.state().0, 5);
        view.state_mut().1 = false;
        view.run();
        view.get(track()).hover();
        view.run();
        assert_eq!(
            view.output().platform_output.cursor_icon,
            CursorIcon::NotAllowed
        );
        view.key_press(Key::ArrowRight);
        view.run();
        assert_eq!(view.state().0, 5);
    }
}
