use garmin_connect_codegen::generated::{ApiOpError, HttpClient, HttpError};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

async fn server(
    status: u16,
    content_type: &str,
    body: &[u8],
) -> (HttpClient, tokio::task::JoinHandle<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let headers = indoc::formatdoc! {"
        HTTP/1.1 {status} Test
        Content-Type: {content_type}
        Content-Length: {}
        Connection: close

        ", body.len()
    }
    .replace('\n', "\r\n");
    let reply = [headers.as_bytes(), body].concat();
    let peer = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = Vec::new();
        loop {
            let mut bytes = [0; 4096];
            let count = socket.read(&mut bytes).await.unwrap();
            assert!(count > 0);
            request.extend(&bytes[..count]);
            if request.windows(4).any(|part| part == b"\r\n\r\n") {
                break;
            }
        }
        socket.write_all(&reply).await.unwrap();
        String::from_utf8(request).unwrap()
    });
    let mut token = reqwest::header::HeaderValue::from_static("Bearer compatibility-only");
    token.set_sensitive(true);
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert(reqwest::header::AUTHORIZATION, token);
    let http = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .default_headers(headers)
        .timeout(std::time::Duration::from_secs(3))
        .build()
        .unwrap();
    (HttpClient::with_client(http).with_base_url(url), peer)
}

#[tokio::test]
async fn listing_preserves_zero_null_unknown_fields_and_encodes_pagination() {
    let (client, peer) = server(
        200,
        "application/json",
        br#"[{"activityId":1,"distance":0,"duration":null,"futureField":true}]"#,
    )
    .await;
    let page = client
        .get_activitylist_service_activities_search_activities(
            Some(2),
            Some(1),
            None,
            None::<&str>,
            None,
            None::<&str>,
            None::<&str>,
            None::<&str>,
            None::<&str>,
            None::<&str>,
            None,
            None,
            None,
            None,
            None,
            None,
            None::<&str>,
            None::<&str>,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            Some(vec!["indoor_cycling".into(), "virtual_ride".into()]),
            None,
        )
        .await
        .unwrap();
    assert_eq!(page[0].activity_id, Some(1));
    assert_eq!(page[0].distance, Some(0.0));
    assert_eq!(page[0].duration, None);
    let request = peer.await.unwrap();
    for query in [
        "start=2",
        "limit=1",
        "excludedActivitySubType=indoor_cycling",
        "excludedActivitySubType=virtual_ride",
    ] {
        assert!(request.contains(query), "{request}");
    }
    assert!(
        request
            .to_lowercase()
            .contains("authorization: bearer compatibility-only")
    );
    assert!(
        !request
            .lines()
            .next()
            .unwrap()
            .contains("compatibility-only")
    );
}

#[tokio::test]
async fn summary_deserializes_and_uses_activity_id_route() {
    let (client, peer) = server(
        200,
        "application/json",
        br#"{"activityId":42,"activityName":"Road"}"#,
    )
    .await;
    let value = client
        .get_activity_service_activity_activity_id(42)
        .await
        .unwrap();
    assert_eq!(value.activity_id, Some(42));
    assert!(
        peer.await
            .unwrap()
            .starts_with("GET /activity-service/activity/42 ")
    );
}

#[tokio::test]
async fn malformed_json_and_wrong_field_types_are_errors() {
    for body in [
        b"not-json".as_slice(),
        br#"{"activityId":"wrong"}"#.as_slice(),
    ] {
        let (client, peer) = server(200, "application/json", body).await;
        assert!(
            client
                .get_activity_service_activity_activity_id(42)
                .await
                .is_err()
        );
        peer.await.unwrap();
    }
}

#[tokio::test]
async fn raw_descriptor_download_discards_zip_and_returns_unit() {
    let (client, peer) = server(
        200,
        "application/x-zip-compressed",
        b"PK\x03\x04\x00\x80\xff\x00",
    )
    .await;
    let result: () = client
        .get_download_service_files_activity_activity_id(42)
        .await
        .unwrap();
    assert_eq!(result, ());
    peer.await.unwrap();
}

#[tokio::test]
async fn auth_throttling_redirects_and_server_failures_are_errors() {
    for status in [302, 401, 403, 429, 500] {
        let (client, peer) = server(status, "text/plain", b"not an activity").await;
        match client
            .get_download_service_files_activity_activity_id(42)
            .await
            .unwrap_err()
        {
            ApiOpError::Api(error) => assert_eq!(error.status, status),
            error => panic!("expected HTTP error, got {error:?}"),
        }
        peer.await.unwrap();
    }
}

#[tokio::test]
async fn documented_hrv_no_content_is_rejected() {
    let (client, peer) = server(204, "application/json", b"").await;
    match client
        .get_hrv_service_hrv_calendar_date("2026-11-10")
        .await
        .unwrap_err()
    {
        ApiOpError::Api(error) => assert_eq!(error.status, 204),
        error => panic!("expected HTTP error, got {error:?}"),
    }
    peer.await.unwrap();
}

#[tokio::test]
async fn response_size_limit_is_enforced() {
    let (client, peer) = server(200, "application/json", br#"{"activityId":42}"#).await;
    let result = client
        .with_max_response_body_bytes(8)
        .get_activity_service_activity_activity_id(42)
        .await;
    assert!(matches!(
        result,
        Err(ApiOpError::Transport(HttpError::ResponseTooLarge {
            limit: 8
        }))
    ));
    peer.await.unwrap();
}
