use std::time::Instant;

use http_body::{Body as _, SizeHint};
use hyper::{Body, Request, Response, StatusCode};
use url::Url;

use hyper_tungstenite::{upgrade, HyperWebsocket};

use futures::stream::{SplitSink, SplitStream};
use futures::{pin_mut, SinkExt, StreamExt};
use hyper::upgrade::Upgraded;
use tokio::net::TcpStream;
use tokio::{spawn, try_join};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::MaybeTlsStream;
use tokio_tungstenite::{connect_async_with_config, WebSocketStream};

use crate::metrics::{commit_http_metrics, SocketMetricsGuard};
use crate::{get_response, BAD_GATEWAY, RUNTIME_CONFIG};

type GenericError = Box<dyn std::error::Error + Send + Sync>;
type Result<T> = std::result::Result<T, GenericError>;
type ServerWebSocket = WebSocketStream<MaybeTlsStream<TcpStream>>;

type TxServerSink = SplitSink<ServerWebSocket, Message>;
type TxClientSink = SplitSink<WebSocketStream<Upgraded>, Message>;
type RxServerStream = SplitStream<ServerWebSocket>;
type RxClientStream = SplitStream<WebSocketStream<Upgraded>>;

pub async fn handle_upgrade(
    request: Request<Body>,
    labels: &[&str],
    start_time: &Instant,
    req_size: &SizeHint,
    ws_uri_string: &str,
) -> Result<Response<Body>> {
    let app = labels[0].to_string(); // TODO: erk
    let (response, ws_client) = upgrade(request, Some(RUNTIME_CONFIG.get_websocket_config()))?;
    let ws_server = create_ws_server(ws_uri_string).await;

    if let Err(error) = ws_server {
        info!("method='Not yet decoded' uri='{}' status_code='502' user_sub='Not yet decoded' token_id='Not yet decoded' error='Websocket: {}'", ws_uri_string, error);
        return get_response(
            StatusCode::BAD_GATEWAY,
            BAD_GATEWAY,
            labels,
            start_time,
            req_size,
        );
    }
    spawn(async move {
        if let Err(e) = serve_websocket(&app, ws_client, ws_server.unwrap()).await {
            warn!("event='Error in websocket connection: {:?}'", e);
        }
    });

    commit_http_metrics(
        labels,
        start_time,
        response.status(),
        req_size,
        &response.size_hint(),
    );

    Ok(response)
}

async fn create_ws_server(ws_uri_string: &str) -> Result<ServerWebSocket> {
    let (ws_server, response) = connect_async_with_config(
        Url::parse(ws_uri_string)?,
        Some(RUNTIME_CONFIG.get_websocket_config()),
    )
    .await?;
    match response.status() {
        StatusCode::SWITCHING_PROTOCOLS => Ok(ws_server),
        status => Err(status
            .canonical_reason()
            .unwrap_or_else(|| status.as_str())
            .into()),
    }
}

async fn serve_websocket(
    app: &str,
    ws_client: HyperWebsocket,
    ws_server: ServerWebSocket,
) -> Result<()> {
    let ws_client = ws_client.await?;
    let (tx_client, rx_client) = ws_client.split();
    let (tx_server, rx_server) = ws_server.split();
    let socket_metrics = &SocketMetricsGuard::new(app);

    let client_to_server_closure =
        move |mut tx_server: TxServerSink, mut rx_client: RxClientStream| async move {
            async fn close_tx(tx_server: &mut TxServerSink) {
                if let Err(e) = tx_server.close().await {
                    warn!("event='Fail to close server socket: {:?}'", e);
                }
            }

            while let Some(message) = rx_client.next().await {
                match message {
                    Err(e) => {
                        warn!("event='Error in client message: {:?}'", e);
                        close_tx(&mut tx_server).await;
                        return Err(e);
                    }
                    Ok(message) => {
                        socket_metrics.commit_message_received(message.len());

                        if let Err(e) = tx_server.send(message).await {
                            warn!("event='Fail to send message to server: {:?}'", e);
                            close_tx(&mut tx_server).await;
                            return Err(e);
                        }
                    }
                };
            }

            Ok(())
        };

    let server_to_client_closure =
        move |mut tx_client: TxClientSink, mut rx_server: RxServerStream| async move {
            async fn close_tx(tx_client: &mut TxClientSink) {
                if let Err(e) = tx_client.close().await {
                    warn!("event='Fail to close server socket: {:?}'", e);
                }
            }
            while let Some(message) = rx_server.next().await {
                match message {
                    Err(e) => {
                        warn!("event='Error in server message: {:?}'", e);
                        close_tx(&mut tx_client).await;
                        return Err(e);
                    }
                    Ok(message) => {
                        socket_metrics.commit_message_sent(message.len());

                        if let Err(e) = tx_client.send(message).await {
                            warn!("event='Fail to send message to server: {:?}'", e);
                            close_tx(&mut tx_client).await;
                            return Err(e);
                        }
                    }
                }
            }
            Ok(())
        };

    let client_to_server = client_to_server_closure(tx_server, rx_client);
    let server_to_client = server_to_client_closure(tx_client, rx_server);

    pin_mut!(client_to_server, server_to_client);
    if let Err(e) = try_join!(client_to_server, server_to_client) {
        warn!("event='Websocket error: {:?}'", e)
    }
    Ok(())
}
