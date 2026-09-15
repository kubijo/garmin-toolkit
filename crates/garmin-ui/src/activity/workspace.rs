//! Stateful, linked activity analysis workspace.

#![expect(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "plot coordinates and pixel-sized render budgets intentionally use f64/f32/usize"
)]

use std::{collections::HashMap, sync::Arc};

use cint::ColorInterop;
use egui::{Align, Id, Key, Layout, RichText, ScrollArea, Sense, Ui, Vec2};
use egui_plot::{Line, Plot, PlotPoint, Points, Span, VLine};
use garmin_color::theme;
use garmin_i18n::{Intl, format_message};
use garmin_model::{activity::ActivitySport, identity::UnitSystem};
use garmin_service_api::{
    ActivityRecordingSnapshot, ActivitySampleSnapshot, ActivityTimerStateSnapshot,
};

use super::map;
use super::map::ActivityMap;
use super::{Action, ItemProps, ListProps, MetricProps, Presentation, list};
use crate::{
    Size, button, icons,
    theme::{CONTROL_RADIUS, PANEL_RADIUS, color32},
};

const COMPACT_BREAKPOINT: f32 = 760.0;
const LIST_WIDTH: f32 = 232.0;
const DETAILS_WIDTH: f32 = 248.0;
const CHART_HEIGHT: f32 = 112.0;
const PLAYBACK_SECONDS: f64 = 30.0;

/// Inputs shared by desktop, HASS, and embedded FIT preview workspaces.
pub struct WorkspaceProps<'a> {
    pub items: &'a [ItemProps<'a>],
    pub presentations: &'a [Presentation],
    pub selected: Option<usize>,
    pub recording: Option<&'a ActivityRecordingSnapshot>,
    pub recording_key: Option<&'a str>,
    pub units: UnitSystem,
    pub empty_list: &'a str,
    pub empty_detail: &'a str,
    pub no_route: &'a str,
}

/// Stateful activity workspace. Hosts retain one instance for the life of their view.
#[derive(Default)]
pub struct Workspace {
    viewer: Viewer,
    compact_panel: Option<CompactPanel>,
}

#[derive(Clone, Copy)]
enum PaneSurface {
    Workspace,
    Sidebar,
    Details,
}

#[derive(Clone, Copy)]
struct ResizeStrokeWidths {
    hovered: f32,
    active: f32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CompactPanel {
    Activities,
    Details,
}

impl Workspace {
    /// Return the sample selection shared by the map, charts, readouts, and laps.
    #[must_use]
    pub const fn cursor(&self) -> ActivityCursor {
        self.viewer.cursor()
    }

    /// Set the shared cursor from a host interaction or deterministic presentation.
    pub fn set_cursor(&mut self, cursor: ActivityCursor) {
        self.viewer.set_cursor(cursor);
    }

    /// Return the lap currently constraining map, charts, and playback.
    #[must_use]
    pub const fn selected_lap(&self) -> Option<usize> {
        self.viewer.selected_lap()
    }

    /// Set the lap range from a host interaction or deterministic presentation.
    pub fn set_selected_lap(&mut self, selected_lap: Option<usize>) {
        self.viewer.set_selected_lap(selected_lap);
    }

    /// Drain newly requested vector tiles for a host-owned fetch queue.
    pub fn take_map_tile_requests(&mut self) -> Vec<super::MapTileRequest> {
        self.viewer.map.take_requests()
    }

    /// Resolve a host tile result into the decoded UI cache.
    pub fn resolve_map_tile(&mut self, context: &egui::Context, response: super::MapTileResponse) {
        self.viewer.map.resolve(context, response);
    }

    /// Render the workspace and return activity-list selection changes.
    #[must_use]
    pub fn show(&mut self, ui: &mut Ui, intl: &Intl, props: &WorkspaceProps<'_>) -> Option<Action> {
        if ui.available_width() >= COMPACT_BREAKPOINT {
            self.compact_panel = None;
            self.show_wide(ui, intl, props)
        } else {
            self.show_compact(ui, intl, props)
        }
    }

    fn show_wide(
        &mut self,
        ui: &mut Ui,
        intl: &Intl,
        props: &WorkspaceProps<'_>,
    ) -> Option<Action> {
        let mut action = None;
        let resize_strokes = suppress_resize_strokes(ui);
        egui::Panel::left(Id::new((ui.id(), "activity-list")))
            .resizable(true)
            .show_separator_line(false)
            .default_size(LIST_WIDTH)
            .size_range(180.0..=360.0)
            .frame(pane_frame(ui, PaneSurface::Sidebar))
            .show(ui, |ui| {
                restore_resize_strokes(ui, resize_strokes);
                ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        action = list(ui, &list_props(props));
                    });
            });
        restore_resize_strokes(ui, resize_strokes);

        let resize_strokes = suppress_resize_strokes(ui);
        egui::Panel::right(Id::new((ui.id(), "activity-details")))
            .resizable(true)
            .show_separator_line(false)
            .default_size(DETAILS_WIDTH)
            .size_range(200.0..=420.0)
            .frame(pane_frame(ui, PaneSurface::Details))
            .show(ui, |ui| {
                restore_resize_strokes(ui, resize_strokes);
                self.show_details(ui, intl, props);
            });
        restore_resize_strokes(ui, resize_strokes);

        egui::CentralPanel::default()
            .frame(pane_frame(ui, PaneSurface::Workspace))
            .show(ui, |ui| {
                self.show_viewer(ui, intl, props);
            });
        action
    }

    fn show_compact(
        &mut self,
        ui: &mut Ui,
        intl: &Intl,
        props: &WorkspaceProps<'_>,
    ) -> Option<Action> {
        let activities = format_message!(intl, default_message: "Activities");
        let details = format_message!(intl, default_message: "Details");
        if ui.input(|input| input.key_pressed(Key::Escape)) {
            self.compact_panel = None;
        }
        ui.horizontal(|ui| {
            if (button::Props {
                label: &activities,
                icon: Some(icons::ACTIVITY),
                kind: button::Kind::Tertiary,
                size: Size::Small,
                width: button::Width::Fit,
                enabled: true,
            })
            .show(ui)
            .clicked()
            {
                self.compact_panel = toggle_panel(self.compact_panel, CompactPanel::Activities);
            }
            if (button::Props {
                label: &details,
                icon: Some(icons::INFO),
                kind: button::Kind::Tertiary,
                size: Size::Small,
                width: button::Width::Fit,
                enabled: props.selected.is_some(),
            })
            .show(ui)
            .clicked()
            {
                self.compact_panel = toggle_panel(self.compact_panel, CompactPanel::Details);
            }
        });
        ui.add_space(8.0);
        let drawer_rect = ui.available_rect_before_wrap();
        self.show_viewer(ui, intl, props);

        let mut action = None;
        if let Some(panel) = self.compact_panel {
            let drawer_width = drawer_rect.width().min(336.0);
            let position = match panel {
                CompactPanel::Activities => drawer_rect.left_top(),
                CompactPanel::Details => {
                    egui::pos2(drawer_rect.right() - drawer_width, drawer_rect.top())
                }
            };
            egui::Area::new(ui.id().with("activity-workspace-drawer"))
                .order(egui::Order::Foreground)
                .fixed_pos(position)
                .show(ui.ctx(), |ui| {
                    ui.set_width(drawer_width);
                    egui::Frame::new()
                        .fill(color32(
                            crate::theme::palette(ui)
                                .surfaces()
                                .layer(theme::Level::Two),
                        ))
                        .stroke(egui::Stroke::new(
                            1.0,
                            color32(crate::theme::palette(ui).borders().strong()),
                        ))
                        .shadow(egui::epaint::Shadow {
                            offset: [0, 6],
                            blur: 20,
                            spread: 2,
                            color: egui::Color32::from_black_alpha(120),
                        })
                        .inner_margin(12.0)
                        .show(ui, |ui| match panel {
                            CompactPanel::Activities => {
                                ScrollArea::vertical()
                                    .max_height(drawer_rect.height())
                                    .show(ui, |ui| {
                                        action = list(ui, &list_props(props));
                                    });
                            }
                            CompactPanel::Details => {
                                ui.set_max_height(drawer_rect.height());
                                self.show_details(ui, intl, props);
                            }
                        });
                });
        }
        if action.is_some() {
            self.compact_panel = None;
        }
        action
    }

    fn show_viewer(&mut self, ui: &mut Ui, intl: &Intl, props: &WorkspaceProps<'_>) {
        let Some(selected) = props.selected else {
            empty_panel(ui, props.empty_detail);
            return;
        };
        let Some(presentation) = props.presentations.get(selected) else {
            empty_panel(ui, props.empty_detail);
            return;
        };
        let Some(recording) = props.recording else {
            loading_panel(ui, intl);
            return;
        };
        self.viewer.show(
            ui,
            intl,
            &ViewerProps {
                presentation,
                recording,
                recording_key: props.recording_key.unwrap_or("activity"),
                units: props.units,
                no_route: props.no_route,
            },
        );
    }

    fn show_details(&mut self, ui: &mut Ui, intl: &Intl, props: &WorkspaceProps<'_>) {
        let Some(selected) = props.selected else {
            empty_panel(ui, props.empty_detail);
            return;
        };
        let Some(presentation) = props.presentations.get(selected) else {
            empty_panel(ui, props.empty_detail);
            return;
        };
        ScrollArea::vertical().show(ui, |ui| {
            summary(ui, presentation);
            let Some(recording) = props.recording else {
                return;
            };
            self.viewer
                .sample_details(ui, intl, recording, presentation.sport(), props.units);
            self.viewer.laps(ui, intl, recording, props.units);
        });
    }
}

