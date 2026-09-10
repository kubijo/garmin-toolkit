use super::PreviewScreen;
use crate::progress_dashboard::{DashboardScroll, DashboardViewport, render_progress_dashboard};
use crate::{ProgressModel, ProgressPhase, ProgressPresentation};
use ratatui::{Frame, layout::Rect};

/// Persistent viewport and input state for one gallery terminal.
#[derive(Debug, Clone, Default)]
pub struct PreviewState {
    screen: Option<PreviewScreen>,
    scroll: DashboardScroll,
    viewport: Option<(DashboardViewport, ProgressPhase)>,
}

impl PreviewState {
    /// Whether this screen accepts progress-panel input.
    #[must_use]
    pub const fn is_interactive(&self) -> bool {
        self.viewport.is_some()
    }

    /// Forward terminal input without executing preview actions.
    pub fn input(&mut self, event: &crossterm::event::Event) -> bool {
        let Some((viewport, phase)) = &self.viewport else {
            return false;
        };
        let before = self.scroll.clone();
        self.scroll.input(event, viewport, *phase);
        self.scroll != before
    }

    pub(super) fn prepare(&mut self, screen: PreviewScreen) {
        if self.screen != Some(screen) {
            *self = Self::default();
            self.screen = Some(screen);
            if screen == PreviewScreen::ProgressOverflow {
                self.scroll.active = 3;
            }
        }
        self.viewport = None;
    }

    pub(super) fn render_progress(
        &mut self,
        frame: &mut Frame<'_>,
        area: Rect,
        presentation: ProgressPresentation<'_>,
        model: &ProgressModel,
    ) {
        let device_state = Ok(super::screens::storage_fixture(false));
        let area = crate::device_state::render_update(frame, area, Some(&device_state));
        let viewport =
            render_progress_dashboard(frame, area, presentation, model, &mut self.scroll);
        self.viewport = Some((viewport, presentation.phase));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{RunProfile, preview::render_preview};
    use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
    use ratatui::{Terminal, backend::TestBackend, buffer::Buffer};

    fn render(
        state: &mut PreviewState,
        screen: PreviewScreen,
        tick: usize,
        size: (u16, u16),
    ) -> Buffer {
        let mut terminal = Terminal::new(TestBackend::new(size.0, size.1)).expect("terminal");
        terminal
            .draw(|frame| render_preview(frame, RunProfile::PRODUCTION, screen, tick, state))
            .expect("preview");
        terminal.backend().buffer().clone()
    }

    fn key(state: &mut PreviewState, key: KeyCode) -> bool {
        state.input(&Event::Key(KeyEvent::new(key, KeyModifiers::NONE)))
    }

    fn buffer_text(buffer: &Buffer) -> String {
        buffer
            .content()
            .iter()
            .map(ratatui::buffer::Cell::symbol)
            .collect()
    }

    #[test]
    fn pending_recovery_preview_shows_both_operator_choices() {
        let mut state = PreviewState::default();
        let buffer = render(&mut state, PreviewScreen::PendingRecovery, 0, (80, 24));
        let text = buffer_text(&buffer);

        assert!(text.contains("Recover now"));
        assert!(text.contains("Clear state"));
    }

    #[test]
    fn scroll_survives_animation_and_clamps_on_resize_without_leaking() {
        let screen = PreviewScreen::ProgressOverflow;
        let mut state = PreviewState::default();
        render(&mut state, screen, 10, (80, 24));
        assert!(key(&mut state, KeyCode::Down));
        let scrolled = state.scroll.clone();
        render(&mut state, screen, 20, (80, 24));
        assert_eq!(state.scroll, scrolled);

        let mut other = PreviewState::default();
        render(&mut other, screen, 20, (80, 24));
        assert_ne!(other.scroll, state.scroll);

        render(&mut state, screen, 30, (100, 30));
        assert_eq!(state.scroll.active, 4);
        assert_ne!(state.scroll, scrolled);

        render(&mut state, PreviewScreen::MapSelection, 30, (100, 30));
        assert!(!state.is_interactive());
        assert!(!key(&mut state, KeyCode::Down));
    }

    #[test]
    fn static_completion_scrolls_without_closing_or_aborting() {
        let mut state = PreviewState::default();
        let before = render(&mut state, PreviewScreen::Completion, 0, (80, 24));
        assert!(key(&mut state, KeyCode::End));
        let after = render(&mut state, PreviewScreen::Completion, 0, (80, 24));
        assert_ne!(before, after);
        assert!(!key(&mut state, KeyCode::Enter));
        assert!(!key(&mut state, KeyCode::Esc));
        assert_eq!(
            after,
            render(&mut state, PreviewScreen::Completion, 0, (80, 24))
        );
    }
}
