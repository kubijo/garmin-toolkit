use crate::devices::Host;
use axum::{
    Router,
    body::Bytes,
    extract::{
        State,
        ws::{Message, WebSocket, WebSocketUpgrade},
    },
    response::{Html, Response},
    routing::{any, get},
};
use futures_util::{SinkExt as _, StreamExt as _, future};
use garmin_service_api::DeviceServiceServerShared;
use remoc::{codec, prelude::*};
use std::{net::SocketAddr, path::PathBuf, sync::Arc};
use tower_http::services::ServeDir;

const ADDRESS_ENVIRONMENT: &str = "GARMIN_TOOLKIT_HASS_ADDRESS";
const WEB_ROOT_ENVIRONMENT: &str = "GARMIN_TOOLKIT_HASS_WEB_ROOT";

pub(super) async fn serve(host: Arc<Host>) -> Result<(), std::io::Error> {
    let address = std::env::var(ADDRESS_ENVIRONMENT)
        .unwrap_or_else(|_| "127.0.0.1:8099".to_owned())
        .parse::<SocketAddr>()
        .map_err(std::io::Error::other)?;
    let listener = tokio::net::TcpListener::bind(address).await?;
    tracing::info!(%address, "Home Assistant host listening");
    axum::serve(listener, router(host)).await
}

fn router(host: Arc<Host>) -> Router {
    let app = Router::new()
        .route("/remoc", any(websocket))
        .route("/health", get(|| async { "ok" }))
        .with_state(host);
    match std::env::var_os(WEB_ROOT_ENVIRONMENT).map(PathBuf::from) {
        Some(root) => {
            app.fallback_service(ServeDir::new(root).append_index_html_on_directories(true))
        }
        None => app.route(
            "/",
            get(|| async {
                Html("Garmin Toolkit host is running; no browser bundle was configured.")
            }),
        ),
    }
}

async fn websocket(State(host): State<Arc<Host>>, upgrade: WebSocketUpgrade) -> Response {
    upgrade.on_upgrade(move |socket| async move {
        if let Err(error) = serve_client(socket, host).await {
            tracing::warn!(%error, "Remoc client connection failed");
        }
    })
}

async fn serve_client(socket: WebSocket, host: Arc<Host>) -> anyhow::Result<()> {
    let (websocket_tx, websocket_rx) = socket.split();
    let transport_tx = websocket_tx
        .with(|packet: Bytes| future::ready(Ok::<_, axum::Error>(Message::Binary(packet))));
    let transport_rx = websocket_rx.filter_map(|message| {
        future::ready(match message {
            Ok(Message::Binary(packet)) => Some(Ok(packet)),
            Ok(_) => None,
            Err(error) => Some(Err(error)),
        })
    });
    let (server, client) = DeviceServiceServerShared::<_, codec::Default>::new(host);
    remoc::Connect::framed(remoc::Cfg::default(), transport_tx, transport_rx)
        .provide(client)
        .await
        .map_err(|error| anyhow::anyhow!("could not establish Remoc connection: {error}"))?;
    server
        .serve()
        .await
        .map_err(|error| anyhow::anyhow!("could not serve device service: {error}"))
}

#[cfg(test)]
mod tests {
    use super::router;
    use crate::devices::{DemoSource, Host};
    use axum::body::Bytes;
    use futures_util::{SinkExt as _, StreamExt as _, future};
    use garmin_service_api::{DeviceService as _, DeviceServiceClient, InspectionState};
    use remoc::prelude::*;
    use tokio_tungstenite::{connect_async, tungstenite::Message};

    #[test]
    fn host_routes_are_constructible() {
        let _router = router(Host::new(Box::new(DemoSource::new())));
    }

    #[tokio::test]
    async fn websocket_carries_device_snapshots_and_inspection() -> anyhow::Result<()> {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        let server = tokio::spawn(async move {
            axum::serve(listener, router(Host::new(Box::new(DemoSource::new())))).await
        });
        let (socket, _) = connect_async(format!("ws://{address}/remoc")).await?;
        let (socket_tx, socket_rx) = socket.split();
        let transport_tx = socket_tx.with(|packet: Bytes| {
            future::ready(Ok::<_, tokio_tungstenite::tungstenite::Error>(
                Message::Binary(packet),
            ))
        });
        let transport_rx = socket_rx.filter_map(|message| {
            future::ready(match message {
                Ok(Message::Binary(packet)) => Some(Ok(packet)),
                Ok(_) => None,
                Err(error) => Some(Err(error)),
            })
        });
        let client: DeviceServiceClient =
            remoc::Connect::framed(remoc::Cfg::default(), transport_tx, transport_rx)
                .consume()
                .await?;
        let mut snapshots = client.watch().await?;

        assert_eq!(
            snapshots.borrow()?.first().map(|device| device.inspection),
            Some(InspectionState::Available)
        );
        client.inspect("demo:fenix-8".to_owned()).await?;
        snapshots.changed().await?;
        let update = snapshots.borrow_and_update()?;
        let device = update.first().expect("the demo device remains attached");

        assert_eq!(device.inspection, InspectionState::Ready);
        assert_eq!(
            device
                .storages
                .first()
                .and_then(|storage| storage.capacity.bytes()),
            Some((32_000_000_000, 8_600_000_000))
        );

        server.abort();
        Ok(())
    }
}
