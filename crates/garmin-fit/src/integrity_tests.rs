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
fn developer_metadata_does_not_override_native_activity_values() -> Result<(), Box<dyn Error>> {
    use crate::fixture::developer;
    use crate::fixture::developer::Metadata;

    for header in [Header::Legacy, Header::Checksummed] {
        let control = normalize_activities(&developer::activity(header, Metadata::Plain))?;
        for metadata in Metadata::ALL {
            let bytes = developer::activity(header, metadata);
            let sequences = super::decode_raw(&bytes)?;
            assert_eq!(sequences.len(), 1);
            let record = sequences[0].messages.last().ok_or("missing record")?;
            assert_eq!(record.num, rustyfit::profile::typedef::MesgNum::RECORD);
            if matches!(metadata, Metadata::Described | Metadata::MissingDeveloperId) {
                let [field] = record.developer_fields.as_slice() else {
                    return Err("expected one decoded developer field".into());
                };
                assert_eq!(field.num, 5);
                assert_eq!(field.developer_data_index, 0);
                assert!(matches!(
                    &field.value,
                    rustyfit::proto::Value::Uint32(9_999_999)
                ));
            } else {
                assert_eq!(record.developer_fields.len(), 0);
            }
            assert_eq!(
                normalize_activities(&bytes)?,
                control,
                "{header:?}: {metadata:?}"
            );
        }
    }
    Ok(())
}

#[test]
fn cycling_workout_has_a_serial_bound_name() -> Result<(), Box<dyn Error>> {
    use rustyfit::profile::mesgdef;

    for header in [Header::Legacy, Header::Checksummed] {
        let (name, bytes) = crate::fixture::workout::cycling(header, 0x1234_5678);
        assert_eq!(name, "TKW-12345678");
        let sequences = super::decode_raw(&bytes)?;
        assert_eq!(sequences.len(), 1);
        let messages = &sequences[0].messages;
        assert_eq!(messages.len(), 4);
        assert_eq!(
            mesgdef::FileId::from(&messages[0]).serial_number,
            0x1234_5678
        );
        let workout = mesgdef::Workout::from(&messages[1]);
        assert_eq!(workout.sport.0, 2);
        assert_eq!(workout.wkt_name, name);
        assert_eq!(workout.num_valid_steps, 2);
    }
    Ok(())
}

#[test]
fn workout_decodes_but_is_not_an_activity() -> Result<(), Box<dyn Error>> {
    use rustyfit::profile::{mesgdef, typedef};

    for header in [Header::Legacy, Header::Checksummed] {
        let bytes = crate::fixture::workout::bytes(header);
        let sequences = super::decode_raw(&bytes)?;
        assert_eq!(sequences.len(), 1);
        let messages = &sequences[0].messages;
        assert_eq!(messages.len(), 4);
        assert_eq!(mesgdef::FileId::from(&messages[0]).r#type.0, 5);
        assert_eq!(messages[1].num, typedef::MesgNum::WORKOUT);
        let workout = mesgdef::Workout::from(&messages[1]);
        assert_eq!(workout.sport.0, 1);
        assert_eq!(workout.num_valid_steps, 2);
        assert_eq!(workout.wkt_name, "Test");
        for (message, (index, duration, intensity)) in
            messages[2..].iter().zip([(0, 60_000, 0), (1, 30_000, 1)])
        {
            assert_eq!(message.num, typedef::MesgNum::WORKOUT_STEP);
            let step = mesgdef::WorkoutStep::from(message);
            assert_eq!(step.message_index.0, index);
            assert_eq!(step.duration_type.0, 0);
            assert_eq!(step.duration_value, duration);
            assert_eq!(step.target_type.0, 2);
            assert_eq!(step.target_value, 0);
            assert_eq!(step.intensity.0, intensity);
        }
        assert!(
            matches!(normalize_activities(&bytes), Err(ActivityError::UnsupportedFileType(kind)) if kind.as_u8() == 5)
        );
    }
    Ok(())
}

#[test]
fn health_messages_decode_but_are_rejected_as_non_activity_files() -> Result<(), Box<dyn Error>> {
    use rustyfit::profile::{mesgdef, typedef};

    use crate::fixture::health::Health;

    for header in [Header::Legacy, Header::Checksummed] {
        for health in Health::ALL {
            let bytes = health.bytes(header);
            let sequences = super::decode_raw(&bytes)?;
            let [fit] = sequences.as_slice() else {
                return Err("expected one health sequence".into());
            };
            assert_eq!(
                mesgdef::FileId::from(&fit.messages[0]).r#type.0,
                health.file_type()
            );
            match health {
                Health::Wellness => {
                    assert_eq!(fit.messages.len(), 2);
                    assert_eq!(fit.messages[1].num, typedef::MesgNum::MONITORING);
                    let sample = mesgdef::Monitoring::from(&fit.messages[1]);
                    assert_eq!(sample.timestamp.0, integrity::START);
                    assert_eq!(sample.heart_rate, 62);
                }
                Health::Sleep => {
                    assert_eq!(fit.messages.len(), 3);
                    for (message, (offset, level)) in
                        fit.messages[1..].iter().zip([(0, 2), (60, 3)])
                    {
                        assert_eq!(message.num, typedef::MesgNum::SLEEP_LEVEL);
                        let sample = mesgdef::SleepLevel::from(message);
                        assert_eq!(sample.timestamp.0, integrity::START + offset);
                        assert_eq!(sample.sleep_level.0, level);
                    }
                }
                Health::Hrv => {
                    assert_eq!(fit.messages.len(), 2);
                    assert_eq!(fit.messages[1].num, typedef::MesgNum::HRV);
                    assert_eq!(
                        mesgdef::Hrv::from(&fit.messages[1]).time,
                        [800, 810, u16::MAX]
                    );
                }
            }
            assert!(matches!(
                normalize_activities(&bytes),
                Err(ActivityError::UnsupportedFileType(kind)) if kind.as_u8() == health.file_type()
            ));
        }
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
