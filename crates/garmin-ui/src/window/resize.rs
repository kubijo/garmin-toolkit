//! Resize hit regions shared by undecorated native windows.
use egui::{CursorIcon, Pos2, Rect, ResizeDirection, Ui, ViewportCommand};

const EDGE_WIDTH: f32 = 6.0;
const CORNER_WIDTH: f32 = 12.0;
const GRIP_SIZE: f32 = 32.0;

/// Starts native resizing from the current viewport's edges and corners.
pub fn resize(ui: &Ui) {
    resize_at(ui, ui.ctx().viewport_rect());
}

pub(super) fn resize_at(ui: &Ui, bounds: Rect) {
    let context = ui.ctx();
    let unavailable = context.input(|input| {
        input.viewport().maximized.unwrap_or_default()
            || input.viewport().fullscreen.unwrap_or_default()
    });
    if unavailable {
        return;
    }
    paint_south_east_grip(ui, bounds);
    let Some(pointer) = context.pointer_hover_pos() else {
        return;
    };
    let on_grip = south_east_grip(bounds).contains(pointer);
    let Some((direction, cursor)) = resize_target(bounds, pointer) else {
        return;
    };
    // Scrollbars and controls can reach the client edge.
    // Respect egui's hit test instead of starting an OS resize over a widget that owns the pointer.
    if !on_grip && context.egui_is_using_pointer() {
        return;
    }
    if !on_grip {
        let hovered = context.interaction_snapshot(|interaction| {
            interaction.hovered.iter().copied().collect::<Vec<_>>()
        });
        if hovered.into_iter().any(|id| {
            context.read_response(id).is_some_and(|response| {
                response.sense.senses_click() || response.sense.senses_drag()
            })
        }) {
            return;
        }
    }
    context.set_cursor_icon(cursor);
    if context.input(|input| input.pointer.primary_pressed()) {
        context.send_viewport_cmd(ViewportCommand::BeginResize(direction));
    }
}

fn resize_target(bounds: Rect, pointer: Pos2) -> Option<(ResizeDirection, CursorIcon)> {
    if !bounds.contains(pointer) {
        return None;
    }
    if south_east_grip(bounds).contains(pointer) {
        return Some((ResizeDirection::SouthEast, CursorIcon::ResizeSouthEast));
    }
    let left = pointer.x <= bounds.left() + EDGE_WIDTH;
    let right = pointer.x >= bounds.right() - EDGE_WIDTH;
    let top = pointer.y <= bounds.top() + EDGE_WIDTH;
    let bottom = pointer.y >= bounds.bottom() - EDGE_WIDTH;
    if !(left || right || top || bottom) {
        return None;
    }
    let near_left = pointer.x <= bounds.left() + CORNER_WIDTH;
    let near_right = pointer.x >= bounds.right() - CORNER_WIDTH;
    let near_top = pointer.y <= bounds.top() + CORNER_WIDTH;
    let near_bottom = pointer.y >= bounds.bottom() - CORNER_WIDTH;
    let target = match (near_left, near_right, near_top, near_bottom) {
        (true, _, true, _) => (ResizeDirection::NorthWest, CursorIcon::ResizeNorthWest),
        (_, true, true, _) => (ResizeDirection::NorthEast, CursorIcon::ResizeNorthEast),
        (true, _, _, true) => (ResizeDirection::SouthWest, CursorIcon::ResizeSouthWest),
        (_, true, _, true) => (ResizeDirection::SouthEast, CursorIcon::ResizeSouthEast),
        _ if top => (ResizeDirection::North, CursorIcon::ResizeNorth),
        _ if bottom => (ResizeDirection::South, CursorIcon::ResizeSouth),
        _ if left => (ResizeDirection::West, CursorIcon::ResizeWest),
        _ if right => (ResizeDirection::East, CursorIcon::ResizeEast),
        _ => return None,
    };
    Some(target)
}

fn south_east_grip(bounds: Rect) -> Rect {
    Rect::from_min_max(
        Pos2::new(bounds.right() - GRIP_SIZE, bounds.bottom() - GRIP_SIZE),
        bounds.right_bottom(),
    )
}

