use crate::rate_limit::Ticket;
use axum::Error;
use axum::extract::ws::WebSocket;
use std::net::IpAddr;

pub struct ClientConnection {
    client_addr: IpAddr,
    _ticket: Ticket,
    pub(crate) websocket: WebSocket,
}

impl ClientConnection {
    pub fn new(client_addr: IpAddr, ticket: Ticket, websocket: WebSocket) -> Self {
        Self {
            client_addr,
            _ticket: ticket,
            websocket,
        }
    }

    pub async fn send(&mut self, data: Vec<u8>) -> Result<(), Error> {
        self.websocket.send(data.into()).await
    }

    pub fn id(&self) -> String {
        self.client_addr.to_string()
    }
}
