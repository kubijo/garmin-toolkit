use crossterm::event::{Event, KeyCode, KeyEventKind, KeyModifiers, MouseEventKind};
use garmin_i18n::format_message;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{
    Block, Borders, Padding, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState, Wrap,
};

use super::progress_state::{OperationView, ProgressModel};
use super::{
    ProgressInput, ProgressPhase, ProgressPresentation, decimal_bytes, history_prefix,
    operation_line, operation_text, path_style, progress_metrics, render_progress_footer,
    render_progress_gauge, selected_formatter, stage_name, stale_byte_progress, state_icon,
};
const ACTIVE_MAX_HEIGHT: u16 = 10;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum Panel {
    #[default]
    Active,
    History,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct DashboardScroll {
    pub active: usize,
    history: usize,
    history_lines_seen: usize,
    history_rows_seen: usize,
    focus: Panel,
}

#[derive(Debug, Clone, Copy, Default)]
struct Viewport {
    area: Rect,
    lines: usize,
}

impl Viewport {
    const fn height(self) -> usize {
        self.area.height.saturating_sub(2) as usize
    }

    const fn max_offset(self) -> usize {
        self.lines.saturating_sub(self.height())
    }
}

#[derive(Debug, Clone, Default)]
pub(super) struct DashboardViewport {
    active: Viewport,
    history: Viewport,
}

impl DashboardScroll {
    pub fn input(
        &mut self,
        event: &Event,
        viewport: &DashboardViewport,
        phase: ProgressPhase,
    ) -> ProgressInput {
        if phase == ProgressPhase::Complete {
            self.focus = Panel::History;
        }
        match event {
            Event::Key(key) if key.kind == KeyEventKind::Press => match key.code {
                KeyCode::Tab | KeyCode::BackTab if phase == ProgressPhase::Running => {
                    self.focus = if self.focus == Panel::Active {
                        Panel::History
                    } else {
                        Panel::Active
                    };
                }
                KeyCode::Up
                | KeyCode::Down
                | KeyCode::PageUp
                | KeyCode::PageDown
                | KeyCode::Home
                | KeyCode::End => {
                    self.scroll(key.code, viewport);
                }
                KeyCode::Enter if phase == ProgressPhase::Complete => return ProgressInput::Close,
                KeyCode::Esc | KeyCode::Char('q' | 'Q') => return exit_input(phase),
                KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    return exit_input(phase);
                }
                _ => {}
            },
            Event::Mouse(mouse) => {
                let position = Position::new(mouse.column, mouse.row);
                let panel =
                    if viewport.active.area.contains(position) && phase == ProgressPhase::Running {
                        Panel::Active
                    } else if viewport.history.area.contains(position) {
                        Panel::History
                    } else {
                        return ProgressInput::Continue;
                    };
                match mouse.kind {
                    MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
                        self.focus = panel;
                        self.scroll(
                            if mouse.kind == MouseEventKind::ScrollUp {
                                KeyCode::Up
                            } else {
                                KeyCode::Down
                            },
                            viewport,
                        );
                    }
                    MouseEventKind::Down(_) => self.focus = panel,
                    _ => {}
                }
            }
            _ => {}
        }
        ProgressInput::Continue
    }

    fn scroll(&mut self, key: KeyCode, viewport: &DashboardViewport) {
        let (offset, view) = match self.focus {
            Panel::Active => (&mut self.active, viewport.active),
            Panel::History => (&mut self.history, viewport.history),
        };
        *offset = match key {
            KeyCode::Up => offset.saturating_sub(1),
            KeyCode::Down => offset.saturating_add(1),
            KeyCode::PageUp => offset.saturating_sub(view.height()),
            KeyCode::PageDown => offset.saturating_add(view.height()),
            KeyCode::Home => 0,
            KeyCode::End => view.max_offset(),
            _ => *offset,
        }
        .min(view.max_offset());
    }

    fn track_history(&mut self, lines: usize, rows_added: usize) {
        if self.history > 0 {
            let added_rows = rows_added.saturating_sub(self.history_rows_seen);
            let added_lines = lines.saturating_sub(self.history_lines_seen);
            self.history = self.history.saturating_add(added_rows.max(added_lines));
        }
        self.history_lines_seen = lines;
        self.history_rows_seen = rows_added;
    }
}