fn suppress_resize_strokes(ui: &mut Ui) -> ResizeStrokeWidths {
    let widgets = &mut ui.visuals_mut().widgets;
    let widths = ResizeStrokeWidths {
        hovered: widgets.hovered.fg_stroke.width,
        active: widgets.active.fg_stroke.width,
    };
    widgets.hovered.fg_stroke.width = 0.0;
    widgets.active.fg_stroke.width = 0.0;
    widths
}

fn restore_resize_strokes(ui: &mut Ui, widths: ResizeStrokeWidths) {
    let widgets = &mut ui.visuals_mut().widgets;
    widgets.hovered.fg_stroke.width = widths.hovered;
    widgets.active.fg_stroke.width = widths.active;
}

fn pane_frame(ui: &Ui, surface: PaneSurface) -> egui::Frame {
    let palette = crate::theme::palette(ui);
    let fill = match surface {
        PaneSurface::Workspace => palette.surfaces().background(),
        PaneSurface::Sidebar | PaneSurface::Details => palette.surfaces().layer(theme::Level::One),
    };
    let inner_margin = match surface {
        PaneSurface::Workspace | PaneSurface::Sidebar => egui::Margin::ZERO,
        PaneSurface::Details => egui::Margin::same(12),
    };
    egui::Frame::new()
        .fill(color32(fill))
        .inner_margin(inner_margin)
}

fn toggle_panel(current: Option<CompactPanel>, requested: CompactPanel) -> Option<CompactPanel> {
    if current == Some(requested) {
        None
    } else {
        Some(requested)
    }
}

fn list_props<'a>(props: &'a WorkspaceProps<'a>) -> ListProps<'a> {
    ListProps {
        items: props.items,
        selected: props.selected,
        empty: props.empty_list,
    }
}

/// Inputs for a reusable single-activity viewer.
pub struct ViewerProps<'a> {
    pub presentation: &'a Presentation,
    pub recording: &'a ActivityRecordingSnapshot,
    pub recording_key: &'a str,
    pub units: UnitSystem,
    pub no_route: &'a str,
}

/// Shared activity cursor mode.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum CursorMode {
    #[default]
    Idle,
    Hover,
    Pinned,
    Playback,
}

/// Authoritative selection consumed by route, charts, readouts, and laps.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ActivityCursor {
    pub sample_index: Option<usize>,
    pub mode: CursorMode,
}

/// Stateful single-activity analysis view.
pub struct Viewer {
    recording_key: String,
    cursor: ActivityCursor,
    axis: Axis,
    selected_lap: Option<usize>,
    hovered_lap: Option<usize>,
    playing: bool,
    playback_speed: PlaybackSpeed,
    playback_position: Option<f64>,
    domain_cache: Option<(UnitSystem, Domain)>,
    chart_cache: HashMap<ChartCacheKey, Arc<[Vec<PlotPoint>]>>,
    map: ActivityMap,
}

impl Default for Viewer {
    fn default() -> Self {
        Self {
            recording_key: String::new(),
            cursor: ActivityCursor::default(),
            axis: Axis::Distance,
            selected_lap: None,
            hovered_lap: None,
            playing: false,
            playback_speed: PlaybackSpeed::Normal,
            playback_position: None,
            domain_cache: None,
            chart_cache: HashMap::new(),
            map: ActivityMap::default(),
        }
    }
}

impl Viewer {
    /// Return the authoritative sample-index cursor.
    #[must_use]
    pub const fn cursor(&self) -> ActivityCursor {
        self.cursor
    }

    /// Set the authoritative sample-index cursor.
    ///
    /// Playback begins from the supplied sample on the next rendered frame. Idle, hover, and
    /// pinned states stop any active playback.
    pub fn set_cursor(&mut self, cursor: ActivityCursor) {
        self.cursor = cursor;
        self.playing = cursor.mode == CursorMode::Playback && cursor.sample_index.is_some();
        self.playback_position = None;
    }

    /// Return the lap currently constraining map, charts, and playback.
    #[must_use]
    pub const fn selected_lap(&self) -> Option<usize> {
        self.selected_lap
    }

    /// Set the lap range from a host interaction or deterministic presentation.
    pub fn set_selected_lap(&mut self, selected_lap: Option<usize>) {
        self.selected_lap = selected_lap;
        self.hovered_lap = None;
        self.playing = false;
        self.playback_position = None;
    }

    pub fn show(&mut self, ui: &mut Ui, intl: &Intl, props: &ViewerProps<'_>) {
        self.sync_recording(props.recording_key);
        if self
            .cursor
            .sample_index
            .is_some_and(|index| index >= props.recording.samples.len())
        {
            self.playing = false;
            self.clear_cursor();
        }
        if self
            .selected_lap
            .is_some_and(|index| index >= props.recording.laps.len())
        {
            self.selected_lap = None;
        }
        if ui.input(|input| input.key_pressed(Key::Escape)) {
            self.playing = false;
            self.clear_cursor();
        }

        if self
            .domain_cache
            .as_ref()
            .is_none_or(|(units, _domain)| *units != props.units)
        {
            self.domain_cache = Some((props.units, Domain::new(props.recording, props.units)));
        }
        let domain = self.domain_cache.as_ref().map_or_else(
            || Domain::new(props.recording, props.units),
            |(_units, domain)| domain.clone(),
        );
        if self.axis == Axis::Distance && !domain.distance_available {
            self.axis = Axis::Elapsed;
        }
        self.advance_playback(ui, props.recording);

        let background = ui.interact(
            ui.available_rect_before_wrap(),
            ui.id().with("activity-analysis-background"),
            Sense::CLICK,
        );
        ScrollArea::vertical().show(ui, |ui| {
            viewer_inset().show(ui, |ui| {
                self.header(ui, intl, props, &domain);
            });
            ui.add_space(8.0);
            let map_height = (ui.available_height() * 0.55).clamp(260.0, 420.0);
            let route_output = self.route(ui, intl, props.recording, props.no_route, map_height);
            let mut hover_seen = route_output.hovered.is_some();
            if route_output.empty_clicked {
                self.playing = false;
                self.clear_cursor();
                ui.ctx().request_repaint();
            } else if self.apply_pointer(route_output.hovered, route_output.clicked) {
                ui.ctx().request_repaint();
            }
            ui.add_space(8.0);
            hover_seen |= viewer_inset()
                .show(ui, |ui| self.charts(ui, intl, props, &domain))
                .inner;
            if !hover_seen && self.cursor.mode == CursorMode::Hover {
                self.clear_cursor();
                ui.ctx().request_repaint();
            }
        });
        if background.clicked() {
            self.playing = false;
            self.clear_cursor();
            ui.ctx().request_repaint();
        }
    }

    fn sync_recording(&mut self, key: &str) {
        if self.recording_key == key {
            return;
        }
        self.recording_key.clear();
        self.recording_key.push_str(key);
        self.cursor = ActivityCursor::default();
        self.axis = Axis::Distance;
        self.selected_lap = None;
        self.hovered_lap = None;
        self.playing = false;
        self.playback_position = None;
        self.domain_cache = None;
        self.chart_cache.clear();
    }

