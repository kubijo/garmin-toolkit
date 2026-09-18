//! Route worker protocol, CPU segmentation, and GPU publication.

use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
};

use super::{CpuRoute, GpuRoute, RouteSegment, RouteSource, UploadContext};
use crate::activity::map::mercator_y;

#[derive(Clone, Copy)]
pub(super) struct RouteSample {
    pub(super) coordinate: Option<[f64; 2]>,
    pub(super) speed: Option<f32>,
}

pub(super) struct RouteTask {
    pub(super) key: String,
    pub(super) samples: Vec<RouteSample>,
    pub(super) sample_offset: usize,
    pub(super) context: egui::Context,
}

pub(super) struct RouteResult {
    pub(super) key: String,
    pub(super) outcome: RouteOutcome,
}

pub(super) enum RouteOutcome {
    Ready(Option<RouteSource>),
    Failed,
}

pub(super) struct PreparedCpuRoute {
    pub(super) origin: [f64; 2],
    pub(super) route: Arc<CpuRoute>,
}

/// Browser-worker route operation with an opaque request/completion protocol.
pub struct BrowserRouteTask {
    pub(super) task: RouteTask,
    pub(super) results: Arc<Mutex<VecDeque<RouteResult>>>,
    pub(super) upload: Arc<UploadContext>,
}

impl BrowserRouteTask {
    /// Serialize this operation for the dedicated browser worker.
    ///
    /// # Errors
    ///
    /// Returns an error when the bounded worker request cannot be serialized.
    pub fn request(&self) -> Result<Vec<u8>, String> {
        let request = RouteRequestWire {
            version: ROUTE_PROTOCOL_VERSION,
            sample_offset: self.task.sample_offset,
            samples: self
                .task
                .samples
                .iter()
                .map(|sample| BrowserRouteSample {
                    coordinate: sample.coordinate,
                    speed: sample.speed,
                })
                .collect(),
        };
        postcard::to_stdvec(&request).map_err(|error| error.to_string())
    }

    pub fn complete(self, result: Result<Vec<u8>, String>) {
        let completed = result.and_then(|bytes| decode_browser_route(&bytes));
        let outcome = match completed {
            Ok(Some(prepared)) => RouteOutcome::Ready(Some(RouteSource {
                resource: Arc::new(GpuRoute::new(&self.upload, prepared.origin, prepared.route)),
            })),
            Ok(None) => RouteOutcome::Ready(None),
            Err(reason) => {
                tracing::warn!(%reason, "browser map route preparation failed");
                RouteOutcome::Failed
            }
        };
        self.results
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push_back(RouteResult {
                key: self.task.key,
                outcome,
            });
        self.task.context.request_repaint();
    }
}

pub(super) const ROUTE_PROTOCOL_VERSION: u8 = 2;

#[derive(serde::Deserialize, serde::Serialize)]
pub(super) struct RouteRequestWire {
    pub(super) version: u8,
    pub(super) sample_offset: usize,
    pub(super) samples: Vec<BrowserRouteSample>,
}

#[derive(serde::Deserialize, serde::Serialize)]
pub(super) struct BrowserRouteSample {
    pub(super) coordinate: Option<[f64; 2]>,
    pub(super) speed: Option<f32>,
}

#[derive(serde::Deserialize, serde::Serialize)]
pub(super) struct RouteResultWire {
    pub(super) version: u8,
    pub(super) origin: Option<[f64; 2]>,
    pub(super) segments: Vec<BrowserRouteSegment>,
}

#[derive(serde::Deserialize, serde::Serialize)]
pub(super) struct BrowserRouteSegment {
    pub(super) start: [f32; 2],
    pub(super) end: [f32; 2],
    pub(super) speed: [f32; 2],
    pub(super) sample_indices: [f32; 2],
    pub(super) start_join: [f32; 2],
    pub(super) end_join: [f32; 2],
    pub(super) caps: [f32; 2],
}