const fn exit_input(phase: ProgressPhase) -> ProgressInput {
    match phase {
        ProgressPhase::Running => ProgressInput::Abort,
        ProgressPhase::Cancelling => ProgressInput::Continue,
        ProgressPhase::Complete => ProgressInput::Close,
    }
}

pub(super) fn render_progress_dashboard(
    frame: &mut Frame<'_>,
    area: Rect,
    presentation: ProgressPresentation<'_>,
    model: &ProgressModel,
    scroll: &mut DashboardScroll,
) -> DashboardViewport {
    let intl = selected_formatter();
    let complete = presentation.phase == ProgressPhase::Complete;
    let cancelling = presentation.phase == ProgressPhase::Cancelling;
    if complete {
        scroll.focus = Panel::History;
    }
    let outer = Block::default()
        .title(format!(" {} ", presentation.title))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(if complete {
            Color::Green
        } else if cancelling {
            Color::Yellow
        } else {
            Color::Cyan
        }));
    let inner = outer.inner(area);
    frame.render_widget(outer, area);
    let active_count = model.active().count();
    let active = Paragraph::new(active_text(model, complete)).wrap(Wrap { trim: false });
    let active_lines = active.line_count(inner.width.saturating_sub(4));
    let stage_height = u16::try_from(model.stages.len()).unwrap_or(u16::MAX);
    let active_height = u16::try_from(active_lines)
        .unwrap_or(u16::MAX)
        .saturating_add(2)
        .min(ACTIVE_MAX_HEIGHT)
        .min(inner.height.saturating_sub(stage_height.saturating_add(8)));
    let areas = Layout::vertical([
        Constraint::Length(stage_height),
        Constraint::Length(1),
        Constraint::Length(active_height),
        Constraint::Length(1),
        Constraint::Min(4),
        Constraint::Length(2),
    ])
    .split(inner);
    render_stages(frame, areas[0], model);
    let history = Paragraph::new(history_text(model.history.iter())).wrap(Wrap { trim: false });
    let history_lines = history.line_count(inner.width.saturating_sub(4));
    scroll.track_history(history_lines, model.history_rows_added);
    let viewport = DashboardViewport {
        active: Viewport {
            area: areas[2],
            lines: active_lines,
        },
        history: Viewport {
            area: areas[4],
            lines: history_lines,
        },
    };
    scroll.active = scroll.active.min(viewport.active.max_offset());
    scroll.history = scroll.history.min(viewport.history.max_offset());
    let active_title = if complete {
        format_message!(&intl, default_message: "Complete")
    } else if active_count > 0 {
        format_message!(
            &intl,
            default_message: "Active files ({count})",
            values: { count: i64::try_from(active_count).unwrap_or(i64::MAX) },
        )
    } else {
        format_message!(&intl, default_message: "Current operation")
    };
    render_panel(
        frame,
        active,
        &active_title,
        viewport.active,
        scroll.active,
        if complete {
            Color::Green
        } else if scroll.focus == Panel::Active {
            Color::Yellow
        } else {
            Color::Gray
        },
    );
    render_panel(
        frame,
        history,
        &format_message!(&intl, default_message: "History — newest first"),
        viewport.history,
        scroll.history,
        if scroll.focus == Panel::History {
            Color::Yellow
        } else {
            Color::Gray
        },
    );
    render_progress_footer(frame, areas[5], presentation.phase);
    viewport
}