    fn header(&mut self, ui: &mut Ui, intl: &Intl, props: &ViewerProps<'_>, domain: &Domain) {
        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                ui.label(RichText::new(&props.presentation.title).size(18.0).strong());
                ui.label(
                    RichText::new(&props.presentation.subtitle)
                        .small()
                        .color(color32(
                            crate::theme::palette(ui).content().text_secondary(),
                        )),
                );
            });
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let elapsed = format_message!(intl, default_message: "Time");
                let distance = format_message!(intl, default_message: "Distance");
                let choices = [
                    button::GroupChoice::new(&distance, icons::ROUTE, Axis::Distance)
                        .enabled(domain.distance_available),
                    button::GroupChoice::new(&elapsed, icons::ACTIVITY, Axis::Elapsed),
                ];
                if let Some(axis) = button::compact_group(
                    ui,
                    self.axis,
                    &choices,
                    button::GroupProps {
                        size: Size::Small,
                        enabled: true,
                    },
                ) {
                    self.axis = axis;
                }
            });
        });
    }

    fn route(
        &mut self,
        ui: &mut Ui,
        intl: &Intl,
        recording: &ActivityRecordingSnapshot,
        no_route: &str,
        height: f32,
    ) -> map::Output {
        let range = self.sample_range(recording);
        let selected_coordinate = selected_route_coordinate(
            recording,
            self.cursor,
            self.playback_position,
            range.clone(),
        );
        let fit_key = format!("{}:{:?}", self.recording_key, self.selected_lap);
        let background_unavailable =
            format_message!(intl, default_message: "Map background unavailable");
        let highlighted_range = self
            .hovered_lap
            .and_then(|lap| lap_sample_range(recording, lap));
        let mut output = self.map.show(
            ui,
            &map::Props {
                recording,
                selected_coordinate,
                sample_range: range,
                highlighted_range,
                fit_key: &fit_key,
                empty: no_route,
                background_unavailable: &background_unavailable,
                height,
            },
        );
        let controls = self.map_controls(ui, intl, recording, output.rect);
        if ui
            .ctx()
            .pointer_hover_pos()
            .is_some_and(|pointer| controls.iter().any(|rect| rect.contains(pointer)))
        {
            output.hovered = None;
            output.clicked = None;
            output.empty_clicked = false;
        }
        output
    }

    fn map_controls(
        &mut self,
        ui: &mut Ui,
        intl: &Intl,
        recording: &ActivityRecordingSnapshot,
        map_rect: egui::Rect,
    ) -> Vec<egui::Rect> {
        const INSET: f32 = 12.0;
        const BUTTON: f32 = 32.0;
        const FRAME: f32 = BUTTON + 4.0;
        const GAP: f32 = 2.0;

        let mut controls = Vec::with_capacity(4);
        let fit_route = format_message!(intl, default_message: "Fit route");
        let fit_bounds = egui::Rect::from_min_size(
            map_rect.left_top() + egui::vec2(INSET, INSET),
            egui::Vec2::splat(FRAME),
        );
        let fit = floating_control(ui, fit_bounds, Layout::left_to_right(Align::Center), |ui| {
            floating_icon_button(ui, &fit_route, icons::TARGET, false, true)
        });
        if fit.inner.clicked() {
            self.map.fit();
            ui.ctx().request_repaint();
        }
        controls.push(fit.response.rect);

        let zoom_bounds = egui::Rect::from_min_size(
            egui::pos2(map_rect.right() - INSET - FRAME, map_rect.top() + INSET),
            egui::vec2(FRAME, BUTTON.mul_add(2.0, GAP) + 4.0),
        );
        let zoom = floating_control(ui, zoom_bounds, Layout::top_down(Align::Center), |ui| {
            ui.spacing_mut().item_spacing.y = GAP;
            let zoom_in = format_message!(intl, default_message: "Zoom in");
            let zoom_in = floating_icon_button(ui, &zoom_in, icons::PLUS, false, true);
            let zoom_out = format_message!(intl, default_message: "Zoom out");
            let zoom_out = floating_icon_button(ui, &zoom_out, icons::MINUS, false, true);
            paint_horizontal_control_separator(ui, zoom_in.rect, GAP);
            (zoom_in, zoom_out)
        });
        if zoom.inner.0.clicked() {
            self.map.zoom_in();
            ui.ctx().request_repaint();
        }
        if zoom.inner.1.clicked() {
            self.map.zoom_out();
            ui.ctx().request_repaint();
        }
        controls.push(zoom.response.rect);

        let can_play = recording.samples.len() > 1;
        let play_label = if self.playing {
            format_message!(intl, default_message: "Pause")
        } else {
            format_message!(intl, default_message: "Play")
        };
        let icon = if self.playing {
            icons::PAUSE
        } else {
            icons::PLAY
        };
        let playback_width = 6.0 + BUTTON + GAP + 44.0;
        let playback_bounds = egui::Rect::from_min_size(
            egui::pos2(
                map_rect.center().x - playback_width / 2.0,
                map_rect.bottom() - INSET - FRAME,
            ),
            egui::vec2(playback_width, FRAME),
        );
        let speed_tooltip = format!(
            "{}: {}",
            format_message!(intl, default_message: "Playback speed"),
            self.playback_speed.label()
        );
        let playback = floating_control(
            ui,
            playback_bounds,
            Layout::left_to_right(Align::Center),
            |ui| {
                ui.spacing_mut().item_spacing.x = GAP;
                let play = floating_icon_button(ui, &play_label, icon, self.playing, can_play);
                let speed =
                    floating_text_button(ui, self.playback_speed.label(), 44.0, false, can_play)
                        .on_hover_text(&speed_tooltip);
                paint_vertical_control_separator(ui, play.rect, GAP);
                (play, speed)
            },
        );
        if playback.inner.0.clicked() {
            if self.playing {
                self.playing = false;
                self.cursor.mode = CursorMode::Pinned;
            } else {
                self.start_playback(recording);
            }
            ui.ctx().request_repaint();
        }
        if playback.inner.1.clicked() {
            self.playback_speed = self.playback_speed.next();
            ui.ctx().request_repaint();
        }
        controls.push(playback.response.rect);

        if let Some(full_activity) = self.full_activity_control(ui, intl, map_rect, INSET, FRAME) {
            controls.push(full_activity);
        }

        controls
    }

    fn full_activity_control(
        &mut self,
        ui: &mut Ui,
        intl: &Intl,
        map_rect: egui::Rect,
        inset: f32,
        frame_size: f32,
    ) -> Option<egui::Rect> {
        self.selected_lap?;
        let full_activity = format_message!(intl, default_message: "Full activity");
        let bounds = egui::Rect::from_min_size(
            egui::pos2(
                map_rect.left() + inset,
                map_rect.bottom() - inset - frame_size,
            ),
            egui::vec2(116.0, frame_size),
        );
        let control = floating_control(ui, bounds, Layout::left_to_right(Align::Center), |ui| {
            floating_labeled_button(ui, &full_activity, icons::TARGET, 112.0, false, true)
        });
        if control.inner.clicked() {
            self.selected_lap = None;
            self.playback_position = None;
            self.map.fit();
            ui.ctx().request_repaint();
        }
        Some(control.response.rect)
    }

    #[expect(
        clippy::too_many_lines,
        reason = "each chart must share one cursor, domain, bounds group, and lap overlay in the same render loop"
    )]
    fn charts(
        &mut self,
        ui: &mut Ui,
        intl: &Intl,
        props: &ViewerProps<'_>,
        domain: &Domain,
    ) -> bool {
        let visible = ChartKind::ALL
            .iter()
            .copied()
            .filter(|kind| kind.has_values(props.recording, props.presentation.sport()))
            .collect::<Vec<_>>();
        if visible.is_empty() {
            return false;
        }
        let range = self.sample_range(props.recording);
        let x_values = domain.values(self.axis);
        let x_bounds = domain.bounds(self.axis, range.clone());
        let linked = ui.id().with(("activity-chart-axis", &self.recording_key));
        let mut hovered = None;
        let mut clicked = None;
        let palette = crate::theme::palette(ui);
        let accent = color32(crate::theme::selection_accent(ui));
        let guide = color32(palette.content().icon_secondary());
        let plot_surface = color32(palette.surfaces().layer(theme::Level::One));
        let plot_field = color32(palette.surfaces().background_hover());
        let plot_grid = color32(palette.borders().subtle()).gamma_multiply(0.42);
        for (chart_index, kind) in visible.iter().copied().enumerate() {
            let label = kind.label(intl, props.presentation.sport());
            let chart_color = kind.color(ui);
            let stats = chart_stats(
                props.recording,
                range.clone(),
                kind,
                props.presentation.sport(),
                props.units,
            )
            .expect("a visible chart has at least one measurement");
            let baseline = kind.fill_baseline(stats, props.presentation.sport());
            let value = self
                .cursor
                .sample_index
                .and_then(|index| props.recording.samples.get(index))
                .and_then(|sample| kind.value(sample, props.presentation.sport(), props.units))
                .map(|value| kind.format_value(value, props.presentation.sport(), props.units));
            let max_points = (ui.available_width().max(64.0) * 2.0) as usize;
            let cache_key = ChartCacheKey {
                axis: self.axis,
                kind,
                sport: props.presentation.sport(),
                units: props.units,
                max_points,
            };
            let lines = if let Some(lines) = self.chart_cache.get(&cache_key) {
                Arc::clone(lines)
            } else {
                if self.chart_cache.len() >= ChartKind::ALL.len() * 4 {
                    self.chart_cache.clear();
                }
                let lines: Arc<[Vec<PlotPoint>]> = chart_lines(
                    props.recording,
                    x_values,
                    kind,
                    props.presentation.sport(),
                    props.units,
                    max_points,
                )
                .into();
                self.chart_cache.insert(cache_key, Arc::clone(&lines));
                lines
            };
            let selected = self.cursor.sample_index.and_then(|index| {
                let x = *x_values.get(index)?;
                let y = kind.value(
                    props.recording.samples.get(index)?,
                    props.presentation.sport(),
                    props.units,
                )?;
                Some([x, y])
            });
            let lap_bounds = self
                .hovered_lap
                .or(self.selected_lap)
                .and_then(|lap| lap_x_bounds(props.recording, x_values, lap));
            let axis = self.axis;
            let units = props.units;
            let sport = props.presentation.sport();
            let output = egui::Frame::new()
                .fill(plot_surface)
                .inner_margin(12)
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(&label).size(14.0).strong());
                        if let Some(value) = &value {
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                ui.label(RichText::new(value).color(chart_color).strong());
                            });
                        }
                    });
                    ui.add_space(8.0);
                    chart_summary(ui, intl, kind, stats, sport, units);
                    ui.add_space(8.0);
                    egui::Frame::new()
                        .fill(plot_field)
                        .inner_margin(egui::Margin {
                            left: 24,
                            right: 24,
                            top: 4,
                            bottom: 4,
                        })
                        .show(ui, |ui| {
                            Plot::new(ui.id().with(("activity-chart", chart_index)))
                                .height(CHART_HEIGHT)
                                .allow_drag(false)
                                .allow_axis_zoom_drag(false)
                                .allow_scroll(false)
                                .allow_zoom(false)
                                .allow_boxed_zoom(false)
                                .allow_double_click_reset(false)
                                .show_crosshair(false)
                                .show_background(false)
                                .show_grid(egui::Vec2b::new(true, true))
                                .show_x(false)
                                .show_y(false)
                                .show_axes(egui::Vec2b::new(true, false))
                                .grid_color(plot_grid)
                                .grid_fade(0.75)
                                .include_y(baseline)
                                .invert_y(
                                    kind == ChartKind::PaceSpeed && sport == ActivitySport::Running,
                                )
                                .x_axis_formatter(move |mark, _bounds| {
                                    format_domain_tick(mark.value, axis, units)
                                })
                                .link_axis(linked, egui::Vec2b::new(true, false))
                                .show(ui, |plot_ui| {
                                    plot_ui.set_plot_bounds_x(x_bounds.clone());
                                    if let Some((start, end)) = lap_bounds {
                                        plot_ui.span(
                                            Span::new("lap interval", start..=end)
                                                .fill(accent.gamma_multiply(0.12)),
                                        );
                                        plot_ui.vline(VLine::new("lap start", start).color(guide));
                                        plot_ui.vline(VLine::new("lap end", end).color(guide));
                                    }
                                    for (segment_index, points) in lines.iter().enumerate() {
                                        plot_ui.line(
                                            Line::new(
                                                format!("{label}-{segment_index}"),
                                                points.as_slice(),
                                            )
                                            .color(chart_color)
                                            .width(1.5)
                                            .fill(baseline as f32)
                                            .fill_alpha(0.32),
                                        );
                                    }
                                    if let Some(index) = self.cursor.sample_index
                                        && let Some(x) = x_values.get(index)
                                    {
                                        plot_ui.vline(
                                            VLine::new("sample cursor", *x).color(guide).width(1.0),
                                        );
                                    }
                                    if let Some([x, y]) = selected {
                                        plot_ui.points(
                                            Points::new("selected sample", vec![[x, y]])
                                                .color(chart_color)
                                                .radius(4.0),
                                        );
                                    }
                                    plot_ui
                                        .response()
                                        .hovered()
                                        .then(|| plot_ui.pointer_coordinate().map(|point| point.x))
                                        .flatten()
                                })
                        })
                        .inner
                })
                .inner;
            if let Some(x) = output.inner {
                let index = nearest_index(x_values, x, range.clone());
                hovered = index;
                if output.response.clicked()
                    || output.response.dragged_by(egui::PointerButton::Primary)
                {
                    clicked = index;
                }
            }
            ui.add_space(8.0);
        }
        if self.apply_pointer(hovered, clicked) {
            ui.ctx().request_repaint();
        }
        hovered.is_some()
    }

    fn apply_pointer(&mut self, hovered: Option<usize>, clicked: Option<usize>) -> bool {
        let next = if let Some(index) = clicked {
            self.playing = false;
            self.playback_position = None;
            Some(ActivityCursor {
                sample_index: Some(index),
                mode: CursorMode::Pinned,
            })
        } else {
            hovered
                .filter(|_| !self.playing)
                .map(|index| ActivityCursor {
                    sample_index: Some(index),
                    mode: CursorMode::Hover,
                })
        };
        if let Some(next) = next
            && self.cursor != next
        {
            self.cursor = next;
            true
        } else {
            false
        }
    }

    fn clear_cursor(&mut self) {
        self.cursor = ActivityCursor::default();
        self.playback_position = None;
    }

    fn start_playback(&mut self, recording: &ActivityRecordingSnapshot) {
        let range = self.sample_range(recording);
        let active = active_positions(recording);
        let start = active.get(*range.start()).copied().unwrap_or(0.0);
        let end = active.get(*range.end()).copied().unwrap_or(start);
        let current = self
            .playback_position
            .or_else(|| {
                self.cursor
                    .sample_index
                    .and_then(|index| active.get(index).copied())
            })
            .unwrap_or(start);
        let position = if current >= end {
            start
        } else {
            current.max(start)
        };
        self.playback_position = Some(position);
        self.cursor = ActivityCursor {
            sample_index: nearest_value_index(&active, position, range),
            mode: CursorMode::Playback,
        };
        self.playing = end > start;
    }

    fn sample_range(
        &self,
        recording: &ActivityRecordingSnapshot,
    ) -> std::ops::RangeInclusive<usize> {
        self.selected_lap
            .and_then(|lap| lap_sample_range(recording, lap))
            .unwrap_or(0..=recording.samples.len().saturating_sub(1))
    }

    fn advance_playback(&mut self, ui: &Ui, recording: &ActivityRecordingSnapshot) {
        if !self.playing || recording.samples.len() < 2 {
            return;
        }
        let active = active_positions(recording);
        let range = self.sample_range(recording);
        let start = *active.get(*range.start()).unwrap_or(&0.0);
        let end = *active.get(*range.end()).unwrap_or(&start);
        let current_index = self
            .cursor
            .sample_index
            .filter(|index| range.contains(index))
            .unwrap_or(*range.start());
        let position = self
            .playback_position
            .unwrap_or_else(|| active.get(current_index).copied().unwrap_or(start));
        let delta_seconds = f64::from(ui.ctx().input(|input| input.stable_dt));
        let next = position + playback_delta(end - start, delta_seconds, self.playback_speed);
        if next >= end {
            self.playing = false;
            self.playback_position = Some(end);
            self.cursor = ActivityCursor {
                sample_index: Some(*range.end()),
                mode: CursorMode::Pinned,
            };
        } else {
            self.playback_position = Some(next);
            self.cursor = ActivityCursor {
                sample_index: nearest_value_index(&active, next, range),
                mode: CursorMode::Playback,
            };
            ui.ctx().request_repaint();
        }
    }

    fn sample_details(
        &self,
        ui: &mut Ui,
        intl: &Intl,
        recording: &ActivityRecordingSnapshot,
        sport: ActivitySport,
        units: UnitSystem,
    ) {
        let Some(sample) = self
            .cursor
            .sample_index
            .and_then(|index| recording.samples.get(index))
        else {
            return;
        };
        ui.add_space(8.0);
        ui.label(RichText::new(format_message!(intl, default_message: "At cursor")).strong());
        let start = recording
            .samples
            .first()
            .map_or(0, |sample| sample.timestamp.as_unix_milliseconds());
        let elapsed = (sample.timestamp.as_unix_milliseconds() - start).max(0) as f64 / 1_000.0;
        let mut metrics = vec![(
            format_message!(intl, default_message: "Time"),
            format_domain_tick(elapsed, Axis::Elapsed, units),
        )];
        if let Some(distance) = sample.distance {
            metrics.push((
                format_message!(intl, default_message: "Distance"),
                format_distance(distance.as_millimeters(), units),
            ));
        }
        for kind in ChartKind::ALL {
            if let Some(value) = kind.value(sample, sport, units) {
                metrics.push((
                    kind.label(intl, sport),
                    kind.format_value(value, sport, units),
                ));
            }
        }
        metric_grid(ui, &metrics);
    }

    fn laps(
        &mut self,
        ui: &mut Ui,
        intl: &Intl,
        recording: &ActivityRecordingSnapshot,
        units: UnitSystem,
    ) {
        if recording.laps.is_empty() {
            return;
        }
        ui.add_space(10.0);
        ui.label(RichText::new(format_message!(intl, default_message: "Laps")).strong());
        let active_lap = self
            .cursor
            .sample_index
            .and_then(|sample| lap_containing_sample(recording, sample));
        let mut hovered = None;
        ui.scope(|ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            lap_table_header(ui, intl);
            for (index, lap) in recording.laps.iter().enumerate() {
                let distance = lap.totals.distance().map_or_else(
                    || "—".to_owned(),
                    |distance| format_distance(distance.as_millimeters(), units),
                );
                let number = u64::try_from(index + 1).unwrap_or(u64::MAX);
                let label = format_message!(
                    intl,
                    default_message: "Lap {number}",
                    values: { number: number },
                );
                let duration = super::duration(lap.totals.timer().into_milliseconds());
                let selected = self.selected_lap == Some(index)
                    || (self.selected_lap.is_none() && active_lap == Some(index));
                let response = lap_row(ui, &label, &distance, &duration, selected);
                if response.hovered() {
                    hovered = Some(index);
                }
                if response.clicked() {
                    self.selected_lap = Some(index);
                    self.playing = false;
                    self.playback_position = None;
                    if let Some(range) = lap_sample_range(recording, index) {
                        self.cursor = ActivityCursor {
                            sample_index: Some(*range.start()),
                            mode: CursorMode::Pinned,
                        };
                    }
                    ui.ctx().request_repaint();
                }
            }
        });
        if self.hovered_lap != hovered {
            self.hovered_lap = hovered;
            ui.ctx().request_repaint();
        }
    }
}

