use crate::rate_limit::Ticket;
use axum::extract::ws::Message;
use axum::extract::ws::WebSocket;
use axum::Error;
use std::net::IpAddr;
use std::time::Duration;
use std::time::Instant;

pub struct ClientConnection {
    client_addr: IpAddr,
    _ticket: Ticket,
    pub(crate) websocket: WebSocket,
    last_pong: Instant,
}

impl ClientConnection {
    pub fn new(client_addr: IpAddr, ticket: Ticket, websocket: WebSocket) -> Self {
        Self {
            client_addr,
            _ticket: ticket,
            websocket,
            last_pong: Instant::now(),
        }
    }

    pub async fn send(&mut self, data: String) -> Result<(), Error> {
        self.websocket.send(data.into_bytes().into()).await
    }

    pub async fn recv(&mut self) -> Option<Result<Message, Error>> {
        self.websocket.recv().await
    }

    pub async fn ping(&mut self) -> Result<(), Error> {
        self.websocket.send(Message::Ping(vec![].into())).await
    }

    pub fn update_pong(&mut self) {
        self.last_pong = Instant::now();
    }

    pub fn is_healthy(&self, timeout: Duration) -> bool {
        self.last_pong.elapsed() < timeout
    }

    pub fn id(&self) -> String {
        self.client_addr.to_string()
    }
}
