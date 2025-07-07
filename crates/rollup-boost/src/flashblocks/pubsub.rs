use super::FlashblocksPayloadV1;
use super::metrics::FlashblocksSubscriberMetrics;
use super::provider::FlashblocksProvider;
use futures::stream::{SplitSink, SplitStream};
use futures::{SinkExt, StreamExt};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::{broadcast, watch};
use tokio::task::JoinHandle;
use tokio_tungstenite::tungstenite::{self, Message, Utf8Bytes};
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async};
use tokio_util::bytes::Bytes;
use url::Url;

pub struct FlashblocksPubSubManager {
    subscriber: FlashblocksSubscriber,
    publisher: FlashblocksPublisher,
}

impl FlashblocksPubSubManager {
    pub fn spawn(
        builder_ws_endpoint: Url,
        listen_addr: SocketAddr,
        flashblocks_provider: Arc<FlashblocksProvider>,
    ) -> Result<Self, FlashblocksPubSubError> {
        let (payload_tx, payload_rx) = broadcast::channel(100);

        Ok(Self {
            subscriber: FlashblocksSubscriber::new(
                builder_ws_endpoint,
                payload_tx,
                flashblocks_provider,
            ),
            publisher: FlashblocksPublisher::new(listen_addr, payload_rx),
        })
    }
}

pub struct FlashblocksSubscriber {
    handle: JoinHandle<Result<(), FlashblocksPubSubError>>,
}

impl FlashblocksSubscriber {
    fn new(
        builder_ws_endpoint: Url,
        payload_tx: broadcast::Sender<Utf8Bytes>,
        flashblocks_provider: Arc<FlashblocksProvider>,
    ) -> Self {
        let payload_tx = Arc::new(payload_tx);
        let metrics = FlashblocksSubscriberMetrics::default();
        let handle = tokio::spawn(async move {
            loop {
                let Ok((ws_stream, _)) = connect_async(builder_ws_endpoint.as_str()).await else {
                    // TODO: log error
                    tokio::time::sleep(Duration::from_millis(500)).await;
                    continue;
                };

                let (sink, stream) = ws_stream.split();
                let (pong_tx, mut pong_rx) = watch::channel(Message::Pong(Bytes::default()));
                pong_rx.mark_changed();

                let ping_handle = spawn_ping(sink, pong_rx);
                let stream_handle = FlashblocksSubscriber::handle_flashblocks_stream(
                    stream,
                    flashblocks_provider.clone(),
                    payload_tx.clone(),
                    pong_tx,
                    metrics.clone(),
                );

                tokio::select! {
                    _ = ping_handle => {
                        tracing::warn!("Ping handle resolved early, re-establing connection");
                    }
                    _ = stream_handle => {
                        tracing::warn!("Flashblocks stream handle resolved early, re-establing connection");
                    }
                }

                tokio::time::sleep(Duration::from_millis(500)).await;
            }
        });

        Self { handle }
    }

