use futures_util::TryStreamExt;
use garmin_connect_codegen::binary::{ApiOpError, HttpClient};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

async fn server(
    status: u16,
    media: &str,
    body: &[u8],
) -> (HttpClient, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let headers = indoc::formatdoc! {"
        HTTP/1.1 {status} Test
        Content-Type: {media}
        Content-Length: {}
        Connection: close

        ", body.len()
    }
    .replace('\n', "\r\n");
    let reply = [headers.as_bytes(), body].concat();
    let peer = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request: Vec<u8> = Vec::new();
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
    });
    let http = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(std::time::Duration::from_secs(3))
        .build()
        .unwrap();
    (HttpClient::with_client(http).with_base_url(url), peer)
}

#[tokio::test]
async fn annotated_zip_is_byte_exact_buffered_and_streamed() {
    let original = b"PK\x03\x04\x00\x80\xff\x00";
    let (client, peer) = server(200, "application/x-zip-compressed", original).await;
    let bytes = client
        .get_download_service_files_activity_activity_id(42)
        .await
        .unwrap();
    assert_eq!(bytes.as_ref(), original);
    peer.await.unwrap();
    let (client, peer) = server(200, "application/x-zip-compressed", original).await;
    let stream = client
        .get_download_service_files_activity_activity_id_binary_stream(42)
        .await
        .unwrap();
    let chunks: Vec<bytes::Bytes> = stream.try_collect().await.unwrap();
    assert_eq!(chunks.concat(), original);
    peer.await.unwrap();
}

#[tokio::test]
async fn annotated_stream_rejects_http_errors_and_wrong_media_type() {
    for (status, media) in [
        (302, "text/plain"),
        (401, "text/plain"),
        (403, "text/plain"),
        (429, "text/plain"),
        (500, "text/plain"),
        (200, "text/html"),
    ] {
        let (client, peer) = server(status, media, b"not an activity").await;
        let result = client
            .get_download_service_files_activity_activity_id_binary_stream(42)
            .await;
        match result {
            Err(ApiOpError::Api(error)) => assert_eq!(error.status, status),
            _ => panic!("expected HTTP/media error for {status} {media}"),
        }
        peer.await.unwrap();
    }
}