fn floating_control<R>(
    ui: &mut Ui,
    bounds: egui::Rect,
    layout: Layout,
    add_contents: impl FnOnce(&mut Ui) -> R,
) -> egui::InnerResponse<R> {
    let palette = crate::theme::palette(ui);
    let shadow = if ui.visuals().dark_mode {
        egui::Color32::from_black_alpha(96)
    } else {
        egui::Color32::from_black_alpha(48)
    };
    let mut control_ui = ui.new_child(egui::UiBuilder::new().max_rect(bounds).layout(layout));
    egui::Frame::new()
        .fill(color32(palette.surfaces().layer(theme::Level::One)))
        .stroke(egui::Stroke::new(1.0, color32(palette.borders().subtle())))
        .corner_radius(egui::CornerRadius::same(8))
        .shadow(egui::Shadow {
            offset: [0, 2],
            blur: 8,
            spread: 0,
            color: shadow,
        })
        .inner_margin(2)
        .show(&mut control_ui, add_contents)
}

fn paint_horizontal_control_separator(ui: &Ui, upper: egui::Rect, gap: f32) {
    ui.painter().hline(
        upper.x_range().shrink(8.0),
        upper.bottom() + gap / 2.0,
        control_separator_stroke(ui),
    );
}

fn paint_vertical_control_separator(ui: &Ui, left: egui::Rect, gap: f32) {
    ui.painter().vline(
        left.right() + gap / 2.0,
        left.y_range().shrink(8.0),
        control_separator_stroke(ui),
    );
}

fn control_separator_stroke(ui: &Ui) -> egui::Stroke {
    egui::Stroke::new(
        1.0,
        color32(crate::theme::palette(ui).borders().subtle()).gamma_multiply(0.7),
    )
}

fn floating_icon_button(
    ui: &mut Ui,
    label: &str,
    icon: icons::Icon,
    selected: bool,
    enabled: bool,
) -> egui::Response {
    floating_button(ui, Some(icon), None, 32.0, selected, enabled).on_hover_text(label)
}

fn floating_text_button(
    ui: &mut Ui,
    label: &str,
    width: f32,
    selected: bool,
    enabled: bool,
) -> egui::Response {
    floating_button(ui, None, Some(label), width, selected, enabled)
}

fn floating_labeled_button(
    ui: &mut Ui,
    label: &str,
    icon: icons::Icon,
    width: f32,
    selected: bool,
    enabled: bool,
) -> egui::Response {
    floating_button(ui, Some(icon), Some(label), width, selected, enabled)
}

