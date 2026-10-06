//! Private, versioned IPC; validated domain types never come directly from raw tuples.

use std::collections::HashSet;

use garmin_model::{
    artifact::ArtifactDigest,
    route::{Coordinate, Elevation, RoutePoint, RouteShape},
    value::ComponentVersion,
};

use super::{Error, MAX_CANDIDATES, MAX_NAME_BYTES};
use crate::{Candidate, CandidateSource, Document, RejectedCandidate};

const VERSION: u32 = 1;

#[cfg(test)]
mod tests;

#[garmin_macros::portable]
enum Reply {
    Parsed {
        protocol: u32,
        adapter_version: String,
        digest: [u8; 32],
        candidates: Vec<WireCandidate>,
        rejected: Vec<WireRejected>,
    },
    Rejected(String),
}

#[garmin_macros::portable]
struct WireCandidate {
    source: CandidateSource,
    name: Option<String>,
    geometry: bool,
    points: Vec<(Coordinate, Option<f64>)>,
}

#[garmin_macros::portable]
struct WireRejected {
    source: CandidateSource,
    reason: garmin_model::route::Error,
}

pub(super) fn encode(bytes: &[u8]) -> Result<Vec<u8>, postcard::Error> {
    let reply = match crate::parse(bytes) {
        Ok(document) => encode_document(bytes, document),
        Err(error) => Reply::Rejected(error.to_string().chars().take(MAX_NAME_BYTES).collect()),
    };
    postcard::to_stdvec(&reply)
}

fn encode_document(bytes: &[u8], document: Document) -> Reply {
    if document.candidates.len() + document.rejected.len() > MAX_CANDIDATES {
        return Reply::Rejected("too many GPX candidates".to_owned());
    }
    if document.candidates.iter().any(|candidate| {
        candidate
            .suggested_name
            .as_ref()
            .is_some_and(|name| name.as_str().len() > MAX_NAME_BYTES)
    }) {
        return Reply::Rejected("GPX candidate name exceeds its size limit".to_owned());
    }
    Reply::Parsed {
        protocol: VERSION,
        adapter_version: crate::ADAPTER_VERSION.to_owned(),
        digest: *ArtifactDigest::from_bytes(bytes).as_blake3().as_bytes(),
        candidates: document
            .candidates
            .into_iter()
            .map(|candidate| WireCandidate {
                source: candidate.source,
                name: candidate
                    .suggested_name
                    .map(garmin_model::route::RouteName::into_string),
                geometry: candidate.shape.is_geometry(),
                points: candidate
                    .shape
                    .points()
                    .iter()
                    .map(|point| {
                        (
                            point.coordinate(),
                            point.elevation().map(Elevation::into_meters),
                        )
                    })
                    .collect(),
            })
            .collect(),
        rejected: document
            .rejected
            .into_iter()
            .map(|candidate| WireRejected {
                source: candidate.source,
                reason: candidate.reason,
            })
            .collect(),
    }
}

pub(super) fn decode(
    bytes: &[u8],
    expected: ArtifactDigest,
) -> Result<(Document, ComponentVersion), Error> {
    let (reply, trailing): (Reply, _) =
        postcard::take_from_bytes(bytes).map_err(|_| Error::Protocol)?;
    if !trailing.is_empty() {
        return Err(Error::Protocol);
    }
    let Reply::Parsed {
        protocol,
        adapter_version,
        digest,
        candidates,
        rejected,
    } = reply
    else {
        let Reply::Rejected(reason) = reply else {
            return Err(Error::Protocol);
        };
        if reason.len() > MAX_NAME_BYTES * 4 {
            return Err(Error::Protocol);
        }
        return Err(Error::Rejected(reason));
    };
    if protocol != VERSION
        || adapter_version != crate::ADAPTER_VERSION
        || &digest != expected.as_blake3().as_bytes()
        || candidates.len() + rejected.len() > MAX_CANDIDATES
    {
        return Err(Error::Protocol);
    }
    let count = candidates
        .iter()
        .try_fold(0_usize, |count, candidate| {
            count.checked_add(candidate.points.len())
        })
        .ok_or(Error::Protocol)?;
    if count > crate::MAX_POINTS {
        return Err(Error::Protocol);
    }
    let mut sources = HashSet::new();
    for source in candidates
        .iter()
        .map(|candidate| candidate.source)
        .chain(rejected.iter().map(|candidate| candidate.source))
    {
        if !sources.insert(source) {
            return Err(Error::Protocol);
        }
    }
    let candidates = candidates
        .into_iter()
        .map(decode_candidate)
        .collect::<Result<Vec<_>, _>>()?;
    let rejected = rejected
        .into_iter()
        .map(|candidate| RejectedCandidate {
            source: candidate.source,
            reason: candidate.reason,
        })
        .collect();
    let parser = ComponentVersion::from_parts(
        crate::ADAPTER_NAME,
        adapter_version.parse().map_err(|_| Error::Protocol)?,
    )
    .map_err(|_| Error::Protocol)?;
    Ok((
        Document {
            candidates,
            rejected,
        },
        parser,
    ))
}

fn decode_candidate(candidate: WireCandidate) -> Result<Candidate, Error> {
    if candidate
        .name
        .as_ref()
        .is_some_and(|name| name.len() > MAX_NAME_BYTES)
    {
        return Err(Error::Protocol);
    }
    let suggested_name = candidate
        .name
        .map(|name| name.parse())
        .transpose()
        .map_err(|_| Error::Protocol)?;
    let points = candidate
        .points
        .into_iter()
        .map(|(coordinate, elevation)| {
            Ok(RoutePoint::from_parts(
                coordinate,
                elevation
                    .map(Elevation::from_meters)
                    .transpose()
                    .map_err(|_| Error::Protocol)?,
            ))
        })
        .collect::<Result<Vec<_>, Error>>()?;
    let shape = if candidate.geometry {
        RouteShape::from_geometry(points)
    } else {
        RouteShape::from_control_points(points)
    }
    .map_err(|_| Error::Protocol)?;
    if candidate.geometry != matches!(candidate.source, CandidateSource::TrackSegment { .. }) {
        return Err(Error::Protocol);
    }
    Ok(Candidate {
        source: candidate.source,
        suggested_name,
        shape,
    })
}