/// Project and segment an activity route inside the dedicated browser worker.
///
/// # Errors
///
/// Returns an error when the request is malformed or its output cannot be serialized.
pub fn prepare_route_for_browser_worker(bytes: &[u8]) -> Result<Vec<u8>, String> {
    encode_browser_route(bytes)
}

pub(super) fn encode_browser_route(bytes: &[u8]) -> Result<Vec<u8>, String> {
    let request: RouteRequestWire =
        postcard::from_bytes(bytes).map_err(|error| error.to_string())?;
    if request.version != ROUTE_PROTOCOL_VERSION {
        return Err(format!(
            "unsupported map route protocol version {}",
            request.version
        ));
    }
    let samples = request
        .samples
        .into_iter()
        .map(|sample| RouteSample {
            coordinate: sample.coordinate,
            speed: sample.speed,
        })
        .collect::<Vec<_>>();
    let (origin, segments) = match build_cpu_route(&samples, request.sample_offset) {
        Some(prepared) => (
            Some(prepared.origin),
            prepared
                .route
                .segments
                .iter()
                .map(|segment| BrowserRouteSegment {
                    start: segment.start,
                    end: segment.end,
                    speed: segment.speed,
                    sample_indices: segment.sample_indices,
                    start_join: segment.start_join,
                    end_join: segment.end_join,
                    caps: segment.caps,
                })
                .collect(),
        ),
        None => (None, Vec::new()),
    };
    postcard::to_stdvec(&RouteResultWire {
        version: ROUTE_PROTOCOL_VERSION,
        origin,
        segments,
    })
    .map_err(|error| error.to_string())
}

pub(super) fn decode_browser_route(bytes: &[u8]) -> Result<Option<PreparedCpuRoute>, String> {
    if bytes.len() > crate::activity::map_runtime::BrowserWorkerTaskKind::Route.result_byte_limit()
    {
        return Err("prepared map route exceeded the 32 MiB browser limit".to_owned());
    }
    let result: RouteResultWire = postcard::from_bytes(bytes).map_err(|error| error.to_string())?;
    if result.version != ROUTE_PROTOCOL_VERSION {
        return Err(format!(
            "unsupported map route protocol version {}",
            result.version
        ));
    }
    let Some(origin) = result.origin else {
        return if result.segments.is_empty() {
            Ok(None)
        } else {
            Err("prepared map route had segments without an origin".to_owned())
        };
    };
    if !origin.into_iter().all(f64::is_finite) || result.segments.is_empty() {
        return Err("prepared map route had invalid geometry".to_owned());
    }
    let segments = result
        .segments
        .into_iter()
        .map(|segment| RouteSegment {
            start: segment.start,
            end: segment.end,
            speed: segment.speed,
            sample_indices: segment.sample_indices,
            start_join: segment.start_join,
            end_join: segment.end_join,
            caps: segment.caps,
        })
        .collect::<Vec<_>>();
    if segments.iter().any(|segment| {
        segment
            .start
            .into_iter()
            .chain(segment.end)
            .chain(segment.speed)
            .chain(segment.sample_indices)
            .chain(segment.start_join)
            .chain(segment.end_join)
            .chain(segment.caps)
            .any(|value| !value.is_finite())
    }) {
        return Err("prepared map route contained non-finite values".to_owned());
    }
    Ok(Some(PreparedCpuRoute {
        origin,
        route: Arc::new(CpuRoute { segments }),
    }))
}

#[cfg(not(target_arch = "wasm32"))]
pub(super) fn build_route(
    context: &UploadContext,
    samples: &[RouteSample],
    sample_offset: usize,
) -> Option<RouteSource> {
    let prepared = build_cpu_route(samples, sample_offset)?;
    let resource = Arc::new(GpuRoute::new(context, prepared.origin, prepared.route));
    Some(RouteSource { resource })
}