fn floating_button(
    ui: &mut Ui,
    icon: Option<icons::Icon>,
    text: Option<&str>,
    width: f32,
    selected: bool,
    enabled: bool,
) -> egui::Response {
    let palette = crate::theme::palette(ui);
    let surface = color32(palette.surfaces().layer(theme::Level::One));
    let foreground = color32(palette.content().icon_primary());
    let disabled = color32(palette.content().icon_disabled());
    let primary = palette.buttons().primary();
    let rest = if selected {
        map_control_visuals(
            color32(primary.rest().background()),
            color32(primary.rest().background()),
            color32(primary.rest().foreground()),
        )
    } else {
        map_control_visuals(surface, egui::Color32::TRANSPARENT, foreground)
    };
    let hovered = if selected {
        map_control_visuals(
            color32(primary.hover().background()),
            color32(primary.hover().background()),
            color32(primary.hover().foreground()),
        )
    } else {
        map_control_visuals(
            color32(palette.surfaces().layer_hover(theme::Level::One)),
            color32(palette.surfaces().background_hover()),
            foreground,
        )
    };
    let active = if selected {
        map_control_visuals(
            color32(primary.active().background()),
            color32(primary.active().background()),
            color32(primary.active().foreground()),
        )
    } else {
        let fill = color32(palette.surfaces().layer(theme::Level::Two));
        map_control_visuals(fill, fill, foreground)
    };
    let disabled = map_control_visuals(surface, egui::Color32::TRANSPARENT, disabled);

    ui.scope(|ui| {
        ui.spacing_mut().interact_size.y = 32.0;
        ui.spacing_mut().button_padding = egui::vec2(8.0, 0.0);
        let widgets = &mut ui.visuals_mut().widgets;
        widgets.inactive = rest;
        widgets.hovered = hovered;
        widgets.active = active;
        widgets.open = active;
        widgets.noninteractive = disabled;
        ui.visuals_mut().interact_cursor = Some(egui::CursorIcon::PointingHand);
        let image = icon.map(|icon| icon.mask().fit_to_exact_size(egui::Vec2::splat(15.0)));
        let text = text.map(|text| RichText::new(text).size(11.0).into());
        ui.add_enabled(
            enabled,
            egui::Button::opt_image_and_text(image, text)
                .image_tint_follows_text_color(true)
                .gap(5.0)
                .min_size(egui::vec2(width, 32.0))
                .corner_radius(egui::CornerRadius::same(5)),
        )
    })
    .inner
}

fn map_control_visuals(
    background: egui::Color32,
    weak_background: egui::Color32,
    foreground: egui::Color32,
) -> egui::style::WidgetVisuals {
    egui::style::WidgetVisuals {
        bg_fill: background,
        weak_bg_fill: weak_background,
        bg_stroke: egui::Stroke::NONE,
        corner_radius: egui::CornerRadius::same(5),
        fg_stroke: egui::Stroke::new(1.0, foreground),
        expansion: 0.0,
    }
}

fn lap_table_header(ui: &mut Ui, intl: &Intl) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 24.0), Sense::hover());
    let palette = crate::theme::palette(ui);
    ui.painter().rect_filled(
        rect,
        egui::CornerRadius::ZERO,
        palette.surfaces().background_hover().into_cint(),
    );
    let columns = LapColumns::new(rect);
    let color = color32(palette.content().text_secondary());
    let font = egui::TextStyle::Small.resolve(ui.style());
    paint_lap_columns(ui, rect, columns);
    ui.painter().text(
        egui::pos2(columns.left, rect.center().y),
        egui::Align2::LEFT_CENTER,
        "#",
        font.clone(),
        color,
    );
    ui.painter().text(
        egui::pos2(columns.distance_left, rect.center().y),
        egui::Align2::LEFT_CENTER,
        format_message!(intl, default_message: "Distance"),
        font.clone(),
        color,
    );
    ui.painter().text(
        egui::pos2(columns.right, rect.center().y),
        egui::Align2::RIGHT_CENTER,
        format_message!(intl, default_message: "Time"),
        font,
        color,
    );
}

fn lap_row(
    ui: &mut Ui,
    label: &str,
    distance: &str,
    duration: &str,
    selected: bool,
) -> egui::Response {
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 30.0), Sense::click());
    let response = response.on_hover_cursor(egui::CursorIcon::PointingHand);
    response.widget_info(|| {
        egui::WidgetInfo::labeled(
            egui::WidgetType::Button,
            ui.is_enabled(),
            format!("{label}, {distance}, {duration}"),
        )
    });
    let palette = crate::theme::palette(ui);
    if response.hovered() || selected {
        ui.painter().rect_filled(
            rect,
            CONTROL_RADIUS,
            palette
                .surfaces()
                .layer_hover(theme::Level::Two)
                .into_cint(),
        );
    }
    if selected {
        ui.painter().rect_filled(
            egui::Rect::from_min_size(rect.min, egui::vec2(3.0, rect.height())),
            egui::CornerRadius::ZERO,
            crate::theme::selection_accent(ui).into_cint(),
        );
    }
    let columns = LapColumns::new(rect);
    paint_lap_columns(ui, rect, columns);
    let secondary = color32(palette.content().text_secondary());
    let primary = color32(palette.content().text_primary());
    ui.painter().text(
        egui::pos2(columns.left, rect.center().y),
        egui::Align2::LEFT_CENTER,
        label,
        egui::TextStyle::Button.resolve(ui.style()),
        primary,
    );
    ui.painter().text(
        egui::pos2(columns.distance_left, rect.center().y),
        egui::Align2::LEFT_CENTER,
        distance,
        egui::TextStyle::Small.resolve(ui.style()),
        secondary,
    );
    ui.painter().text(
        egui::pos2(columns.right, rect.center().y),
        egui::Align2::RIGHT_CENTER,
        duration,
        egui::TextStyle::Small.resolve(ui.style()),
        secondary,
    );
    if response.has_focus() {
        ui.painter().rect_stroke(
            rect,
            CONTROL_RADIUS,
            egui::Stroke::new(2.0, palette.interaction().focus().into_cint()),
            egui::StrokeKind::Inside,
        );
    }
    response
}

#[derive(Clone, Copy)]
struct LapColumns {
    left: f32,
    distance_left: f32,
    time_left: f32,
    right: f32,
}

impl LapColumns {
    fn new(rect: egui::Rect) -> Self {
        let left = rect.left() + 8.0;
        let right = rect.right() - 8.0;
        Self {
            left,
            distance_left: left + 42.0,
            time_left: right - 58.0,
            right,
        }
    }
}

fn paint_lap_columns(ui: &Ui, rect: egui::Rect, columns: LapColumns) {
    let stroke = egui::Stroke::new(
        1.0,
        color32(crate::theme::palette(ui).borders().subtle()).gamma_multiply(0.72),
    );
    ui.painter().hline(rect.x_range(), rect.bottom(), stroke);
    ui.painter()
        .vline(columns.distance_left - 7.0, rect.y_range(), stroke);
    ui.painter()
        .vline(columns.time_left - 7.0, rect.y_range(), stroke);
}

fn summary(ui: &mut Ui, presentation: &Presentation) {
    let palette = crate::theme::palette(ui);
    ui.scope(|ui| {
        ui.spacing_mut().item_spacing.y = 1.0;
        ui.label(RichText::new(&presentation.title).size(16.0).strong());
        ui.label(
            RichText::new(&presentation.subtitle)
                .small()
                .color(color32(palette.content().text_secondary())),
        );
    });
    ui.add_space(6.0);
    let metrics = presentation
        .metric_props()
        .into_iter()
        .map(|MetricProps { label, value }| (label.to_owned(), value.to_owned()))
        .collect::<Vec<_>>();
    metric_grid(ui, &metrics);
}

fn metric_grid(ui: &mut Ui, metrics: &[(String, String)]) {
    const CELL_HEIGHT: f32 = 54.0;
    const CELL_GAP: f32 = 2.0;
    const CELL_PADDING: f32 = 10.0;

    let palette = crate::theme::palette(ui);
    let fill = color32(palette.surfaces().layer(theme::Level::Two));
    let primary = color32(palette.content().text_primary());
    let secondary = color32(palette.content().text_secondary());
    ui.scope(|ui| {
        ui.spacing_mut().item_spacing.y = CELL_GAP;
        for row in metrics.chunks(2) {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = CELL_GAP;
                let cell_width = ((ui.available_width() - CELL_GAP) / 2.0).max(1.0);
                for (label, value) in row {
                    let (rect, _) =
                        ui.allocate_exact_size(egui::vec2(cell_width, CELL_HEIGHT), Sense::hover());
                    let painter = ui.painter().with_clip_rect(rect);
                    painter.rect_filled(rect, CONTROL_RADIUS, fill);
                    painter.text(
                        rect.left_top() + egui::vec2(CELL_PADDING, 8.0),
                        egui::Align2::LEFT_TOP,
                        label,
                        egui::TextStyle::Small.resolve(ui.style()),
                        secondary,
                    );
                    painter.text(
                        rect.left_top() + egui::vec2(CELL_PADDING, 23.0),
                        egui::Align2::LEFT_TOP,
                        value,
                        egui::FontId::proportional(15.0),
                        primary,
                    );
                }
            });
        }
    });
}

fn viewer_inset() -> egui::Frame {
    egui::Frame::new().inner_margin(egui::Margin::symmetric(12, 0))
}

fn empty_panel(ui: &mut Ui, message: &str) {
    let (rect, _) = ui.allocate_exact_size(
        Vec2::new(ui.available_width(), ui.available_height().max(180.0)),
        Sense::hover(),
    );
    let palette = crate::theme::palette(ui);
    ui.painter().rect_filled(
        rect,
        PANEL_RADIUS,
        palette.surfaces().layer(theme::Level::One).into_cint(),
    );
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        message,
        egui::TextStyle::Body.resolve(ui.style()),
        color32(palette.content().text_secondary()),
    );
}

