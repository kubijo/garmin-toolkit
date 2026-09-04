use egui::{RichText, TextStyle, TextWrapMode, Ui, WidgetText};
use std::borrow::Cow;

pub(crate) fn balanced<'a>(
    ui: &Ui,
    text: &'a str,
    style: &TextStyle,
    strong: bool,
) -> Cow<'a, str> {
    let available = ui.available_width();
    if width(ui, text, style, strong) <= available {
        return Cow::Borrowed(text);
    }

    let mut best = None;
    for (index, character) in text.char_indices() {
        if !character.is_whitespace() {
            continue;
        }
        let left = text[..index].trim_end();
        let right = text[index..].trim_start();
        let left_width = width(ui, left, style, strong);
        let right_width = width(ui, right, style, strong);
        if left_width > available || right_width > available {
            continue;
        }
        let imbalance = (left_width - right_width).abs();
        if best.is_none_or(|(_, _, score)| imbalance < score) {
            best = Some((left, right, imbalance));
        }
    }

    best.map_or(Cow::Borrowed(text), |(left, right, _)| {
        Cow::Owned(format!("{left}\n{right}"))
    })
}

fn width(ui: &Ui, text: &str, style: &TextStyle, strong: bool) -> f32 {
    let rich = if strong {
        RichText::new(text).strong()
    } else {
        RichText::new(text)
    };
    WidgetText::from(rich)
        .into_galley(ui, Some(TextWrapMode::Extend), f32::INFINITY, style.clone())
        .size()
        .x
}
