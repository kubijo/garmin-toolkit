use std::error::Error;

use crate::{
    ActivityError, DecodeError, fixture::integrity, fixture::integrity::Header,
    normalize_activities,
};

#[test]
fn independent_recordings_have_the_declared_activity() -> Result<(), Box<dyn Error>> {
    for header in [Header::Legacy, Header::Checksummed] {
        let import = normalize_activities(&integrity::activity(header))?;
        let activity = import.activities().next().ok_or("missing activity")?;
        assert_eq!(import.activities().count(), 1);
        assert_eq!(activity.summary().sport(), crate::ActivitySport::Running);
        assert_eq!(
            activity.summary().time().start().as_unix_seconds(),
            i64::from(integrity::START) + 631_065_600
        );
        assert_eq!(
            activity.summary().time().end().as_unix_seconds(),
            i64::from(integrity::START) + 631_065_630
        );
        assert_eq!(
            activity
                .summary()
                .totals()
                .distance()
                .ok_or("missing distance")?
                .as_millimeters(),
            500_000
        );
        assert_eq!(activity.laps().len(), 1);
        assert_eq!(activity.track().len(), 1);
    }
    Ok(())
}

#[test]
fn multisession_is_decodable_but_not_normalized() -> Result<(), Box<dyn Error>> {
    for header in [Header::Legacy, Header::Checksummed] {
        let bytes = integrity::multisession(header);
        let sequences = super::decode_raw(&bytes)?;
        assert_eq!(sequences.len(), 1);
        let sessions = sequences[0]
            .messages
            .iter()
            .filter(|message| message.num == rustyfit::profile::typedef::MesgNum::SESSION)
            .map(rustyfit::profile::mesgdef::Session::from)
            .collect::<Vec<_>>();
        assert_eq!(sessions.len(), 2);
        for (session, start) in sessions
            .iter()
            .zip([integrity::START, integrity::START + 30])
        {
            assert_eq!(session.start_time.0, start);
            assert_eq!(session.timestamp.0, start + 30);
            assert_eq!(session.total_distance, 50_000);
        }
        assert!(matches!(
            normalize_activities(&bytes),
            Err(ActivityError::CountMismatch {
                name: "session count",
                expected: 1,
                found: 2,
            })
        ));
    }
    Ok(())
}

#[test]
fn valid_zero_header_crc_is_a_preserved_parser_limitation() {
    assert!(matches!(
        normalize_activities(&integrity::activity(Header::ZeroCrc)),
        Err(ActivityError::Decode(DecodeError::Rejected(_)))
    ));
}

#[test]
fn rejects_all_independent_integrity_failures() {
    let mut accepted = Vec::new();
    for header in Header::ALL {
        for (name, bytes) in integrity::corruptions(header) {
            if normalize_activities(&bytes).is_ok() {
                accepted.push(format!("{header:?}: {name}"));
            }
        }
    }
    assert!(accepted.is_empty(), "accepted corrupt FIT: {accepted:?}");
}

#[test]
fn validates_every_sequence_in_a_chain() -> Result<(), Box<dyn Error>> {
    let first = integrity::activity(Header::Legacy);
    let second = integrity::activity(Header::Checksummed);
    let mut valid = first.clone();
    valid.extend(&second);
    assert_eq!(normalize_activities(&valid)?.activities().count(), 2);
    // A cut exactly between complete sequences is a valid shorter artifact.
    assert_eq!(normalize_activities(&first)?.activities().count(), 1);
    for (name, tail) in integrity::corruptions(Header::Checksummed) {
        if tail.is_empty() {
            continue;
        }
        let mut bytes = first.clone();
        bytes.extend(tail);
        assert!(
            normalize_activities(&bytes).is_err(),
            "accepted corrupt chained tail: {name}"
        );
    }
    Ok(())
}