fn loading_panel(ui: &mut Ui, intl: &Intl) {
    empty_panel(
        ui,
        &format_message!(intl, default_message: "Loading activity…"),
    );
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
enum Axis {
    Distance,
    Elapsed,
}

#[derive(Clone)]
struct Domain {
    elapsed: Arc<[f64]>,
    distance: Arc<[f64]>,
    distance_available: bool,
}

impl Domain {
    fn new(recording: &ActivityRecordingSnapshot, units: UnitSystem) -> Self {
        let start = recording
            .samples
            .first()
            .map_or(0, |sample| sample.timestamp.as_unix_milliseconds());
        let elapsed = recording
            .samples
            .iter()
            .map(|sample| (sample.timestamp.as_unix_milliseconds() - start).max(0) as f64 / 1_000.0)
            .collect::<Vec<_>>()
            .into();
        let distance = distance_domain(&recording.samples, units);
        let distance_available = distance.is_some();
        Self {
            elapsed,
            distance: distance.unwrap_or_default().into(),
            distance_available,
        }
    }

    fn values(&self, axis: Axis) -> &[f64] {
        match axis {
            Axis::Distance if self.distance_available => &self.distance,
            Axis::Distance | Axis::Elapsed => &self.elapsed,
        }
    }

    fn bounds(
        &self,
        axis: Axis,
        range: std::ops::RangeInclusive<usize>,
    ) -> std::ops::RangeInclusive<f64> {
        let values = self.values(axis);
        let start = values.get(*range.start()).copied().unwrap_or(0.0);
        let mut end = values.get(*range.end()).copied().unwrap_or(start + 1.0);
        if end <= start {
            end = start + 1.0;
        }
        start..=end
    }
}

fn distance_domain(samples: &[ActivitySampleSnapshot], units: UnitSystem) -> Option<Vec<f64>> {
    let anchors = samples
        .iter()
        .enumerate()
        .filter_map(|(index, sample)| {
            sample
                .distance
                .map(|distance| (index, distance.as_millimeters()))
        })
        .collect::<Vec<_>>();
    if anchors.len() < 2
        || anchors.windows(2).any(|pair| pair[1].1 < pair[0].1)
        || anchors.last()?.1 == anchors.first()?.1
    {
        return None;
    }
    let mut values = vec![anchors[0].1 as f64; samples.len()];
    for pair in anchors.windows(2) {
        let (start_index, start_value) = pair[0];
        let (end_index, end_value) = pair[1];
        let count = (end_index - start_index) as f64;
        for (offset, value) in values[start_index..=end_index].iter_mut().enumerate() {
            let fraction = if count > 0.0 {
                offset as f64 / count
            } else {
                0.0
            };
            *value = ((end_value - start_value) as f64).mul_add(fraction, start_value as f64);
        }
    }
    let (last_index, last_value) = *anchors.last()?;
    values[last_index..].fill(last_value as f64);
    for value in &mut values {
        *value = display_distance(*value, units);
    }
    Some(values)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PlaybackSpeed {
    Half,
    Normal,
    Double,
}

impl PlaybackSpeed {
    const fn factor(self) -> f64 {
        match self {
            Self::Half => 0.5,
            Self::Normal => 1.0,
            Self::Double => 2.0,
        }
    }

    const fn label(self) -> &'static str {
        match self {
            Self::Half => "0.5×",
            Self::Normal => "1×",
            Self::Double => "2×",
        }
    }

    const fn next(self) -> Self {
        match self {
            Self::Half => Self::Normal,
            Self::Normal => Self::Double,
            Self::Double => Self::Half,
        }
    }
}

fn playback_delta(active_span: f64, frame_seconds: f64, speed: PlaybackSpeed) -> f64 {
    active_span / PLAYBACK_SECONDS * frame_seconds * speed.factor()
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
enum ChartKind {
    Elevation,
    PaceSpeed,
    HeartRate,
    Cadence,
    Power,
    Temperature,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct ChartCacheKey {
    axis: Axis,
    kind: ChartKind,
    sport: ActivitySport,
    units: UnitSystem,
    max_points: usize,
}

impl ChartKind {
    const ALL: [Self; 6] = [
        Self::Elevation,
        Self::PaceSpeed,
        Self::HeartRate,
        Self::Cadence,
        Self::Power,
        Self::Temperature,
    ];

    fn label(self, intl: &Intl, sport: ActivitySport) -> String {
        match self {
            Self::Elevation => format_message!(intl, default_message: "Elevation"),
            Self::PaceSpeed if sport == ActivitySport::Running => {
                format_message!(intl, default_message: "Pace")
            }
            Self::PaceSpeed => format_message!(intl, default_message: "Speed"),
            Self::HeartRate => format_message!(intl, default_message: "Heart rate"),
            Self::Cadence => format_message!(intl, default_message: "Cadence"),
            Self::Power => format_message!(intl, default_message: "Power"),
            Self::Temperature => format_message!(intl, default_message: "Temperature"),
        }
    }

    fn has_values(self, recording: &ActivityRecordingSnapshot, sport: ActivitySport) -> bool {
        recording
            .samples
            .iter()
            .any(|sample| self.value(sample, sport, UnitSystem::Metric).is_some())
    }

    fn color(self, ui: &Ui) -> egui::Color32 {
        let palette = crate::theme::palette(ui);
        color32(match self {
            Self::Elevation => palette.support().success(),
            Self::PaceSpeed => crate::theme::selection_accent(ui),
            Self::HeartRate => palette.support().error(),
            Self::Cadence => palette.interaction().link_visited(),
            Self::Power => palette.support().warning(),
            Self::Temperature => palette.content().icon_secondary(),
        })
    }

    fn fill_baseline(self, stats: ChartStats, sport: ActivitySport) -> f64 {
        let span = (stats.maximum - stats.minimum).max(1.0);
        if self == Self::PaceSpeed && sport == ActivitySport::Running {
            stats.maximum + span * 0.12
        } else if stats.minimum >= 0.0 && stats.minimum <= span * 1.5 {
            0.0
        } else {
            stats.minimum - span * 0.12
        }
    }

    fn value(
        self,
        sample: &ActivitySampleSnapshot,
        sport: ActivitySport,
        units: UnitSystem,
    ) -> Option<f64> {
        match self {
            Self::Elevation => sample.elevation_meters.map(|elevation| match units {
                UnitSystem::Metric => elevation,
                UnitSystem::Imperial => elevation * 3.280_839_895,
            }),
            Self::PaceSpeed if sport == ActivitySport::Running => {
                let speed = f64::from(sample.speed?.as_millimeters_per_second()) / 1_000.0;
                (speed > 0.0).then(|| match units {
                    UnitSystem::Metric => 1_000.0 / speed / 60.0,
                    UnitSystem::Imperial => 1_609.344 / speed / 60.0,
                })
            }
            Self::PaceSpeed => {
                let speed = f64::from(sample.speed?.as_millimeters_per_second()) / 1_000.0;
                Some(match units {
                    UnitSystem::Metric => speed * 3.6,
                    UnitSystem::Imperial => speed * 2.236_936_292,
                })
            }
            Self::HeartRate => Some(f64::from(sample.heart_rate?.as_beats_per_minute())),
            Self::Cadence => Some(sample.cadence?.as_revolutions_per_minute()),
            Self::Power => Some(f64::from(sample.power?.as_watts())),
            Self::Temperature => {
                let celsius = f64::from(sample.temperature_millicelsius?) / 1_000.0;
                Some(match units {
                    UnitSystem::Metric => celsius,
                    UnitSystem::Imperial => celsius.mul_add(9.0 / 5.0, 32.0),
                })
            }
        }
    }

    fn format_value(self, value: f64, sport: ActivitySport, units: UnitSystem) -> String {
        match self {
            Self::Elevation => format!("{value:.0} {}", elevation_unit(units)),
            Self::PaceSpeed if sport == ActivitySport::Running => {
                format!("{} /{}", format_pace(value), distance_unit(units))
            }
            Self::PaceSpeed => format!(
                "{value:.1} {}",
                match units {
                    UnitSystem::Metric => "km/h",
                    UnitSystem::Imperial => "mph",
                }
            ),
            Self::HeartRate => format!("{value:.0} bpm"),
            Self::Cadence => format!("{value:.0} rpm"),
            Self::Power => format!("{value:.0} W"),
            Self::Temperature => format!(
                "{value:.1} °{}",
                match units {
                    UnitSystem::Metric => "C",
                    UnitSystem::Imperial => "F",
                }
            ),
        }
    }
}

#[derive(Clone, Copy)]
struct ChartStats {
    minimum: f64,
    average: f64,
    maximum: f64,
}

fn chart_stats(
    recording: &ActivityRecordingSnapshot,
    range: std::ops::RangeInclusive<usize>,
    kind: ChartKind,
    sport: ActivitySport,
    units: UnitSystem,
) -> Option<ChartStats> {
    let mut values = recording.samples.get(range)?.iter().filter_map(|sample| {
        kind.value(sample, sport, units)
            .filter(|value| value.is_finite())
    });
    let first = values.next()?;
    let mut minimum = first;
    let mut maximum = first;
    let mut total = first;
    let mut count = 1_u64;
    for value in values {
        minimum = minimum.min(value);
        maximum = maximum.max(value);
        total += value;
        count += 1;
    }
    Some(ChartStats {
        minimum,
        average: total / count as f64,
        maximum,
    })
}

fn chart_summary(
    ui: &mut Ui,
    intl: &Intl,
    kind: ChartKind,
    stats: ChartStats,
    sport: ActivitySport,
    units: UnitSystem,
) {
    let labels = [
        format_message!(intl, default_message: "Minimum"),
        format_message!(intl, default_message: "Average"),
        format_message!(intl, default_message: "Maximum"),
    ];
    let values = [
        kind.format_value(stats.minimum, sport, units),
        kind.format_value(stats.average, sport, units),
        kind.format_value(stats.maximum, sport, units),
    ];
    let secondary = color32(crate::theme::palette(ui).content().text_secondary());
    ui.columns(3, |columns| {
        for ((column, label), value) in columns.iter_mut().zip(labels).zip(values) {
            column.spacing_mut().item_spacing.y = 2.0;
            column.label(RichText::new(label).small().color(secondary));
            column.label(RichText::new(value).size(15.0).strong());
        }
    });
}

fn format_pace(minutes: f64) -> String {
    let total_seconds = (minutes.max(0.0) * 60.0).round();
    let minutes = (total_seconds / 60.0).floor();
    let seconds = total_seconds % 60.0;
    format!("{minutes:.0}:{seconds:02.0}")
}

fn chart_lines(
    recording: &ActivityRecordingSnapshot,
    x_values: &[f64],
    kind: ChartKind,
    sport: ActivitySport,
    units: UnitSystem,
    max_points: usize,
) -> Vec<Vec<PlotPoint>> {
    let mut lines = Vec::new();
    let mut current = Vec::new();
    for (index, sample) in recording.samples.iter().enumerate() {
        if let (Some(x), Some(value)) = (x_values.get(index), kind.value(sample, sport, units)) {
            current.push(PlotPoint::new(*x, value));
        } else if !current.is_empty() {
            lines.push(downsample(&current, max_points));
            current.clear();
        }
    }
    if !current.is_empty() {
        lines.push(downsample(&current, max_points));
    }
    lines
}

fn downsample(points: &[PlotPoint], max_points: usize) -> Vec<PlotPoint> {
    if points.len() <= max_points.max(4) {
        return points.to_vec();
    }
    let bucket_count = (max_points / 4).max(1);
    let bucket_size = points.len().div_ceil(bucket_count);
    let mut result = Vec::with_capacity(bucket_count * 4);
    for bucket in points.chunks(bucket_size) {
        let mut indexes = [0, 0, 0, bucket.len() - 1];
        for (index, point) in bucket.iter().enumerate().skip(1) {
            if point.y < bucket[indexes[1]].y {
                indexes[1] = index;
            }
            if point.y > bucket[indexes[2]].y {
                indexes[2] = index;
            }
        }
        indexes.sort_unstable();
        let mut previous = None;
        for index in indexes {
            if previous != Some(index) {
                result.push(bucket[index]);
                previous = Some(index);
            }
        }
    }
    result
}

fn nearest_index(
    values: &[f64],
    needle: f64,
    range: std::ops::RangeInclusive<usize>,
) -> Option<usize> {
    let start = *range.start();
    let end = (*range.end()).min(values.len().checked_sub(1)?);
    if start > end {
        return None;
    }
    let slice = &values[start..=end];
    let insertion = slice.partition_point(|value| *value < needle);
    match insertion {
        0 => Some(start),
        value if value >= slice.len() => Some(end),
        value => {
            let before = value - 1;
            let selected = if (slice[value] - needle).abs() < (slice[before] - needle).abs() {
                value
            } else {
                before
            };
            Some(start + selected)
        }
    }
}

fn nearest_value_index(
    values: &[f64],
    needle: f64,
    range: std::ops::RangeInclusive<usize>,
) -> Option<usize> {
    nearest_index(values, needle, range)
}

fn lap_sample_range(
    recording: &ActivityRecordingSnapshot,
    lap: usize,
) -> Option<std::ops::RangeInclusive<usize>> {
    let lap = recording.laps.get(lap)?;
    let start = lap.time.start().as_unix_milliseconds();
    let end = lap.time.end().as_unix_milliseconds();
    let first = recording
        .samples
        .partition_point(|sample| sample.timestamp.as_unix_milliseconds() < start);
    if recording
        .samples
        .get(first)
        .is_none_or(|sample| sample.timestamp.as_unix_milliseconds() > end)
    {
        return None;
    }
    let after = recording
        .samples
        .partition_point(|sample| sample.timestamp.as_unix_milliseconds() <= end);
    let last = after.checked_sub(1)?;
    Some(first..=last)
}

fn lap_x_bounds(
    recording: &ActivityRecordingSnapshot,
    x_values: &[f64],
    lap: usize,
) -> Option<(f64, f64)> {
    let range = lap_sample_range(recording, lap)?;
    Some((*x_values.get(*range.start())?, *x_values.get(*range.end())?))
}

fn active_positions(recording: &ActivityRecordingSnapshot) -> Vec<f64> {
    let Some(first) = recording.samples.first() else {
        return Vec::new();
    };
    let mut events = recording.timer_events.iter().peekable();
    let mut state = ActivityTimerStateSnapshot::Running;
    let mut previous = first.timestamp.as_unix_milliseconds();
    while let Some(event) =
        events.next_if(|event| event.timestamp.as_unix_milliseconds() <= previous)
    {
        state = event.state;
    }
    let mut active = 0_i64;
    let mut positions = Vec::with_capacity(recording.samples.len());
    for sample in &recording.samples {
        let timestamp = sample.timestamp.as_unix_milliseconds().max(previous);
        while let Some(event) =
            events.next_if(|event| event.timestamp.as_unix_milliseconds() <= timestamp)
        {
            let transition = event.timestamp.as_unix_milliseconds().max(previous);
            if state == ActivityTimerStateSnapshot::Running {
                active += transition - previous;
            }
            previous = transition;
            state = event.state;
        }
        if state == ActivityTimerStateSnapshot::Running {
            active += timestamp - previous;
        }
        previous = timestamp;
        positions.push(active.max(0) as f64);
    }
    positions
}

fn display_distance(millimeters: f64, units: UnitSystem) -> f64 {
    match units {
        UnitSystem::Metric => millimeters / 1_000_000.0,
        UnitSystem::Imperial => millimeters / 1_609_344.0,
    }
}

fn format_distance(millimeters: u64, units: UnitSystem) -> String {
    let distance = display_distance(millimeters as f64, units);
    format!("{distance:.2} {}", distance_unit(units))
}

fn format_domain_tick(value: f64, axis: Axis, units: UnitSystem) -> String {
    match axis {
        Axis::Distance => format!("{value:.1} {}", distance_unit(units)),
        Axis::Elapsed => {
            let elapsed = value.max(0.0).round();
            let hours = (elapsed / 3_600.0).floor();
            let minutes = ((elapsed % 3_600.0) / 60.0).floor();
            let seconds = elapsed % 60.0;
            if hours == 0.0 {
                format!("{minutes:02}:{seconds:02}")
            } else {
                format!("{hours:02}:{minutes:02}:{seconds:02}")
            }
        }
    }
}

fn lap_containing_sample(
    recording: &ActivityRecordingSnapshot,
    sample_index: usize,
) -> Option<usize> {
    let timestamp = recording.samples.get(sample_index)?.timestamp;
    recording
        .laps
        .iter()
        .position(|lap| lap.time.start() <= timestamp && timestamp <= lap.time.end())
}

const fn distance_unit(units: UnitSystem) -> &'static str {
    match units {
        UnitSystem::Metric => "km",
        UnitSystem::Imperial => "mi",
    }
}

const fn elevation_unit(units: UnitSystem) -> &'static str {
    match units {
        UnitSystem::Metric => "m",
        UnitSystem::Imperial => "ft",
    }
}

fn selected_route_coordinate(
    recording: &ActivityRecordingSnapshot,
    cursor: ActivityCursor,
    playback_position: Option<f64>,
    range: std::ops::RangeInclusive<usize>,
) -> Option<(f64, f64)> {
    let selected = cursor.sample_index?;
    let selected_coordinate = recording.samples.get(selected)?.coordinate?;
    if cursor.mode != CursorMode::Playback {
        return Some((
            selected_coordinate.longitude().as_degrees(),
            selected_coordinate.latitude().as_degrees(),
        ));
    }
    let position = playback_position?;
    let active = active_positions(recording);
    let start = *range.start();
    let end = (*range.end()).min(active.len().checked_sub(1)?);
    let values = active.get(start..=end)?;
    let after = values.partition_point(|value| *value <= position);
    if after == 0 || after >= values.len() {
        return Some((
            selected_coordinate.longitude().as_degrees(),
            selected_coordinate.latitude().as_degrees(),
        ));
    }
    let before_index = start + after - 1;
    let after_index = start + after;
    let before = recording.samples.get(before_index)?.coordinate?;
    let after = recording.samples.get(after_index)?.coordinate?;
    let before_position = active[before_index];
    let after_position = active[after_index];
    if after_position <= before_position {
        return Some((
            selected_coordinate.longitude().as_degrees(),
            selected_coordinate.latitude().as_degrees(),
        ));
    }
    let fraction =
        ((position - before_position) / (after_position - before_position)).clamp(0.0, 1.0);
    let before_longitude = before.longitude().as_degrees();
    let longitude_delta =
        (after.longitude().as_degrees() - before_longitude + 540.0).rem_euclid(360.0) - 180.0;
    let longitude =
        (longitude_delta.mul_add(fraction, before_longitude) + 180.0).rem_euclid(360.0) - 180.0;
    let latitude = (after.latitude().as_degrees() - before.latitude().as_degrees())
        .mul_add(fraction, before.latitude().as_degrees());
    Some((longitude, latitude))
}

#[cfg(test)]
mod tests {
    use egui_plot::PlotPoint;
    use garmin_model::{
        activity::{
            ActivityDuration, ActivityMetrics, ActivitySport, ActivityTotals, Distance, TimeRange,
        },
        identity::UnitSystem,
        route::{Coordinate, Latitude, Longitude},
        value::Timestamp,
    };
    use garmin_service_api::{
        ActivityLapSnapshot, ActivityRecordingSnapshot, ActivitySampleSnapshot,
        ActivityTimerEventSnapshot, ActivityTimerStateSnapshot,
    };

    use super::{
        ActivityCursor, Axis, ChartKind, CursorMode, Viewer, active_positions, chart_lines,
        distance_domain, downsample, format_pace, lap_sample_range, nearest_index, playback_delta,
        selected_route_coordinate,
    };

    fn timestamp(milliseconds: i64) -> Timestamp {
        Timestamp::from_unix_milliseconds(milliseconds).unwrap()
    }

    fn sample(
        milliseconds: i64,
        distance_millimeters: Option<u64>,
        elevation_meters: Option<f64>,
        coordinate: Option<(f64, f64)>,
    ) -> ActivitySampleSnapshot {
        ActivitySampleSnapshot {
            timestamp: timestamp(milliseconds),
            coordinate: coordinate.map(|(latitude, longitude)| {
                Coordinate::from_parts(
                    Latitude::from_degrees(latitude).unwrap(),
                    Longitude::from_degrees(longitude).unwrap(),
                )
            }),
            elevation_meters,
            distance: distance_millimeters.map(Distance::from_millimeters),
            speed: None,
            heart_rate: None,
            cadence: None,
            power: None,
            temperature_millicelsius: None,
        }
    }

    fn recording(samples: Vec<ActivitySampleSnapshot>) -> ActivityRecordingSnapshot {
        ActivityRecordingSnapshot {
            laps: Vec::new(),
            samples,
            timer_events: Vec::new(),
        }
    }

    #[test]
    fn nearest_plot_position_reports_original_sample_index() {
        let values = [0.0, 1.0, 2.5, 9.0];
        assert_eq!(nearest_index(&values, 2.1, 0..=3), Some(2));
        assert_eq!(nearest_index(&values, 8.8, 1..=3), Some(3));
    }

    #[test]
    fn downsampling_keeps_bucket_extrema_and_endpoints() {
        let points = (0..100)
            .map(|index| PlotPoint::new(f64::from(index), f64::from(index % 7)))
            .collect::<Vec<_>>();
        let sampled = downsample(&points, 20);
        assert_eq!(sampled.first(), points.first());
        assert_eq!(sampled.last(), points.last());
        assert!(sampled.len() <= 20);
    }

    #[test]
    fn distance_domain_interpolates_gaps_and_rejects_regressions() {
        let samples = [
            sample(0, Some(0), None, None),
            sample(1_000, None, None, None),
            sample(2_000, Some(2_000_000), None, None),
        ];
        assert_eq!(
            distance_domain(&samples, UnitSystem::Metric),
            Some(vec![0.0, 1.0, 2.0])
        );

        let regressing = [
            sample(0, Some(2_000), None, None),
            sample(1_000, Some(1_000), None, None),
        ];
        assert_eq!(distance_domain(&regressing, UnitSystem::Metric), None);
    }

    #[test]
    fn chart_domains_and_measurements_respect_profile_units() {
        let mut first = sample(0, Some(0), Some(10.0), None);
        first.speed = Some(garmin_model::activity::Speed::from_millimeters_per_second(
            1_000,
        ));
        first.temperature_millicelsius = Some(0);
        let second = sample(1_000, Some(1_609_344), Some(20.0), None);
        let samples = [first, second];

        assert_eq!(
            distance_domain(&samples, UnitSystem::Imperial),
            Some(vec![0.0, 1.0])
        );
        assert!(
            (ChartKind::Elevation
                .value(&samples[0], ActivitySport::Cycling, UnitSystem::Imperial)
                .unwrap()
                - 32.808_398_95)
                .abs()
                < 1.0e-9
        );
        assert!(
            (ChartKind::PaceSpeed
                .value(&samples[0], ActivitySport::Cycling, UnitSystem::Imperial)
                .unwrap()
                - 2.236_936_292)
                .abs()
                < 1.0e-9
        );
        assert_eq!(
            ChartKind::Temperature.value(&samples[0], ActivitySport::Cycling, UnitSystem::Imperial),
            Some(32.0)
        );
    }

    #[test]
    fn chart_series_preserve_measurement_gaps() {
        let recording = recording(vec![
            sample(0, None, Some(10.0), None),
            sample(1_000, None, None, None),
            sample(2_000, None, Some(12.0), None),
        ]);
        let lines = chart_lines(
            &recording,
            &[0.0, 1.0, 2.0],
            ChartKind::Elevation,
            ActivitySport::Running,
            UnitSystem::Metric,
            100,
        );

        assert_eq!(
            lines,
            vec![
                vec![PlotPoint::new(0.0, 10.0)],
                vec![PlotPoint::new(2.0, 12.0)]
            ]
        );
    }

    #[test]
    fn active_timeline_omits_stopped_timer_intervals() {
        let mut recording = recording(vec![
            sample(0, None, None, None),
            sample(10_000, None, None, None),
            sample(20_000, None, None, None),
            sample(30_000, None, None, None),
        ]);
        recording.timer_events = vec![
            ActivityTimerEventSnapshot {
                timestamp: timestamp(10_000),
                state: ActivityTimerStateSnapshot::Stopped,
            },
            ActivityTimerEventSnapshot {
                timestamp: timestamp(20_000),
                state: ActivityTimerStateSnapshot::Running,
            },
        ];

        assert_eq!(
            active_positions(&recording),
            vec![0.0, 10_000.0, 10_000.0, 20_000.0]
        );
    }

    #[test]
    fn playback_speed_scales_the_thirty_second_timeline() {
        assert!((playback_delta(90.0, 1.0, super::PlaybackSpeed::Half) - 1.5).abs() < f64::EPSILON);
        assert!(
            (playback_delta(90.0, 1.0, super::PlaybackSpeed::Normal) - 3.0).abs() < f64::EPSILON
        );
        assert!(
            (playback_delta(90.0, 1.0, super::PlaybackSpeed::Double) - 6.0).abs() < f64::EPSILON
        );
        assert_eq!(
            super::PlaybackSpeed::Half.next(),
            super::PlaybackSpeed::Normal
        );
        assert_eq!(
            super::PlaybackSpeed::Normal.next(),
            super::PlaybackSpeed::Double
        );
        assert_eq!(
            super::PlaybackSpeed::Double.next(),
            super::PlaybackSpeed::Half
        );
    }

    #[test]
    fn pace_format_carries_rounded_seconds_into_minutes() {
        assert_eq!(format_pace(4.999), "5:00");
    }

    #[test]
    fn lap_ranges_include_their_boundary_samples() {
        let mut recording = recording(vec![
            sample(0, None, None, None),
            sample(10_000, None, None, None),
            sample(20_000, None, None, None),
            sample(30_000, None, None, None),
        ]);
        let time = TimeRange::from_parts(timestamp(10_000), timestamp(20_000)).unwrap();
        let totals = ActivityTotals::from_parts(
            ActivityDuration::from_milliseconds(10_000),
            ActivityDuration::from_milliseconds(10_000),
            None,
            None,
            None,
            None,
        )
        .unwrap();
        recording.laps.push(ActivityLapSnapshot {
            time,
            totals,
            metrics: ActivityMetrics::default(),
        });
        recording.laps.push(ActivityLapSnapshot {
            time: TimeRange::from_parts(timestamp(40_000), timestamp(50_000)).unwrap(),
            totals,
            metrics: ActivityMetrics::default(),
        });

        assert_eq!(lap_sample_range(&recording, 0), Some(1..=2));
        assert_eq!(lap_sample_range(&recording, 1), None);
    }

    #[test]
    fn playback_marker_interpolates_across_the_dateline() {
        let recording = recording(vec![
            sample(0, None, None, Some((60.0, 179.0))),
            sample(10_000, None, None, Some((62.0, -179.0))),
        ]);
        let coordinate = selected_route_coordinate(
            &recording,
            ActivityCursor {
                sample_index: Some(0),
                mode: CursorMode::Playback,
            },
            Some(5_000.0),
            0..=1,
        )
        .unwrap();

        assert!((coordinate.0.abs() - 180.0).abs() < 1.0e-9);
        assert!((coordinate.1 - 61.0).abs() < 1.0e-9);
    }

    #[test]
    fn shared_cursor_reports_hover_pin_and_host_updates() {
        let mut viewer = Viewer::default();
        assert!(viewer.apply_pointer(Some(3), None));
        assert_eq!(
            viewer.cursor(),
            ActivityCursor {
                sample_index: Some(3),
                mode: CursorMode::Hover,
            }
        );

        assert!(viewer.apply_pointer(Some(4), Some(4)));
        assert_eq!(viewer.cursor().mode, CursorMode::Pinned);
        viewer.set_cursor(ActivityCursor {
            sample_index: Some(8),
            mode: CursorMode::Playback,
        });
        assert_eq!(viewer.cursor().sample_index, Some(8));
        assert!(viewer.playing);

        viewer.playback_position = Some(12.0);
        viewer.set_selected_lap(Some(2));
        assert_eq!(viewer.selected_lap(), Some(2));
        assert!(!viewer.playing);
        assert_eq!(viewer.playback_position, None);
    }

    #[test]
    fn activity_changes_reset_all_linked_interaction_state() {
        let mut viewer = Viewer::default();
        viewer.sync_recording("first");
        viewer.cursor = ActivityCursor {
            sample_index: Some(8),
            mode: CursorMode::Pinned,
        };
        viewer.axis = Axis::Elapsed;
        viewer.selected_lap = Some(2);
        viewer.hovered_lap = Some(1);
        viewer.playing = true;
        viewer.playback_position = Some(42.0);

        viewer.sync_recording("second");

        assert_eq!(viewer.cursor, ActivityCursor::default());
        assert_eq!(viewer.axis, Axis::Distance);
        assert_eq!(viewer.selected_lap, None);
        assert_eq!(viewer.hovered_lap, None);
        assert!(!viewer.playing);
        assert_eq!(viewer.playback_position, None);
    }
}
