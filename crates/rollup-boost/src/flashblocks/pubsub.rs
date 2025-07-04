use super::FlashblocksPayloadV1;
use futures::stream::{SplitSink, SplitStream};
use futures::{SinkExt, StreamExt};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::watch::error::RecvError;
use tokio::sync::{broadcast, watch};
use tokio::task::JoinHandle;
use tokio_tungstenite::tungstenite::{self, Message, Utf8Bytes};
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async};
use tokio_util::bytes::Bytes;
use url::Url;

pub struct FlashblocksPubSubManager {
    subscriber_handle: JoinHandle<Result<(), FlashblocksPubSubError>>,
    publisher_handle: JoinHandle<Result<(), FlashblocksPubSubError>>,
    payload_rx: broadcast::Receiver<FlashblocksPayloadV1>,
}

impl FlashblocksPubSubManager {
    pub fn spawn(
        builder_ws_endpoint: Url,
        listen_addr: SocketAddr,
    ) -> Result<Self, FlashblocksPubSubError> {
        let (payload_tx, payload_rx) = broadcast::channel(100);
        let (publisher_tx, publisher_rx) = broadcast::channel(100);

        Ok(Self {
            subscriber_handle: FlashblocksSubscriber::spawn(
                builder_ws_endpoint,
                payload_tx,
                publisher_tx,
            ),
            publisher_handle: FlashblocksPublisher::spawn(listen_addr, publisher_rx),
            payload_rx,
        })
    }

    pub fn payload_rx(&self) -> broadcast::Receiver<FlashblocksPayloadV1> {
        self.payload_rx.resubscribe()
    }
}

#[derive(Clone, Debug)]
pub struct FlashblocksSubscriber;

impl FlashblocksSubscriber {
    fn spawn(
        builder_ws_endpoint: Url,
        payload_tx: broadcast::Sender<FlashblocksPayloadV1>,
        publisher_tx: broadcast::Sender<Utf8Bytes>,
    ) -> JoinHandle<Result<(), FlashblocksPubSubError>> {
        let payload_tx = Arc::new(payload_tx);
        let publisher_tx = Arc::new(publisher_tx);

        tokio::spawn(async move {
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
                    payload_tx.clone(),
                    publisher_tx.clone(),
                    pong_tx,
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
        })
    }

    fn handle_flashblocks_stream(
        mut stream: SplitStream<WebSocketStream<MaybeTlsStream<TcpStream>>>,
        payload_tx: Arc<broadcast::Sender<FlashblocksPayloadV1>>,
        publisher_tx: Arc<broadcast::Sender<Utf8Bytes>>,
        pong_tx: watch::Sender<Message>,
    ) -> JoinHandle<Result<(), FlashblocksPubSubError>> {
        tokio::spawn(async move {
            while let Some(msg) = stream.next().await {
                let msg = msg.map_err(|e| {
                    tracing::error!("Ws connection error: {e}");
                    e
                })?;

                match msg {
                    Message::Text(bytes) => {
                        let flashblock_payload =
                            serde_json::from_str::<FlashblocksPayloadV1>(&bytes)?;
                        payload_tx.send(flashblock_payload)?;
                        publisher_tx.send(bytes)?;
                    }

                    Message::Pong(_) => {
                        pong_tx.send(Message::Pong(Bytes::default()))?;
                    }

                    Message::Close(_) => {
                        todo!("conection closed")
                    }

                    // TODO: handle other message types
                    _ => {}
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

pub struct FlashblocksPublisher;

impl FlashblocksPublisher {
    fn spawn(
        listen_addr: SocketAddr,
        publisher_rx: broadcast::Receiver<Utf8Bytes>,
    ) -> JoinHandle<Result<(), FlashblocksPubSubError>> {
        tokio::spawn(async move {
            let listener = TcpListener::bind(listen_addr)
                .await
                .expect("TODO: handle error");
            loop {
                let (stream, _) = listener.accept().await.expect("TODO: handle error");

                let rx = publisher_rx.resubscribe();
                tokio::spawn(Self::handle_connection(stream, rx));
            }
        })
    }

    async fn handle_connection(
        mut stream: TcpStream,
        mut publisher_rx: broadcast::Receiver<Utf8Bytes>,
    ) {
        loop {
            if let Ok(payload) = publisher_rx.recv().await {
            } else {
                todo!()
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
    RecvError(#[from] RecvError),
    #[error(transparent)]
    SerdeJsonError(#[from] serde_json::Error),
}