#[expect(
    clippy::cast_possible_truncation,
    reason = "route coordinates are stored relative to a nearby f64 origin before GPU conversion"
)]
pub(super) fn build_cpu_route(
    samples: &[RouteSample],
    sample_offset: usize,
) -> Option<PreparedCpuRoute> {
    let mut points = Vec::with_capacity(samples.len());
    let mut previous_longitude: Option<f64> = None;
    let mut unwrapped_longitude = 0.0_f64;
    for sample in samples {
        let Some([longitude, latitude]) = sample.coordinate else {
            points.push(None);
            continue;
        };
        unwrapped_longitude = previous_longitude.map_or(longitude, |previous| {
            unwrapped_longitude + (longitude - previous + 540.0).rem_euclid(360.0) - 180.0
        });
        previous_longitude = Some(longitude);
        points.push(Some((
            [unwrapped_longitude / 360.0 + 0.5, mercator_y(latitude)],
            sample.speed.unwrap_or(-1.0),
        )));
    }
    let origin = points.iter().flatten().next()?.0;
    let segments = points
        .windows(2)
        .enumerate()
        .filter_map(|(offset, pair)| match (pair[0], pair[1]) {
            (Some((start, start_speed)), Some((end, end_speed))) => {
                let previous = offset
                    .checked_sub(1)
                    .and_then(|previous| points[previous].map(|point| point.0));
                let next = points
                    .get(offset + 2)
                    .and_then(|point| point.map(|point| point.0));
                let normal = segment_normal(start, end);
                Some(RouteSegment {
                    start: [(start[0] - origin[0]) as f32, (start[1] - origin[1]) as f32],
                    end: [(end[0] - origin[0]) as f32, (end[1] - origin[1]) as f32],
                    speed: [start_speed, end_speed],
                    sample_indices: [
                        (sample_offset + offset) as f32,
                        (sample_offset + offset + 1) as f32,
                    ],
                    start_join: previous
                        .map_or(normal, |previous| join_offset(previous, start, end)),
                    end_join: next.map_or(normal, |next| join_offset(start, end, next)),
                    caps: [
                        f32::from(u8::from(previous.is_none())),
                        f32::from(u8::from(next.is_none())),
                    ],
                })
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    (!segments.is_empty()).then(|| PreparedCpuRoute {
        origin,
        route: Arc::new(CpuRoute { segments }),
    })
}

fn segment_normal(start: [f64; 2], end: [f64; 2]) -> [f32; 2] {
    let direction = [end[0] - start[0], end[1] - start[1]];
    let length = direction[0].hypot(direction[1]);
    if length <= f64::EPSILON {
        return [0.0; 2];
    }
    [
        gpu_coordinate(-direction[1] / length),
        gpu_coordinate(direction[0] / length),
    ]
}

fn join_offset(previous: [f64; 2], point: [f64; 2], next: [f64; 2]) -> [f32; 2] {
    const MITER_LIMIT: f64 = 2.0;

    let incoming = segment_normal(previous, point).map(f64::from);
    let outgoing = segment_normal(point, next).map(f64::from);
    let sum = [incoming[0] + outgoing[0], incoming[1] + outgoing[1]];
    let sum_length = sum[0].hypot(sum[1]);
    if sum_length <= f64::EPSILON {
        return segment_normal(point, next);
    }
    let miter = [sum[0] / sum_length, sum[1] / sum_length];
    let denominator = (miter[0] * outgoing[0] + miter[1] * outgoing[1]).abs();
    let scale = (1.0 / denominator.max(f64::EPSILON)).min(MITER_LIMIT);
    [
        gpu_coordinate(miter[0] * scale),
        gpu_coordinate(miter[1] * scale),
    ]
}

#[expect(
    clippy::cast_possible_truncation,
    reason = "normalized route geometry is deliberately stored as f32 GPU data"
)]
fn gpu_coordinate(value: f64) -> f32 {
    value as f32
}
