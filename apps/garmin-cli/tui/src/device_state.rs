use garmin_device::{DeviceStateSnapshot, StorageCapacity};
use ratatui::{
    Frame,
    layout::{Alignment, Rect},
    style::{Color, Style},
    widgets::{Gauge, Paragraph, Wrap},
};

pub(super) fn render(
    frame: &mut Frame<'_>,
    area: Rect,
    state: Option<&DeviceStateSnapshot>,
) -> Rect {
    let Some(state) = state else {
        return area;
    };
    let budget = area.height.saturating_sub(8);
    let mut height = 0_u16;
    let mut count = 0_usize;
    for storage in &state.storages {
        let status = if storage.writable == Some(false) {
            " · read-only"
        } else {
            ""
        };
        let label = Paragraph::new(format!("{}{status}", storage.label)).wrap(Wrap { trim: true });
        let label_height = paragraph_height(&label, area.width);
        let detail_height = capacity_height(storage, area.width);
        let storage_height = label_height.saturating_add(detail_height);
        let has_more = count + 1 < state.storages.len();
        let reserve = u16::from(has_more);
        if height
            .saturating_add(storage_height)
            .saturating_add(reserve)
            > budget
        {
            break;
        }
        let row = Rect {
            y: area.y + height,
            height: label_height,
            ..area
        };
        frame.render_widget(label.style(Style::default().fg(Color::Cyan)), row);
        let detail = Rect {
            y: row.y + label_height,
            height: detail_height,
            ..area
        };
        if let Some((total, free)) = storage.capacity.bytes() {
            let used = total - free;
            let percent = if total == 0 {
                0
            } else {
                u16::try_from(u128::from(used) * 100 / u128::from(total)).expect("percentage")
            };
            frame.render_widget(
                Gauge::default()
                    .percent(percent)
                    .gauge_style(
                        Style::default()
                            .fg(Color::Rgb(37, 88, 78))
                            .bg(Color::Rgb(43, 46, 50)),
                    )
                    .label(" ".repeat(usize::from(detail.width))),
                detail,
            );
            frame.render_widget(
                Paragraph::new(format!(
                    "{} used / {} total · {} free",
                    super::decimal_bytes(used),
                    super::decimal_bytes(total),
                    super::decimal_bytes(free)
                ))
                .alignment(Alignment::Center)
                .style(Style::default().fg(Color::Rgb(215, 224, 220))),
                detail,
            );
        } else {
            let reason = match &storage.capacity {
                StorageCapacity::Unavailable { reason } => reason.as_str(),
                StorageCapacity::Available { .. } => "The device reported invalid storage totals",
            };
            frame.render_widget(
                Paragraph::new(format!("Could not read storage usage: {reason}"))
                    .style(Style::default().fg(Color::Yellow))
                    .wrap(Wrap { trim: true }),
                detail,
            );
        }
        height += storage_height;
        count += 1;
    }
    let extra = u16::from(count < state.storages.len());
    if extra != 0 {
        frame.render_widget(
            Paragraph::new(format!("{} more storages", state.storages.len() - count)),
            Rect {
                y: area.y + height,
                height: 1,
                ..area
            },
        );
    }
    Rect {
        y: area.y + height + extra,
        height: area.height.saturating_sub(height + extra),
        ..area
    }
}

fn capacity_height(storage: &garmin_device::DeviceStorageState, width: u16) -> u16 {
    match &storage.capacity {
        StorageCapacity::Available { .. } => 1,
        StorageCapacity::Unavailable { reason } => paragraph_height(
            &Paragraph::new(format!("Could not read storage usage: {reason}"))
                .wrap(Wrap { trim: true }),
            width,
        ),
    }
}

fn paragraph_height(paragraph: &Paragraph<'_>, width: u16) -> u16 {
    u16::try_from(paragraph.line_count(width))
        .unwrap_or(u16::MAX)
        .max(1)
}

pub(super) fn render_update(
    frame: &mut Frame<'_>,
    area: Rect,
    update: Option<&garmin_progress::DeviceStateUpdate>,
) -> Rect {
    match update {
        Some(Ok(state)) => render(frame, area, Some(state)),
        Some(Err(reason)) => {
            let message = Paragraph::new(format!("Could not refresh storage usage: {reason}"))
                .style(Style::default().fg(Color::Yellow))
                .wrap(Wrap { trim: true });
            let height = u16::try_from(message.line_count(area.width))
                .unwrap_or(u16::MAX)
                .min(area.height);
            frame.render_widget(message, Rect { height, ..area });
            Rect {
                y: area.y.saturating_add(height),
                height: area.height.saturating_sub(height),
                ..area
            }
        }
        None => area,
    }
}
