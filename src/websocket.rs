use std::time::Instant;

use anyhow::{bail, Result};
use bytes::Bytes;
use futures::stream::{SplitSink, SplitStream};
use futures::{pin_mut, SinkExt, StreamExt};
use http_body::SizeHint;
use http_body_util::Full;
use hyper::body::Body;
use hyper::upgrade::Upgraded;
use hyper::{Request, Response, StatusCode};
use hyper_tungstenite::{upgrade, HyperWebsocket};
use hyper_util::rt::TokioIo;
use tokio::net::TcpStream;
use tokio::{spawn, try_join};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::MaybeTlsStream;
use tokio_tungstenite::{connect_async_with_config, WebSocketStream};
use url::Url;

use crate::metrics::{commit_http_metrics, SocketMetricsGuard};
use crate::{get_response, BAD_GATEWAY, RUNTIME_CONFIG};

type ServerWebSocket = WebSocketStream<MaybeTlsStream<TcpStream>>;
type TxServerSink = SplitSink<ServerWebSocket, Message>;
type TxClientSink = SplitSink<WebSocketStream<TokioIo<Upgraded>>, Message>;
type RxServerStream = SplitStream<ServerWebSocket>;
type RxClientStream = SplitStream<WebSocketStream<TokioIo<Upgraded>>>;

pub async fn handle_upgrade(
    app: &str,
    request: Request<impl Body>,
    start_time: &Instant,
    req_size: &SizeHint,
    ws_uri_string: &str,
) -> Result<Response<Full<Bytes>>> {
    let app = app.to_string();
    let method = request.method().clone();
    let (response, ws_client) = upgrade(request, Some(RUNTIME_CONFIG.get_websocket_config()))?;
    let ws_server = create_ws_server(ws_uri_string).await;

    if let Err(error) = ws_server {
        info!("method='Not yet decoded' uri='{}' status_code='502' user_sub='Not yet decoded' token_id='Not yet decoded' error='Websocket: {}'", ws_uri_string, error);
        return get_response(
            &app,
            &method,
            StatusCode::BAD_GATEWAY,
            BAD_GATEWAY,
            start_time,
            req_size,
        );
    }

    commit_http_metrics(
        &app,
        &method,
        start_time,
        response.status(),
        req_size,
        &response.size_hint(),
    );

    spawn(async move {
        if let Err(e) = serve_websocket(&app, ws_client, ws_server.unwrap()).await {
            warn!("event='Error in websocket connection: {:?}'", e);
        }
    });

    Ok(response)
}

async fn create_ws_server(ws_uri_string: &str) -> Result<ServerWebSocket> {
    let (ws_server, response) = connect_async_with_config(
        Url::parse(ws_uri_string)?,
        Some(RUNTIME_CONFIG.get_websocket_config()),
        false,
    )
    .await?;

    match response.status() {
        StatusCode::SWITCHING_PROTOCOLS => Ok(ws_server),
        status => bail!(
            "Unexpected status during socket initialization: {}",
            status.canonical_reason().unwrap_or_else(|| status.as_str()),
        ),
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
