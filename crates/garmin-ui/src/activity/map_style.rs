use egui::{Color32, Rect};

pub(super) const ROUTE_WIDTH: f32 = 3.0;
pub(super) const ROUTE_OUTLINE_WIDTH: f32 = 5.0;
pub(super) const HIGHLIGHT_WIDTH: f32 = 4.5;
pub(super) const HIGHLIGHT_OUTLINE_WIDTH: f32 = 6.5;
pub(super) const DIMMED_ROUTE_OPACITY: f32 = 0.32;
pub(super) const MINIMUM_OUTLINE_OPACITY: f32 = 0.6;

pub(super) const SPEED_COLORS: [[u8; 3]; 5] = [
    [45, 132, 255],
    [32, 184, 177],
    [83, 190, 91],
    [244, 190, 52],
    [235, 72, 67],
];

pub(super) fn clip_rect(viewport: Rect, inherited: Rect) -> Rect {
    viewport.intersect(inherited)
}

#[expect(
    clippy::cast_possible_truncation,
    reason = "the normalized speed fraction is explicitly clamped to the f32 range"
)]
pub(super) fn speed_fraction(
    start: Option<f64>,
    end: Option<f64>,
    bounds: Option<(f64, f64)>,
) -> Option<f32> {
    let (minimum, maximum) = bounds?;
    let speed = match (start, end) {
        (Some(start), Some(end)) => f64::midpoint(start, end),
        (Some(speed), None) | (None, Some(speed)) => speed,
        (None, None) => return None,
    };
    let span = maximum - minimum;
    if !speed.is_finite() || !span.is_finite() || span <= 0.0 {
        return None;
    }
    Some(((speed - minimum) / span).clamp(0.0, 1.0) as f32)
}

#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "interpolation between u8 color stops remains in the inclusive u8 range"
)]
pub(super) fn speed_color(fraction: f32) -> Color32 {
    let position = fraction.clamp(0.0, 1.0) * 4.0;
    let (first, second, blend) = if position < 1.0 {
        (0, 1, position)
    } else if position < 2.0 {
        (1, 2, position - 1.0)
    } else if position < 3.0 {
        (2, 3, position - 2.0)
    } else {
        (3, 4, position - 3.0)
    };
    let channels: [u8; 3] = std::array::from_fn(|channel| {
        f32::from(SPEED_COLORS[first][channel])
            .mul_add(
                1.0 - blend,
                f32::from(SPEED_COLORS[second][channel]) * blend,
            )
            .round() as u8
    });
    Color32::from_rgb(channels[0], channels[1], channels[2])
}

pub(super) fn speed_colors_uniform() -> [[f32; 4]; SPEED_COLORS.len()] {
    SPEED_COLORS.map(|color| {
        [
            f32::from(color[0]) / 255.0,
            f32::from(color[1]) / 255.0,
            f32::from(color[2]) / 255.0,
            1.0,
        ]
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn software_speed_ramp_hits_every_shared_gpu_stop() {
        for (fraction, color) in [
            (0.0, SPEED_COLORS[0]),
            (0.25, SPEED_COLORS[1]),
            (0.5, SPEED_COLORS[2]),
            (0.75, SPEED_COLORS[3]),
            (1.0, SPEED_COLORS[4]),
        ] {
            assert_eq!(
                speed_color(fraction),
                Color32::from_rgb(color[0], color[1], color[2])
            );
        }
    }

    #[test]
    fn clipping_is_the_intersection_for_every_scene_painter() {
        let viewport = Rect::from_min_max(egui::pos2(10.0, 20.0), egui::pos2(110.0, 120.0));
        let inherited = Rect::from_min_max(egui::pos2(0.0, 40.0), egui::pos2(90.0, 200.0));

        assert_eq!(
            clip_rect(viewport, inherited),
            Rect::from_min_max(egui::pos2(10.0, 40.0), egui::pos2(90.0, 120.0))
        );
    }
}
