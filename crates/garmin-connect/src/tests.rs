use super::*;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

async fn server(responses: Vec<(u16, Vec<u8>)>) -> (Client, tokio::task::JoinHandle<Vec<String>>) {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("loopback listener");
    let origin = format!(
        "http://{}",
        listener.local_addr().expect("listener address")
    );
    let peer = tokio::spawn(async move {
        let mut requests = Vec::new();
        for (status, body) in responses {
            let (mut socket, _) = listener.accept().await.expect("request");
            let mut request: Vec<u8> = Vec::new();
            loop {
                let mut bytes = [0; 4096];
                let count = socket.read(&mut bytes).await.expect("request headers");
                assert!(count > 0);
                request.extend_from_slice(&bytes[..count]);
                if request.windows(4).any(|part| part == b"\r\n\r\n") {
                    break;
                }
            }
            requests.push(String::from_utf8(request).expect("ASCII headers"));
            let length = body.len();
            let redirect = if status == 302 {
                "Location: http://127.0.0.1:9/unexpected-redirect\n"
            } else {
                ""
            };
            let headers = indoc::formatdoc! {"
                HTTP/1.1 {status} Test
                Content-Type: application/json
                Content-Length: {length}
                {redirect}Connection: close

                "}
            .replace('\n', "\r\n");
            socket
                .write_all(headers.as_bytes())
                .await
                .expect("response headers");
            let _ = socket.write_all(&body).await;
        }
        requests
    });
    (Client { origin }, peer)
}

fn key() -> PersonalApiKey {
    PersonalApiKey::from_string("private-test-key".into()).expect("test key")
}

#[tokio::test]
async fn account_identity_is_exact_or_rejected_never_truncated() {
    for length in [256, 257] {
        let identity = "a".repeat(length);
        let (client, peer) = server(vec![
            (200, b"test.access.token".to_vec()),
            (
                200,
                serde_json::to_vec(&serde_json::json!({"displayName": identity})).unwrap(),
            ),
        ])
        .await;
        let result = client.check(&key()).await;
        if length == 256 {
            assert_eq!(
                result.unwrap().display_name.as_deref(),
                Some(identity.as_str())
            );
        } else {
            assert!(matches!(result, Err(Error::InvalidProfile)));
        }
        peer.await.unwrap();
    }
}

#[tokio::test]
async fn download_requires_an_account_identifier_before_requesting_file_bytes() {
    let (client, peer) = server(vec![
        (200, b"test.access.token".to_vec()),
        (200, br#"{"preferredLocale":"en"}"#.to_vec()),
    ])
    .await;
    assert!(matches!(
        client.download(&key(), 42).await,
        Err(Error::MissingAccountIdentity)
    ));
    assert_eq!(peer.await.unwrap().len(), 2);
}

#[tokio::test]
async fn exchanges_key_then_reads_typed_facts_with_bearer_authorization() {
    let (client, peer) = server(vec![
        (200, b" test.access.token \n".to_vec()),
        (200, br#"{"displayName":"Rider","preferredLocale":"cs","measurementSystem":"metric","timeZone":"Europe/Prague","futureField":true}"#.to_vec()),
    ]).await;
    let facts = client.check(&key()).await.expect("verified profile");
    assert_eq!(facts.display_name.as_deref(), Some("Rider"));
    assert_eq!(facts.locale.as_deref(), Some("cs"));
    assert_eq!(facts.measurement_system.as_deref(), Some("metric"));
    assert_eq!(facts.time_zone.as_deref(), Some("Europe/Prague"));
    let requests = peer.await.expect("server");
    assert!(requests[0].starts_with(&format!("POST {TOKEN_PATH} ")));
    assert!(
        requests[0]
            .to_lowercase()
            .contains("authorization: apikey private-test-key")
    );
    assert!(requests[1].starts_with("GET /userprofile-service/userprofile/settings "));
    assert!(
        requests[1]
            .to_lowercase()
            .contains("authorization: bearer test.access.token")
    );
    assert!(!requests[1].contains("private-test-key"));
}

#[tokio::test]
async fn rejected_keys_redirects_and_rate_limits_stop_before_profile_request_and_redact_errors() {
    for status in [302, 401, 403, 429, 500] {
        let (client, peer) = server(vec![(
            status,
            b"private-test-key test.access.token".to_vec(),
        )])
        .await;
        let error = client.check(&key()).await.expect_err("refused key");
        assert!(matches!(error, Error::Rejected { status: actual, .. } if actual == status));
        let diagnostic = format!("{error:?} {error}");
        assert!(!diagnostic.contains("private-test-key"));
        assert!(!diagnostic.contains("test.access.token"));
        assert_eq!(peer.await.expect("server").len(), 1);
    }
}

#[tokio::test]
async fn malformed_tokens_and_oversized_token_responses_are_rejected() {
    for token in [
        Vec::new(),
        b"not a token".to_vec(),
        vec![b'x'; limit_bytes(TOKEN_LIMIT).unwrap() + 1],
    ] {
        let (client, peer) = server(vec![(200, token)]).await;
        assert!(matches!(
            client.check(&key()).await,
            Err(Error::InvalidToken | Error::TooLarge)
        ));
        peer.await.expect("server");
    }
}

#[tokio::test]
async fn profile_failures_and_empty_or_malformed_facts_do_not_count_as_verification() {
    for (status, profile) in [
        (401, b"denied".as_slice()),
        (429, b"limited".as_slice()),
        (200, b"not-json".as_slice()),
        (200, br#"{"displayName":42}"#.as_slice()),
        (200, b"{}".as_slice()),
    ] {
        let (client, peer) = server(vec![
            (200, b"test.access.token".to_vec()),
            (status, profile.to_vec()),
        ])
        .await;
        assert!(client.check(&key()).await.is_err());
        peer.await.expect("server");
    }
}

#[tokio::test]
async fn oversized_profile_is_rejected_without_retaining_its_body() {
    let (client, peer) = server(vec![
        (200, b"test.access.token".to_vec()),
        (200, vec![b'x'; limit_bytes(PROFILE_LIMIT).unwrap() + 1]),
    ])
    .await;
    assert!(matches!(client.check(&key()).await, Err(Error::TooLarge)));
    peer.await.expect("server");
}

#[tokio::test]
async fn activity_pages_use_bearer_auth_and_explicit_pagination() {
    let (client, peer) = server(vec![
        (200, b"test.access.token".to_vec()),
        (200, br#"{"displayName":"account"}"#.to_vec()),
        (200, br#"[{"activityId":42,"activityName":"Road ride","activityType":{"typeKey":"road_biking"},"startTimeGMT":"2026-11-10 10:00:00","distance":13230.0,"duration":3600.0}]"#.to_vec()),
    ]).await;
    let (_, summaries) = client.activities(&key(), 20).await.unwrap();
    let activities = summaries
        .into_iter()
        .map(|summary| summary.activity)
        .collect::<Vec<_>>();
    assert_eq!(activities[0].id, 42);
    assert_eq!(
        activities[0].sport,
        Some(garmin_model::activity::ActivitySport::Cycling)
    );
    assert_eq!(activities[0].distance_metres, Some(13230.0));
    let requests = peer.await.unwrap();
    assert!(requests[2].starts_with("GET /activitylist-service/activities/search/activities?"));
    assert!(requests[2].contains("start=20"));
    assert!(requests[2].contains("limit=20"));
    assert!(requests[2].contains("excludeChildren=false"));
    assert!(!requests[2].contains("not_imported"));
    assert!(!requests[2].contains("private-test-key"));
    assert!(
        requests[2]
            .to_lowercase()
            .contains("authorization: bearer test.access.token")
    );
}

#[tokio::test]
async fn account_session_reuses_authentication_across_pages_and_original_downloads() {
    let first = serde_json::to_vec(
        &(1..=20)
            .map(|id| serde_json::json!({"activityId": id}))
            .collect::<Vec<_>>(),
    )
    .unwrap();
    let (client, peer) = server(vec![
        (200, b"test.access.token".to_vec()),
        (200, br#"{"displayName":"account"}"#.to_vec()),
        (200, first),
        (200, br#"[{"activityId":21,"activityName":"Coastal ride","description":"Recovery by the sea","startTimeLocal":"2026-11-11 00:30:00","startTimeGMT":"2026-11-10 22:30:00","activityType":{"typeKey":"road_cycling"}}]"#.to_vec()),
        (200, b"original-archive".to_vec()),
    ]).await;
    let session = client.session(&key()).await.unwrap();
    assert_eq!(session.activities(0).await.unwrap().len(), 20);
    let second = session.activities(20).await.unwrap();
    assert_eq!(
        second[0].description.as_deref(),
        Some("Recovery by the sea")
    );
    assert_eq!(
        second[0].started_local.as_deref(),
        Some("2026-11-11 00:30:00")
    );
    assert_eq!(second[0].activity_type.as_deref(), Some("road_cycling"));
    assert_eq!(
        session.download(21).await.unwrap().archive,
        b"original-archive"
    );
    let requests = peer.await.unwrap();
    assert_eq!(requests.len(), 5);
    assert!(requests[3].contains("start=20"));
    for request in &requests[2..] {
        assert!(
            request
                .to_lowercase()
                .contains("authorization: bearer test.access.token")
        );
    }
}

#[tokio::test]
async fn activity_download_preserves_archive_bytes_and_account_identity() {
    let original = b"PK\x03\x04\x00\x80\xff";
    let (client, peer) = server(vec![
        (200, b"test.access.token".to_vec()),
        (200, br#"{"displayName":"account"}"#.to_vec()),
        (200, original.to_vec()),
    ])
    .await;
    let result = client.download(&key(), 42).await.unwrap();
    assert_eq!(result.archive, original);
    assert_eq!(result.account.display_name.as_deref(), Some("account"));
    assert!(peer.await.unwrap()[2].starts_with("GET /download-service/files/activity/42 "));
}

#[tokio::test]
async fn malformed_or_duplicate_activity_identifiers_are_rejected() {
    for body in [
        b"[{}]".as_slice(),
        br#"[{"activityId":0}]"#,
        br#"[{"activityId":42},{"activityId":42}]"#,
        br#"{"unexpected":"shape"}"#,
    ] {
        let (client, peer) = server(vec![
            (200, b"test.access.token".to_vec()),
            (200, br#"{"displayName":"account"}"#.to_vec()),
            (200, body.to_vec()),
        ])
        .await;
        assert!(client.activities(&key(), 0).await.is_err());
        peer.await.unwrap();
    }
}

#[tokio::test]
async fn preview_reads_name_and_notes_with_the_same_bearer_session() {
    let (client, peer) = server(vec![
        (200, b"test.access.token".to_vec()),
        (200, br#"{"displayName":"account"}"#.to_vec()),
        (200, br#"{"activityId":42,"activityName":"Morning ride","description":"Windy, felt good","summaryDTO":{"directWorkoutFeel":75,"directWorkoutRpe":40}}"#.to_vec()),
        (200, b"original archive".to_vec()),
    ]).await;
    let (download, details) = client.preview(&key(), 42).await.unwrap();
    assert_eq!(details.name.as_deref(), Some("Morning ride"));
    assert_eq!(details.description.as_deref(), Some("Windy, felt good"));
    assert_eq!(download.archive, b"original archive");
    let requests = peer.await.unwrap();
    assert_eq!(requests.len(), 4);
    assert!(requests[2].starts_with("GET /activity-service/activity/42 "));
    assert!(requests[3].starts_with("GET /download-service/files/activity/42 "));
}

#[tokio::test]
async fn preview_rejects_mismatched_metadata_before_downloading() {
    let (client, peer) = server(vec![
        (200, b"test.access.token".to_vec()),
        (200, br#"{"displayName":"account"}"#.to_vec()),
        (
            200,
            br#"{"activityId":43,"activityName":"Different activity"}"#.to_vec(),
        ),
    ])
    .await;
    assert!(matches!(
        client.preview(&key(), 42).await,
        Err(Error::InvalidActivities)
    ));
    assert_eq!(peer.await.unwrap().len(), 3);
}