fn paint_south_east_grip(ui: &Ui, bounds: Rect) {
    let right = bounds.right();
    let bottom = bounds.bottom();
    let stroke = egui::Stroke::new(
        1.5,
        crate::theme::color32(crate::theme::palette(ui).content().icon_secondary())
            .gamma_multiply(0.2),
    );
    for length in [6.0, 12.0, 18.0] {
        ui.painter().line_segment(
            [
                Pos2::new(right - length, bottom),
                Pos2::new(right, bottom - length),
            ],
            stroke,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BOUNDS: Rect = Rect::from_min_max(Pos2::ZERO, Pos2::new(100.0, 80.0));

    #[test]
    fn corners_take_precedence_over_edges() {
        assert_eq!(
            resize_target(BOUNDS, Pos2::new(4.0, 8.0)),
            Some((ResizeDirection::NorthWest, CursorIcon::ResizeNorthWest))
        );
        assert_eq!(
            resize_target(BOUNDS, Pos2::new(96.0, 72.0)),
            Some((ResizeDirection::SouthEast, CursorIcon::ResizeSouthEast))
        );
    }

    #[test]
    fn center_is_not_a_resize_target() {
        assert_eq!(resize_target(BOUNDS, BOUNDS.center()), None);
        assert_eq!(resize_target(BOUNDS, Pos2::new(8.0, 8.0)), None);
        assert_eq!(resize_target(BOUNDS, Pos2::new(-1.0, 40.0)), None);
    }

    #[test]
    fn lower_right_grip_resizes_over_draggable_content() {
        let context = egui::Context::default();
        let pointer = Pos2::new(88.0, 68.0);
        let output = context.run_ui(
            egui::RawInput {
                screen_rect: Some(BOUNDS),
                events: vec![
                    egui::Event::PointerMoved(pointer),
                    egui::Event::PointerButton {
                        pos: pointer,
                        button: egui::PointerButton::Primary,
                        pressed: true,
                        modifiers: egui::Modifiers::NONE,
                    },
                ],
                ..egui::RawInput::default()
            },
            |ui| {
                ui.interact(
                    BOUNDS,
                    egui::Id::new("draggable-chart"),
                    egui::Sense::drag(),
                );
                resize_at(ui, BOUNDS);
            },
        );
        assert!(output.viewport_output.values().any(|viewport| {
            viewport.commands.iter().any(|command| {
                matches!(
                    command,
                    ViewportCommand::BeginResize(ResizeDirection::SouthEast)
                )
            })
        }));
        output.drop_without_applying_deltas();
    }

    #[test]
    fn unclaimed_edge_still_starts_native_resize() {
        let context = egui::Context::default();
        let pointer = Pos2::new(99.0, 40.0);
        let output = context.run_ui(
            egui::RawInput {
                screen_rect: Some(BOUNDS),
                events: vec![
                    egui::Event::PointerMoved(pointer),
                    egui::Event::PointerButton {
                        pos: pointer,
                        button: egui::PointerButton::Primary,
                        pressed: true,
                        modifiers: egui::Modifiers::NONE,
                    },
                ],
                ..egui::RawInput::default()
            },
            |ui| resize_at(ui, BOUNDS),
        );
        assert!(output.viewport_output.values().any(|viewport| {
            viewport.commands.iter().any(|command| {
                matches!(command, ViewportCommand::BeginResize(ResizeDirection::East))
            })
        }));
        output.drop_without_applying_deltas();
    }

    #[test]
    fn scrollbar_at_window_edge_keeps_its_drag() {
        assert_scrollbar_drag(egui::style::ScrollStyle::solid());
        assert_scrollbar_drag(egui::style::ScrollStyle::floating());
    }

    fn assert_scrollbar_drag(scroll: egui::style::ScrollStyle) {
        use egui::{Event, Modifiers, PointerButton, RawInput, ScrollArea};

        let context = egui::Context::default();
        context.all_styles_mut(|style| {
            style.spacing.scroll = scroll;
            style.spacing.scroll.bar_outer_margin = 0.0;
        });
        let start = Pos2::new(99.0, 16.0);
        let end = Pos2::new(99.0, 55.0);
        let press = |pos, pressed| Event::PointerButton {
            pos,
            button: PointerButton::Primary,
            pressed,
            modifiers: Modifiers::NONE,
        };
        let mut offset = 0.0;
        for events in [
            vec![Event::PointerMoved(start)],
            vec![Event::PointerMoved(start)],
            vec![press(start, true)],
            vec![Event::PointerMoved(end)],
            vec![press(end, false)],
        ] {
            let output = context.run_ui(
                RawInput {
                    screen_rect: Some(BOUNDS),
                    events,
                    ..RawInput::default()
                },
                |ui| {
                    offset = ScrollArea::vertical()
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            ui.allocate_space(egui::vec2(40.0, 500.0));
                        })
                        .state
                        .offset
                        .y;
                    resize_at(ui, BOUNDS);
                },
            );
            assert!(
                !output.viewport_output.values().any(|viewport| {
                    viewport
                        .commands
                        .iter()
                        .any(|command| matches!(command, ViewportCommand::BeginResize(_)))
                }),
                "scrollbar input must not begin a window resize"
            );
            output.drop_without_applying_deltas();
        }
        assert!(
            offset > 0.0,
            "dragging the scrollbar must scroll its content"
        );
    }
}
