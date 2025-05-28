use crate::client::ClientConnection;
use crate::metrics::Metrics;
use axum::extract::ws::Message;
use std::{sync::Arc, time::Duration};
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::broadcast::Sender;
use tracing::{info, trace, warn};

const PING_INTERVAL: Duration = Duration::from_secs(10);
const PONG_TIMEOUT: Duration = Duration::from_secs(10);
const HEALTHCHECK_INTERVAL: Duration = Duration::from_secs(2);

#[derive(Clone)]
pub struct Registry {
    sender: Sender<String>,
    metrics: Arc<Metrics>,
}

impl Registry {
    pub fn new(sender: Sender<String>, metrics: Arc<Metrics>) -> Self {
        Self { sender, metrics }
    }

    pub async fn subscribe(&self, mut client: ClientConnection) {
        info!(message = "subscribing client", client = client.id());

        let mut receiver = self.sender.subscribe();
        let metrics = self.metrics.clone();
        metrics.new_connections.increment(1);

        tokio::spawn(async move {
            let mut ping_timer = tokio::time::interval(PING_INTERVAL);
            let mut healthcheck_timer = tokio::time::interval(HEALTHCHECK_INTERVAL);

            loop {
                tokio::select! {
                    // Forward messages from upstream to client
                    upstream_msg = receiver.recv() => {
                        match upstream_msg {
                            Ok(msg) => match client.send(msg.clone()).await {
                                Ok(_) => {
                                    trace!(message = "message sent to client", client = client.id());
                                    metrics.sent_messages.increment(1);
                                }
                                Err(e) => {
                                    warn!(
                                        message = "failed to send data to client",
                                        client = client.id(),
                                        error = e.to_string()
                                    );
                                    metrics.failed_messages.increment(1);
                                    break;
                                }
                            },
                            Err(RecvError::Closed) => {
                                info!(message = "upstream connection closed", client = client.id());
                                break;
                            }
                            Err(RecvError::Lagged(_)) => {
                                info!(message = "client is lagging", client = client.id());
                                metrics.lag_events.increment(1);
                                receiver = receiver.resubscribe();
                            }
                        }
                    }

                    // Handle incoming messages from client (pongs)
                    client_msg = client.recv() => {
                        match client_msg {
                            Some(Ok(msg)) => {
                                if let Message::Pong(_) = msg {
                                    trace!(message = "received pong from client", client = client.id());
                                    client.update_pong();
                                }
                            }
                            Some(Err(e)) => {
                                warn!(message = "error receiving message from client", client = client.id(), error = e.to_string());
                                break;
                            }
                            None => {
                                info!(message = "client connection closed", client = client.id());
                                break;
                            }
                        }
                    }

                    // Send pings to client periodically
                    _ = ping_timer.tick() => {
                        if let Err(e) = client.ping().await {
                            warn!(message = "failed to send ping to client", client = client.id(), error = e.to_string());
                            break;
                        }

                        trace!(message = "ping sent to client", client = client.id());
                    }

                    // Check client health
                    _ = healthcheck_timer.tick() => {
                        if !client.is_healthy(PONG_TIMEOUT) {
                            warn!(message = "client health check failed", client = client.id());
                            break;
                        }
                    }
                }
            }

            metrics.closed_connections.increment(1);
            info!(message = "client disconnected", client = client.id());
        });
    }
}