    fn handle_flashblocks_stream(
        mut stream: SplitStream<WebSocketStream<MaybeTlsStream<TcpStream>>>,
        flashblocks_provider: Arc<FlashblocksProvider>,
        payload_tx: Arc<broadcast::Sender<Utf8Bytes>>,
        pong_tx: watch::Sender<Message>,
        metrics: FlashblocksSubscriberMetrics,
    ) -> JoinHandle<Result<(), FlashblocksPubSubError>> {
        tokio::spawn(async move {
            while let Some(msg) = stream.next().await {
                let msg = msg.map_err(|e| {
                    tracing::error!("Ws connection error: {e}");
                    e
                })?;

                match msg {
                    Message::Text(bytes) => {
                        // TODO: docs
                        if let Ok(flashblock) = serde_json::from_str::<FlashblocksPayloadV1>(&bytes)
                        {
                            let local_payload_id = flashblocks_provider.payload_id.lock();
                            if *local_payload_id == flashblock.payload_id {
                                let mut payload_builder =
                                    flashblocks_provider.payload_builder.lock();
                                let flashblock_index = flashblock.index;
                                if let Err(e) = payload_builder.extend(flashblock) {
                                    metrics.extend_payload_errors.increment(1);
                                    tracing::error!(
                                        target: "pubsub::handle_flashblocks_stream",
                                        message = "Failed to extend payload",
                                        error = %e,
                                        payload_id = %local_payload_id,
                                        index = flashblock_index
                                    );
                                    continue;
                                }
                            } else {
                                metrics.current_payload_id_mismatch.increment(1);
                                tracing::error!(
                                    target: "pubsub::handle_flashblocks_stream",
                                    message = "Payload ID mismatch",
                                    payload_id = %flashblock.payload_id,
                                    %local_payload_id,
                                    index = flashblock.index,
                                );
                                continue;
                            }
                        } else {
                            tracing::error!(
                                target: "pubsub::handle_flashblocks_stream",
                                message = "Failed deserialize payload",
                            );
                            continue;
                        }

                        payload_tx.send(bytes)?;
                    }
                    Message::Pong(_) => {
                        pong_tx.send(Message::Pong(Bytes::default()))?;
                    }
                    Message::Close(frame) => {
                        // TODO: report close reason and code
                        tracing::warn!(
                            target: "pubsub::handle_flashblocks_stream",
                            message = "Connection closed",
                        );
                    }
                    other => {
                        tracing::warn!(
                            target: "pubsub::handle_flashblocks_stream",
                            message = format!("Unexpected message {other}")
                        );
                    }
                }
            }

            Ok(())
        })
    }
}

fn spawn_ping(
    mut sink: SplitSink<WebSocketStream<MaybeTlsStream<TcpStream>>, Message>,
    pong_rx: tokio::sync::watch::Receiver<Message>,
) -> JoinHandle<Result<(), FlashblocksPubSubError>> {
    tokio::spawn(async move {
        let mut ping_interval = tokio::time::interval(Duration::from_millis(500));
        loop {
            ping_interval.tick().await;
            if pong_rx.has_changed()? {
                sink.send(Message::Ping(Bytes::new()))
                    .await
                    .map_err(|_| FlashblocksPubSubError::PingFailed)?;
            } else {
                tracing::error!("Missing pong response from builder stream");
                return Err(FlashblocksPubSubError::MissingPong);
            }
        }
    })
}

pub struct FlashblocksPublisher {
    handle: JoinHandle<Result<(), FlashblocksPubSubError>>,
}

impl FlashblocksPublisher {
    fn new(listen_addr: SocketAddr, publisher_rx: broadcast::Receiver<Utf8Bytes>) -> Self {
        let handle = tokio::spawn(async move {
            let listener = TcpListener::bind(listen_addr)
                .await
                .expect("Could not bind publisher to listener addr");

            loop {
                // TODO: handle error
                let (tcp_stream, _) = listener.accept().await.expect("TODO: handle error");

                let ws_stream = tokio_tungstenite::accept_async(tcp_stream).await?;
                let rx = publisher_rx.resubscribe();
                tokio::spawn(Self::handle_connection(ws_stream, rx));
            }
        });

        Self { handle }
    }

    async fn handle_connection(
        mut stream: WebSocketStream<TcpStream>,
        mut publisher_rx: broadcast::Receiver<Utf8Bytes>,
    ) {
        loop {
            match publisher_rx.recv().await {
                Ok(payload) => {
                    // Here you would typically do any transformation or logging.
                    if let Err(e) = stream.send(Message::Text(payload)).await {
                        // If sending fails, close the connection.
                        tracing::debug!("Closing flashblocks subscription: {e}");
                        break;
                    }
                }
                Err(RecvError::Closed) => {
                    tracing::debug!("Broadcast channel closed, exiting subscription loop");
                    return;
                }
                Err(RecvError::Lagged(skipped)) => {
                    tracing::warn!("Broadcast channel lagged, skipped {skipped} messages");
                }
            }
        }
    }
}

