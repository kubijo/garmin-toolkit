use crossterm::event::{
    Event, KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use gallery::egui::{self, Key, Response, Ui};
use garmin_cli_tui::preview::PreviewState;
use num_traits::ToPrimitive as _;

#[cfg(test)]
mod tests;

#[derive(Clone, Default)]
pub struct TerminalInput {
    pub state: PreviewState,
    pub revision: u64,
    wheel_lines: f32,
    cells: [u16; 2],
}

impl TerminalInput {
    pub fn set_grid(&mut self, cells: [u16; 2]) {
        if self.cells != cells {
            self.cells = cells;
            self.wheel_lines = 0.0;
            self.revision = self.revision.wrapping_add(1);
        }
    }

    pub fn interact(&mut self, ui: &Ui, response: &Response, image: egui::Rect, cells: [u16; 2]) {
        if !self.state.is_interactive() {
            return;
        }
        let mut changed = false;
        if response.clicked() {
            response.request_focus();
            ui.ctx().request_repaint();
            if let Some(position) = response
                .interact_pointer_pos()
                .and_then(|pos| terminal_cell(image, cells, pos))
            {
                changed |= self.mouse(MouseEventKind::Down(MouseButton::Left), position);
            }
        }
        if response.hovered() {
            changed |= self.wheel(ui, image, cells);
        } else {
            self.wheel_lines = 0.0;
        }
        if response.has_focus() {
            ui.memory_mut(|memory| {
                memory.set_focus_lock_filter(
                    response.id,
                    egui::EventFilter {
                        tab: true,
                        vertical_arrows: true,
                        escape: true,
                        ..egui::EventFilter::default()
                    },
                );
            });
            for key in take_keys(ui) {
                if key == KeyCode::Esc {
                    response.surrender_focus();
                } else {
                    changed |= self
                        .state
                        .input(&Event::Key(KeyEvent::new(key, KeyModifiers::NONE)));
                }
            }
        }
        if changed {
            self.revision = self.revision.wrapping_add(1);
            ui.ctx().request_repaint();
        }
    }

    fn wheel(&mut self, ui: &Ui, image: egui::Rect, cells: [u16; 2]) -> bool {
        let Some(position) = ui
            .input(|input| input.pointer.hover_pos())
            .and_then(|pos| terminal_cell(image, cells, pos))
        else {
            return false;
        };
        let delta = ui.input_mut(|input| {
            let delta = input.smooth_scroll_delta.y;
            input.smooth_scroll_delta.y = 0.0;
            delta
        });
        let cell_height = image.height() / f32::from(cells[1]);
        self.wheel_lines += delta / cell_height;
        let mut changed = false;
        for _ in 0..100 {
            if self.wheel_lines.abs() < 1.0 {
                break;
            }
            let direction = self.wheel_lines.signum();
            changed |= self.mouse(
                if direction > 0.0 {
                    MouseEventKind::ScrollUp
                } else {
                    MouseEventKind::ScrollDown
                },
                position,
            );
            self.wheel_lines -= direction;
        }
        changed
    }

    fn mouse(&mut self, kind: MouseEventKind, [column, row]: [u16; 2]) -> bool {
        self.state.input(&Event::Mouse(MouseEvent {
            kind,
            column,
            row,
            modifiers: KeyModifiers::NONE,
        }))
    }
}

fn terminal_cell(
    image: egui::Rect,
    [columns, rows]: [u16; 2],
    position: egui::Pos2,
) -> Option<[u16; 2]> {
    if columns == 0 || rows == 0 || !image.contains(position) || !image.is_positive() {
        return None;
    }
    let offset = (position - image.min) / image.size();
    Some([
        (offset.x * f32::from(columns))
            .floor()
            .to_u16()?
            .min(columns - 1),
        (offset.y * f32::from(rows)).floor().to_u16()?.min(rows - 1),
    ])
}

fn take_keys(ui: &Ui) -> Vec<KeyCode> {
    ui.input_mut(|input| {
        let mut keys = Vec::new();
        input.events.retain(|event| {
            if let egui::Event::Key {
                key,
                pressed: true,
                modifiers,
                ..
            } = event
                && !modifiers.ctrl
                && !modifiers.alt
                && !modifiers.command
                && let Some(key) = scroll_key(*key, modifiers.shift)
            {
                keys.push(key);
                return false;
            }
            true
        });
        keys
    })
}

const fn scroll_key(key: Key, shift: bool) -> Option<KeyCode> {
    Some(match key {
        Key::ArrowUp => KeyCode::Up,
        Key::ArrowDown => KeyCode::Down,
        Key::PageUp => KeyCode::PageUp,
        Key::PageDown => KeyCode::PageDown,
        Key::Home => KeyCode::Home,
        Key::End => KeyCode::End,
        Key::Tab => {
            if shift {
                KeyCode::BackTab
            } else {
                KeyCode::Tab
            }
        }
        Key::Escape => KeyCode::Esc,
        _ => return None,
    })
}
