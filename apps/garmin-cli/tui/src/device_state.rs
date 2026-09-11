use garmin_device::{DeviceStateSnapshot, DeviceStorageState, StorageCapacity};
use garmin_i18n::{Intl, format_message};
use ratatui::{
    Frame,
    layout::{Alignment, Rect},
    style::{Color, Style},
    widgets::{Gauge, Paragraph, Wrap},
};

use super::selected_formatter;

pub(super) fn render(
    frame: &mut Frame<'_>,
    area: Rect,
    state: Option<&DeviceStateSnapshot>,
) -> Rect {
    let intl = selected_formatter();
    let Some(state) = state else {
        return area;
    };
    let budget = area.height.saturating_sub(8);
    let mut height = 0_u16;
    let mut count = 0_usize;
    for storage in &state.storages {
        let label = if storage.writable == Some(false) {
            format_message!(
                &intl,
                default_message: "{storage} · read-only",
                values: { storage: storage.label.clone() },
            )
        } else {
            storage.label.clone()
        };
        let label = Paragraph::new(label).wrap(Wrap { trim: true });
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
        render_storage_detail(frame, detail, storage, &intl);
        height += storage_height;
        count += 1;
    }
    let extra = u16::from(count < state.storages.len());
    if extra != 0 {
        frame.render_widget(
            Paragraph::new(format_message!(
                &intl,
                default_message: "{count, plural, one {# more storage} other {# more storages}}",
                values: {
                    count: i64::try_from(state.storages.len() - count).unwrap_or(i64::MAX),
                },
            )),
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

fn render_storage_detail(
    frame: &mut Frame<'_>,
    area: Rect,
    storage: &DeviceStorageState,
    intl: &Intl,
) {
    let Some((total, free)) = storage.capacity.bytes() else {
        let reason = match &storage.capacity {
            StorageCapacity::Unavailable { reason } => reason.clone(),
            StorageCapacity::Available { .. } => format_message!(
                intl,
                default_message: "The device reported invalid storage totals"
            ),
        };
        frame.render_widget(
            Paragraph::new(format_message!(
                intl,
                default_message: "Could not read storage usage: {reason}",
                values: { reason: reason.as_str() },
            ))
            .style(Style::default().fg(Color::Yellow))
            .wrap(Wrap { trim: true }),
            area,
        );
        return;
    };

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
            .label(" ".repeat(usize::from(area.width))),
        area,
    );
    frame.render_widget(
        Paragraph::new(format_message!(
            intl,
            default_message: "{used} used / {total} total · {free} free",
            values: {
                used: super::decimal_bytes(used),
                total: super::decimal_bytes(total),
                free: super::decimal_bytes(free),
            },
        ))
        .alignment(Alignment::Center)
        .style(Style::default().fg(Color::Rgb(215, 224, 220))),
        area,
    );
}

fn capacity_height(storage: &garmin_device::DeviceStorageState, width: u16) -> u16 {
    match &storage.capacity {
        StorageCapacity::Available { .. } => 1,
        StorageCapacity::Unavailable { reason } => {
            let intl = selected_formatter();
            paragraph_height(
                &Paragraph::new(format_message!(
                    &intl,
                    default_message: "Could not read storage usage: {reason}",
                    values: { reason: reason.as_str() },
                ))
                .wrap(Wrap { trim: true }),
                width,
            )
        }
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
            let intl = selected_formatter();
            let message = Paragraph::new(format_message!(
                &intl,
                default_message: "Could not refresh storage usage: {reason}",
                values: { reason: reason.as_str() },
            ))
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
