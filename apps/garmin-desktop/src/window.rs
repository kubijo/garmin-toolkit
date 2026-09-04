use eframe::egui::{CursorIcon, Pos2, Rect, ResizeDirection, Ui, ViewportCommand};

const EDGE_WIDTH: f32 = 6.0;
const CORNER_WIDTH: f32 = 12.0;

pub fn resize(ui: &Ui) {
    let context = ui.ctx();
    let unavailable = context.input(|input| {
        input.viewport().maximized.unwrap_or_default()
            || input.viewport().fullscreen.unwrap_or_default()
    });
    if unavailable {
        return;
    }
    let Some(pointer) = context.pointer_hover_pos() else {
        return;
    };
    let Some((direction, cursor)) = resize_target(ui.max_rect(), pointer) else {
        return;
    };
    context.set_cursor_icon(cursor);
    if context.input(|input| input.pointer.primary_pressed()) {
        context.send_viewport_cmd(ViewportCommand::BeginResize(direction));
    }
}

fn resize_target(bounds: Rect, pointer: Pos2) -> Option<(ResizeDirection, CursorIcon)> {
    let left = pointer.x <= bounds.left() + EDGE_WIDTH;
    let right = pointer.x >= bounds.right() - EDGE_WIDTH;
    let top = pointer.y <= bounds.top() + EDGE_WIDTH;
    let bottom = pointer.y >= bounds.bottom() - EDGE_WIDTH;
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
    }
}
