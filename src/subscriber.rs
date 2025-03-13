use crate::metrics::Metrics;
use axum::http::Uri;
use backoff::{backoff::Backoff, ExponentialBackoff};
use futures::StreamExt;
use std::sync::Arc;
use std::time::Duration;
use tokio::select;
use tokio_tungstenite::{connect_async, tungstenite::Error};
use tokio_util::sync::CancellationToken;
use tracing::{error, info, trace, warn};

pub struct WebsocketSubscriber<F>
where
    F: Fn(String) + Send + Sync + 'static,
{
    uri: Uri,
    handler: F,
    backoff: ExponentialBackoff,
    metrics: Arc<Metrics>,
}

impl<F> WebsocketSubscriber<F>
where
    F: Fn(String) + Send + Sync + 'static,
{
    pub fn new(uri: Uri, handler: F, max_interval: u64, metrics: Arc<Metrics>) -> Self {
        let backoff = ExponentialBackoff {
            initial_interval: Duration::from_secs(1),
            max_interval: Duration::from_secs(max_interval),
            max_elapsed_time: None, // Will retry indefinitely
            ..Default::default()
        };

        Self {
            uri,
            handler,
            backoff,
            metrics,
        }
    }

    pub async fn run(&mut self, token: CancellationToken) {
        // Added the URI to the log message for better identification
        info!(
            message = "starting upstream subscription",
            uri = self.uri.to_string()
        );
        loop {
            select! {
                _ = token.cancelled() => {
                    info!(
                        message = "cancelled upstream subscription",
                        uri = self.uri.to_string()
                    );
                    return;
                }
                result = self.connect_and_listen() => {
                    match result {
                        Ok(()) => {
                            info!(
                                message = "upstream connection closed",
                                uri = self.uri.to_string()
                            );
                        }
                        Err(e) => {
                            // Added URI to the error log for better debugging
                            error!(
                                message = "upstream websocket error", 
                                uri = self.uri.to_string(), 
                                error = e.to_string()
                            );
                            self.metrics.upstream_errors.increment(1);
                            // Decrement the active connections count when connection fails
                            self.metrics.upstream_connections.decrement(1);

                            if let Some(duration) = self.backoff.next_backoff() {
                                // Added URI to the warning message
                                warn!(
                                    message = "reconnecting", 
                                    uri = self.uri.to_string(), 
                                    seconds = duration.as_secs()
                                );
                                select! {
                                    _ = token.cancelled() => {
                                        info!(
                                            message = "cancelled subscriber during backoff", 
                                            uri = self.uri.to_string()
                                        );
                                        return
                                    }
                                    _ = tokio::time::sleep(duration) => {}
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    async fn connect_and_listen(&mut self) -> Result<(), Error> {
        info!(
            message = "connecting to websocket",
            uri = self.uri.to_string()
        );

        // Increment connection attempts counter for metrics
        self.metrics.upstream_connection_attempts.increment(1);

        // Modified connection with success/failure metrics tracking
        let (ws_stream, _) = match connect_async(&self.uri).await {
            Ok(connection) => {
                // Track successful connections
                self.metrics.upstream_connection_successes.increment(1);
                connection
            },
            Err(e) => {
                // Track failed connections
                self.metrics.upstream_connection_failures.increment(1);
                return Err(e);
            }
        };

        info!(
            message = "websocket connection established", 
            uri = self.uri.to_string()
        );

        // Increment active connections counter
        self.metrics.upstream_connections.increment(1);
        // Reset backoff timer on successful connection
        self.backoff.reset();

        let (_, mut read) = ws_stream.split();

        while let Some(message) = read.next().await {
            match message {
                Ok(msg) => {
                    let text = msg.to_text()?;
                    trace!(
                        message = "received message", 
                        uri = self.uri.to_string(), 
                        payload = text
                    );
                    self.metrics.upstream_messages.increment(1);
                    (self.handler)(text.into());
                }
                Err(e) => {
                    error!(
                        message = "error receiving message", 
                        uri = self.uri.to_string(), 
                        error = e.to_string()
                    );
                    return Err(e);
                }
            }
        }

        Ok(())
    }
}