fn active_text(model: &ProgressModel, complete: bool) -> Text<'static> {
    let intl = selected_formatter();
    if complete || model.active().next().is_none() {
        return operation_text(&model.current, complete);
    }
    let mut lines = Vec::new();
    for item in model.active() {
        let view = &item.view;
        let progress = view.total.map_or_else(
            || decimal_bytes(view.completed),
            |total| {
                format!(
                    "{} / {}",
                    decimal_bytes(view.completed),
                    decimal_bytes(total)
                )
            },
        );
        let mut summary = vec![Span::raw(format!(
            "▸ {:<8}  {progress}",
            stage_name(item.stage)
        ))];
        for metric in progress_metrics(item.stage, view) {
            summary.push(Span::styled(
                format!(" · {metric}"),
                Style::default().fg(Color::Gray),
            ));
        }
        if stale_byte_progress(view).is_none()
            && let Some(updated) = view.updated_at
        {
            let idle = updated.elapsed().as_secs();
            let (label, color) = if idle < 2 {
                (
                    format_message!(&intl, default_message: "receiving"),
                    Color::Cyan,
                )
            } else if idle < 10 {
                (
                    format_message!(
                        &intl,
                        default_message: "last data {seconds}s ago",
                        values: { seconds: i64::try_from(idle).unwrap_or(i64::MAX) },
                    ),
                    Color::Gray,
                )
            } else {
                (
                    format_message!(
                        &intl,
                        default_message: "no data for {seconds}s",
                        values: { seconds: i64::try_from(idle).unwrap_or(i64::MAX) },
                    ),
                    Color::Yellow,
                )
            };
            summary.push(Span::styled(
                format!(" · {label}"),
                Style::default().fg(color),
            ));
        }
        lines.push(Line::from(summary));
        lines.push(Line::from(vec![
            Span::raw("  "),
            Span::styled(
                view.path.as_ref().unwrap_or(&view.label).clone(),
                path_style(),
            ),
        ]));
    }
    Text::from(lines)
}

fn history_text<'a>(history: impl DoubleEndedIterator<Item = &'a OperationView>) -> Text<'static> {
    let intl = selected_formatter();
    let mut lines = Vec::new();
    let mut entries = history.rev().peekable();
    while let Some(entry) = entries.next() {
        if entry.is_cached_verification()
            && let Some(download) = entries.peek()
            && download.is_cached_download()
            && entry.path == download.path
        {
            let download = entries.next().expect("peeked cached download");
            let path = entry
                .path
                .as_ref()
                .or(download.path.as_ref())
                .expect("cached file event has a path");
            let mut combined = entry.clone();
            combined.duration = match (entry.duration, download.duration) {
                (Some(verify), Some(download)) => Some(verify.saturating_add(download)),
                (duration, None) | (None, duration) => duration,
            };
            let mut spans = history_prefix(&combined);
            spans.extend([
                Span::styled(path.clone(), path_style()),
                Span::styled(" · ", Style::default().fg(Color::DarkGray)),
                Span::raw(format_message!(&intl, default_message: "cached")),
                Span::styled(" · ", Style::default().fg(Color::DarkGray)),
                Span::raw(format_message!(&intl, default_message: "MD5 verified")),
            ]);
            lines.push(Line::from(spans));
            continue;
        }
        lines.push(operation_line(entry, true));
    }
    Text::from(lines)
}

fn render_stages(frame: &mut Frame<'_>, area: Rect, model: &ProgressModel) {
    for ((stage, view), y) in model.stages.iter().zip(area.top()..area.bottom()) {
        let row = Rect {
            y,
            height: 1,
            ..area
        };
        let columns = Layout::horizontal([
            Constraint::Length(15),
            Constraint::Length(2),
            Constraint::Min(10),
        ])
        .split(row);
        frame.render_widget(
            Paragraph::new(format!("{:>12}  {}", stage_name(*stage), state_icon(view))),
            columns[0],
        );
        render_progress_gauge(frame, *stage, view, columns[2], model);
    }
}

