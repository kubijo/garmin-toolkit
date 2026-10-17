use super::*;

type TestResult = Result<(), Box<dyn std::error::Error>>;
const GPX: &[u8] = include_bytes!("../../../tests/fixtures/candidates.gpx");

#[test]
fn protocol_rejects_wrong_input_version_trailing_bytes_and_invalid_geometry() -> TestResult {
    let expected = ArtifactDigest::from_bytes(GPX);
    let original = encode(GPX)?;
    assert!(decode(&original, ArtifactDigest::from_bytes(b"another input")).is_err());
    let mut trailing = original.clone();
    trailing.push(0);
    assert!(decode(&trailing, expected).is_err());
    for change in 0..5 {
        let mut reply: Reply = postcard::from_bytes(&original)?;
        let Reply::Parsed {
            protocol,
            adapter_version,
            candidates,
            ..
        } = &mut reply
        else {
            return Err("fixture was rejected".into());
        };
        match change {
            0 => *protocol += 1,
            1 => *adapter_version = "999.0.0".to_owned(),
            2 => candidates[0].points.clear(),
            3 => candidates.push(candidates[0].clone()),
            4 => candidates[0].points[0].1 = Some(f64::NAN),
            _ => unreachable!(),
        }
        assert!(matches!(
            decode(&postcard::to_stdvec(&reply)?, expected),
            Err(Error::Protocol)
        ));
    }
    Ok(())
}