#[derive(thiserror::Error, Debug)]
pub enum FlashblocksPubSubError {
    #[error("Ping failed")]
    PingFailed,
    #[error("Missing pong response")]
    MissingPong,
    #[error(transparent)]
    ConnectError(#[from] tungstenite::Error),
    #[error(transparent)]
    FlashblocksPayloadSendError(#[from] broadcast::error::SendError<FlashblocksPayloadV1>),
    #[error(transparent)]
    MessageSendError(#[from] watch::error::SendError<Message>),
    #[error(transparent)]
    Utf8BytesSendError(#[from] broadcast::error::SendError<Utf8Bytes>),
    #[error(transparent)]
    RecvError(#[from] watch::error::RecvError),
}

#[cfg(test)]
mod tests {

    use std::sync::Arc;

    use futures::{SinkExt, StreamExt};
    use http::Uri;
    use reth_rpc_layer::JwtSecret;
    use tokio::{net::TcpListener, sync::broadcast, task::JoinHandle};
    use tokio_tungstenite::{accept_async, tungstenite::Message};
    use url::Url;

    use crate::{
        PayloadSource, RpcClient,
        provider::FlashblocksProvider,
        pubsub::{FlashblocksPubSubError, FlashblocksSubscriber, spawn_ping},
    };

    pub struct MockClient {
        addr: std::net::SocketAddr,
        handle: tokio::task::JoinHandle<eyre::Result<()>>,
    }

    impl MockClient {
        pub async fn spawn(pong: bool) -> eyre::Result<Self> {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
            let addr = listener.local_addr()?;

            let handle = tokio::spawn(async move {
                loop {
                    let (tcp, _) = listener.accept().await.expect("accept failed");
                    let mut ws = tokio_tungstenite::accept_async(tcp).await?;

                    while let Some(msg) = ws.next().await {
                        let msg = match msg {
                            Ok(m) => m,
                            Err(_) => break,
                        };

                        match msg {
                            Message::Ping(payload) => {
                                if pong {
                                    ws.send(Message::Pong(payload)).await?;
                                }
                            }
                            Message::Close(_) => break,
                            _ => {}
                        }
                    }
                }
            });

            Ok(Self { addr, handle })
        }

        pub fn ws_url(&self) -> Url {
            Url::parse(&format!("ws://{}", self.addr)).expect("invalid URL")
        }
    }

    impl Drop for MockClient {
        fn drop(&mut self) {
            self.handle.abort();
        }
    }

    #[tokio::test]
    async fn test_ping_pong() -> eyre::Result<()> {
        let mock = MockClient::spawn(true).await?;

        let rpc_client = RpcClient::new(
            "http://localhost:8545".parse().unwrap(),
            JwtSecret::random(),
            1000,
            PayloadSource::Builder,
        )?;

        let provider = Arc::new(FlashblocksProvider::new(rpc_client));
        let (tx, _rx) = broadcast::channel(10);
        let subscriber = FlashblocksSubscriber::new(mock.ws_url(), tx, provider);

        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        assert!(!subscriber.handle.is_finished());

        Ok(())
    }

    #[tokio::test]
    async fn test_missing_pong() -> eyre::Result<()> {
        let mock = MockClient::spawn(false).await?;

        let rpc_client = RpcClient::new(
            "http://localhost:8545".parse().unwrap(),
            JwtSecret::random(),
            1000,
            PayloadSource::Builder,
        )?;

        let provider = Arc::new(FlashblocksProvider::new(rpc_client));
        let (tx, _rx) = broadcast::channel(10);
        let subscriber = FlashblocksSubscriber::new(mock.ws_url(), tx, provider);

        tokio::time::sleep(std::time::Duration::from_secs(1)).await;

        let res = subscriber.handle.await?;
        matches!(res, Err(FlashblocksPubSubError::MissingPong));

        Ok(())
    }

    #[test]
    fn test_payload_id_mismatch() {
        todo!()
    }

    #[test]
    fn current_payload_id_mismatch() {
        todo!()
    }

    #[test]
    fn test_malformed_flashblocks_payload() {
        todo!()
    }

    #[test]
    // NOTE: connection should be closed and re-established
    fn test_publisher_stream_closed() {
        todo!()
    }

    #[test]
    // NOTE: connection should stay open
    fn test_publisher_stream_lagged() {
        todo!()
    }
}