fn render_panel(
    frame: &mut Frame<'_>,
    paragraph: Paragraph<'_>,
    title: &str,
    view: Viewport,
    offset: usize,
    color: Color,
) {
    let title = if view.lines > view.height() {
        format!(
            " {title} · {}–{}/{} ",
            offset + 1,
            (offset + view.height()).min(view.lines),
            view.lines
        )
    } else {
        format!(" {title} ")
    };
    frame.render_widget(
        paragraph
            .scroll((u16::try_from(offset).unwrap_or(u16::MAX), 0))
            .block(
                Block::default()
                    .title(title)
                    .borders(Borders::ALL)
                    .padding(Padding::new(1, 1, 0, 0))
                    .border_style(Style::default().fg(color)),
            ),
        view.area,
    );
    if view.lines > view.height() {
        frame.render_stateful_widget(
            Scrollbar::new(ScrollbarOrientation::VerticalRight)
                .begin_symbol(None)
                .end_symbol(None),
            Rect {
                y: view.area.y.saturating_add(1),
                height: view.area.height.saturating_sub(2),
                ..view.area
            },
            &mut ScrollbarState::new(view.max_offset().saturating_add(1))
                .position(offset)
                .viewport_content_length(view.height()),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    use crossterm::event::{KeyEvent, MouseEvent};
    use garmin_progress::{OperationStage, ProgressEventKind, ProgressReporter, ProgressState};
    use ratatui::{Terminal, backend::TestBackend};

    fn key(code: KeyCode) -> Event {
        Event::Key(KeyEvent::new(code, KeyModifiers::NONE))
    }

    #[test]
    fn scrollbar_thumb_reaches_both_ends_of_the_content_range() {
        for (lines, height) in [(17, 5), (9, 2), (9, 5), (20, 8), (100, 8)] {
            let view = Viewport {
                area: Rect::new(0, 0, 40, height + 2),
                lines,
            };
            let text = (0..lines)
                .map(|line| Line::from(line.to_string()))
                .collect::<Vec<_>>();
            let mut terminal = Terminal::new(TestBackend::new(40, height + 2)).expect("terminal");
            for (offset, thumb_row) in [(0, 1), (view.max_offset(), height)] {
                terminal
                    .draw(|frame| {
                        render_panel(
                            frame,
                            Paragraph::new(text.clone()),
                            "Items",
                            view,
                            offset,
                            Color::Gray,
                        );
                    })
                    .expect("panel");
                let buffer = terminal.backend().buffer();
                assert_eq!(
                    buffer[(view.area.right() - 1, thumb_row)].symbol(),
                    ratatui::symbols::scrollbar::DOUBLE_VERTICAL.thumb,
                    "{lines} lines, {height} visible, offset {offset}",
                );
            }
        }
    }

    #[test]
    fn panels_scroll_independently_and_mouse_targets_its_panel() {
        let viewport = DashboardViewport {
            active: Viewport {
                area: Rect::new(1, 8, 78, 6),
                lines: 20,
            },
            history: Viewport {
                area: Rect::new(1, 15, 78, 6),
                lines: 30,
            },
        };
        let mut scroll = DashboardScroll::default();
        scroll.input(&key(KeyCode::PageDown), &viewport, ProgressPhase::Running);
        assert_eq!((scroll.active, scroll.history), (4, 0));
        scroll.input(&key(KeyCode::Tab), &viewport, ProgressPhase::Running);
        scroll.input(&key(KeyCode::Down), &viewport, ProgressPhase::Running);
        assert_eq!((scroll.active, scroll.history), (4, 1));
        scroll.input(
            &Event::Mouse(MouseEvent {
                kind: MouseEventKind::ScrollDown,
                column: 4,
                row: 10,
                modifiers: KeyModifiers::NONE,
            }),
            &viewport,
            ProgressPhase::Running,
        );
        assert_eq!((scroll.active, scroll.history), (5, 1));
        scroll.input(&key(KeyCode::End), &viewport, ProgressPhase::Running);
        assert_eq!(scroll.active, 16);
        scroll.input(&key(KeyCode::Home), &viewport, ProgressPhase::Running);
        assert_eq!(scroll.active, 0);
        assert_eq!(
            scroll.input(&key(KeyCode::Esc), &viewport, ProgressPhase::Running),
            ProgressInput::Abort
        );
        assert_eq!(
            scroll.input(&key(KeyCode::Esc), &viewport, ProgressPhase::Cancelling),
            ProgressInput::Continue
        );
        assert_eq!(
            scroll.input(&key(KeyCode::Enter), &viewport, ProgressPhase::Complete),
            ProgressInput::Close
        );
    }

    #[test]
    fn incoming_history_preserves_scroll_and_resize_clamps_overflow() {
        let (progress, receiver) = ProgressReporter::channel();
        let mut model = ProgressModel::new(&[OperationStage::Download], "Starting");
        for index in 0..8 {
            progress.for_item(index.to_string()).completed(
                OperationStage::Download,
                "Downloaded",
                100,
                Some(100),
            );
        }
        for event in receiver.try_iter() {
            model.apply(&event);
        }
        let mut scroll = DashboardScroll {
            history: 3,
            history_lines_seen: model.history.len(),
            history_rows_seen: model.history_rows_added,
            ..DashboardScroll::default()
        };
        progress.for_item("last").completed_with_path(
            OperationStage::Download,
            "Downloaded",
            "Garmin/last.img",
            100,
            Some(100),
        );
        for event in receiver.try_iter() {
            model.apply(&event);
        }
        scroll.track_history(model.history.len(), model.history_rows_added);
        assert_eq!(scroll.history, 4);
        scroll.track_history(model.history.len(), model.history_rows_added);
        assert_eq!(scroll.history, 4);

        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        terminal
            .draw(|frame| {
                let viewport = render_progress_dashboard(
                    frame,
                    frame.area(),
                    ProgressPresentation {
                        title: "Progress",
                        phase: ProgressPhase::Running,
                    },
                    &model,
                    &mut scroll,
                );
                assert!(scroll.history <= viewport.history.max_offset());
                assert_eq!(scroll.active, 0);
            })
            .unwrap();
        terminal.backend_mut().resize(140, 40);
        terminal.autoresize().unwrap();
        terminal
            .draw(|frame| {
                render_progress_dashboard(
                    frame,
                    frame.area(),
                    ProgressPresentation {
                        title: "Progress",
                        phase: ProgressPhase::Running,
                    },
                    &model,
                    &mut scroll,
                );
            })
            .unwrap();
        assert_eq!(scroll.history, 0);
    }

    #[test]
    fn cached_download_and_verification_share_one_horizontal_history_row() {
        let history = [
            OperationView {
                stage: Some(OperationStage::Download),
                state: Some(ProgressState::Completed),
                kind: ProgressEventKind::CachedDownload,
                label: "Cache hit".to_owned(),
                path: Some("Garmin/map.img".to_owned()),
                recorded_at: Some(std::time::UNIX_EPOCH + Duration::from_secs(45_296)),
                duration: Some(Duration::from_millis(150)),
            },
            OperationView {
                stage: Some(OperationStage::Verify),
                state: Some(ProgressState::Completed),
                kind: ProgressEventKind::CachedChecksumVerified,
                label: "Digest accepted".to_owned(),
                path: Some("Garmin/map.img".to_owned()),
                recorded_at: Some(std::time::UNIX_EPOCH + Duration::from_secs(45_296)),
                duration: Some(Duration::from_millis(60)),
            },
        ];

        let rendered = history_text(history.iter());

        assert_eq!(rendered.lines.len(), 1);
        assert_eq!(
            rendered.lines[0].to_string(),
            format!(
                "✓ | {} | 00:00.21 | Garmin/map.img · cached · MD5 verified",
                crate::history_timestamp(Some(std::time::UNIX_EPOCH + Duration::from_secs(45_296)))
            )
        );
    }
}
