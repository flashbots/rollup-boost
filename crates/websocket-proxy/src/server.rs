use crate::auth::Authentication;
use crate::client::ClientConnection;
use crate::metrics::Metrics;
use crate::rate_limit::{RateLimit, RateLimitError};
use crate::registry::Registry;
use axum::body::Body;
use axum::extract::{ConnectInfo, Path, Query, State, WebSocketUpgrade};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{any, get};
use axum::{Error, Router};
use http::{HeaderMap, HeaderValue};
use serde_json::json;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use jsonwebtoken::{decode, Algorithm, DecodingKey, Validation};
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub struct JwtClaims {
    pub sub: String, // Subject (user identifier)
    pub exp: usize,  // Expiration time
    pub iat: usize,  // Issued at
    pub user_id: Option<u32>,
    pub username: String,
    pub app_id: Option<String>, // Optional application identifier
}

#[derive(Clone)]
pub struct JwtConfig {
    pub secret: String,
    pub algorithm: Algorithm,
}

impl JwtConfig {
    pub fn new(secret: String) -> Self {
        Self {
            secret,
            algorithm: Algorithm::HS256,
        }
    }

    pub fn validate_token(&self, token: &str) -> Result<JwtClaims, jsonwebtoken::errors::Error> {
        let mut validation = Validation::new(self.algorithm);
        validation.validate_exp = true;

        decode::<JwtClaims>(
            token,
            &DecodingKey::from_secret(self.secret.as_ref()),
            &validation,
        )
        .map(|data| data.claims)
    }
}

#[derive(Deserialize)]
struct AuthQuery {
    token: Option<String>,
}

#[derive(Clone)]
struct ServerState {
    registry: Registry,
    rate_limiter: Arc<dyn RateLimit>,
    metrics: Arc<Metrics>,
    auth: Authentication,
    jwt_config: Option<JwtConfig>,
    ip_addr_http_header: String,
}

#[derive(Clone)]
pub struct Server {
    listen_addr: SocketAddr,
    registry: Registry,
    rate_limiter: Arc<dyn RateLimit>,
    metrics: Arc<Metrics>,
    ip_addr_http_header: String,
    authentication: Option<Authentication>,
    jwt_config: Option<JwtConfig>,
}

impl Server {
    pub fn new(
        listen_addr: SocketAddr,
        registry: Registry,
        metrics: Arc<Metrics>,
        rate_limiter: Arc<dyn RateLimit>,
        authentication: Option<Authentication>,
        ip_addr_http_header: String,
        jwt_secret: Option<String>,
    ) -> Self {
        Self {
            listen_addr,
            registry,
            rate_limiter,
            metrics,
            authentication,
            ip_addr_http_header,
            jwt_config: jwt_secret.map(JwtConfig::new),
        }
    }

    pub async fn listen(&self, cancellation_token: CancellationToken) {
        let mut router: Router<ServerState> = Router::new().route("/healthz", get(healthz_handler));

        // Determine which authentication method to use
        match (&self.authentication, &self.jwt_config) {
            (Some(_), Some(_)) => {
                info!("Both API key and JWT authentication are enabled - JWT takes precedence");
                router = router
                    .route("/ws", any(jwt_websocket_handler))
                    .route("/ws/apikey/:api_key", any(authenticated_websocket_handler));
            }
            (Some(_), None) => {
                info!("API key authentication is enabled");
                router = router.route("/ws/:api_key", any(authenticated_websocket_handler));
            }
            (None, Some(_)) => {
                info!("JWT authentication is enabled");
                router = router.route("/ws", any(jwt_websocket_handler));
            }
            (None, None) => {
                info!("Public endpoint is enabled");
                router = router.route("/ws", any(unauthenticated_websocket_handler));
            }
        }

        let router = router.with_state(ServerState {
            registry: self.registry.clone(),
            rate_limiter: self.rate_limiter.clone(),
            metrics: self.metrics.clone(),
            auth: self
                .authentication
                .clone()
                .unwrap_or_else(Authentication::none),
            jwt_config: self.jwt_config.clone(),
            ip_addr_http_header: self.ip_addr_http_header.clone(),
        });

        let listener = tokio::net::TcpListener::bind(self.listen_addr)
            .await
            .unwrap();

        info!(
            message = "starting server",
            address = listener.local_addr().unwrap().to_string()
        );

        axum::serve(
            listener,
            router.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .with_graceful_shutdown(cancellation_token.cancelled_owned())
        .await
        .unwrap()
    }
}

async fn healthz_handler() -> impl IntoResponse {
    StatusCode::OK
}

async fn jwt_websocket_handler(
    State(state): State<ServerState>,
    ws: WebSocketUpgrade,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Query(params): Query<AuthQuery>,
) -> impl IntoResponse {
    let jwt_config = match &state.jwt_config {
        Some(config) => config,
        None => {
            return Response::builder()
                .status(StatusCode::INTERNAL_SERVER_ERROR)
                .body(Body::from(
                    json!({"message": "JWT authentication not configured"}).to_string(),
                ))
                .unwrap();
        }
    };

    // Tries to get token from param and use header as fallback
    let token = params.token.or_else(|| {
        headers
            .get("Authorization")
            .and_then(|header| header.to_str().ok())
            .and_then(|header_str| {
                if header_str.starts_with("Bearer ") {
                    Some(header_str[7..].to_string())
                } else {
                    None
                }
            })
    });

    let token = match token {
        Some(token) => token,
        None => {
            state.metrics.unauthorized_requests.increment(1);
            return Response::builder()
                .status(StatusCode::UNAUTHORIZED)
                .body(Body::from(
                    json!({"message": "Missing JWT token. Provide via ?token=<jwt> or Authorization: Bearer <jwt>"}).to_string(),
                ))
                .unwrap();
        }
    };

    match jwt_config.validate_token(&token) {
        Ok(claims) => {
            info!(
                message = "JWT authentication successful",
                user = claims.username,
                user_id = claims.user_id,
                client = addr.to_string()
            );

            if let Some(app_id) = &claims.app_id {
                state.metrics.proxy_connections_by_app(app_id);
            }

            websocket_handler_with_user_info(state, ws, addr, headers, Some(claims))
        }
        Err(e) => {
            warn!(
                message = "JWT validation failed",
                error = e.to_string(),
                client = addr.to_string()
            );

            state.metrics.unauthorized_requests.increment(1);
            Response::builder()
                .status(StatusCode::UNAUTHORIZED)
                .body(Body::from(
                    json!({"message": format!("Invalid JWT token: {}", e)}).to_string(),
                ))
                .unwrap()
        }
    }
}

async fn authenticated_websocket_handler(
    State(state): State<ServerState>,
    ws: WebSocketUpgrade,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Path(api_key): Path<String>,
) -> impl IntoResponse {
    let application = state.auth.get_application_for_key(&api_key);

    match application {
        None => {
            state.metrics.unauthorized_requests.increment(1);

            Response::builder()
                .status(StatusCode::UNAUTHORIZED)
                .body(Body::from(
                    json!({"message": "Invalid API key"}).to_string(),
                ))
                .unwrap()
        }
        Some(app) => {
            state.metrics.proxy_connections_by_app(app);
            websocket_handler_with_user_info(state, ws, addr, headers, None)
        }
    }
}

async fn unauthenticated_websocket_handler(
    State(state): State<ServerState>,
    ws: WebSocketUpgrade,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> impl IntoResponse {
    websocket_handler_with_user_info(state, ws, addr, headers, None)
}

fn websocket_handler_with_user_info(
    state: ServerState,
    ws: WebSocketUpgrade,
    addr: SocketAddr,
    headers: HeaderMap,
    jwt_claims: Option<JwtClaims>,
) -> Response {
    let connect_addr = addr.ip();

    let client_addr = match headers.get(&state.ip_addr_http_header) {
        None => connect_addr,
        Some(value) => extract_addr(value, connect_addr),
    };

    let ticket = match state.rate_limiter.try_acquire(client_addr) {
        Ok(ticket) => ticket,
        Err(RateLimitError::Limit { reason }) => {
            state.metrics.rate_limited_requests.increment(1);

            return Response::builder()
                .status(StatusCode::TOO_MANY_REQUESTS)
                .body(Body::from(json!({"message": reason}).to_string()))
                .unwrap();
        }
    };

    ws.on_failed_upgrade(move |e: Error| {
        info!(
            message = "failed to upgrade connection",
            error = e.to_string(),
            client = addr.to_string()
        )
    })
    .on_upgrade(async move |socket| {
        let client = ClientConnection::new(client_addr, ticket, socket);

        if let Some(claims) = jwt_claims {
            info!(
                message = "WebSocket connection established with JWT user",
                user = claims.username,
                user_id = claims.user_id,
                client = addr.to_string()
            );
        }

        state.registry.subscribe(client).await;
    })
}

fn extract_addr(header: &HeaderValue, fallback: IpAddr) -> IpAddr {
    if header.is_empty() {
        return fallback;
    }

    match header.to_str() {
        Ok(header_value) => {
            let raw_value = header_value
                .split(',')
                .map(|ip| ip.trim().to_string())
                .next_back();

            if let Some(raw_value) = raw_value {
                return raw_value.parse::<IpAddr>().unwrap_or(fallback);
            }

            fallback
        }
        Err(e) => {
            warn!(
                message = "could not get header value",
                error = e.to_string()
            );
            fallback
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Duration, Utc};
    use jsonwebtoken::{encode, EncodingKey, Header};
    use std::net::Ipv4Addr;

    #[tokio::test]
    async fn test_header_addr() {
        let fb = Ipv4Addr::new(127, 0, 0, 1);

        let test = |header: &str, expected: Ipv4Addr| {
            let hv = HeaderValue::from_str(header).unwrap();
            let result = extract_addr(&hv, IpAddr::V4(fb));
            assert_eq!(result, expected);
        };

        test("129.1.1.1", Ipv4Addr::new(129, 1, 1, 1));
        test("129.1.1.1,130.1.1.1", Ipv4Addr::new(130, 1, 1, 1));
        test("129.1.1.1  ,  130.1.1.1   ", Ipv4Addr::new(130, 1, 1, 1));
        test("nonsense", fb);
        test("400.0.0.1", fb);
        test("120.0.0.1.0", fb);
    }

    #[test]
    fn test_jwt_validation() {
        let secret = "test-secret";
        let jwt_config = JwtConfig::new(secret.to_string());

        let now = Utc::now();
        let exp = now + Duration::hours(1);

        let claims = JwtClaims {
            sub: "testuser".to_string(),
            exp: exp.timestamp() as usize,
            iat: now.timestamp() as usize,
            user_id: Some(123),
            username: "testuser".to_string(),
            app_id: Some("test-app".to_string()),
        };

        let token = encode(
            &Header::default(),
            &claims,
            &EncodingKey::from_secret(secret.as_ref()),
        )
        .unwrap();

        let validated_claims = jwt_config.validate_token(&token).unwrap();
        assert_eq!(validated_claims.username, "testuser");
        assert_eq!(validated_claims.user_id, Some(123));
        assert_eq!(validated_claims.app_id, Some("test-app".to_string()));
    }

    #[test]
    fn test_expired_jwt() {
        let secret = "test-secret";
        let jwt_config = JwtConfig::new(secret.to_string());

        let now = Utc::now();
        let exp = now - Duration::hours(1); // Expired token

        let claims = JwtClaims {
            sub: "testuser".to_string(),
            exp: exp.timestamp() as usize,
            iat: now.timestamp() as usize,
            user_id: Some(123),
            username: "testuser".to_string(),
            app_id: None,
        };

        let token = encode(
            &Header::default(),
            &claims,
            &EncodingKey::from_secret(secret.as_ref()),
        )
        .unwrap();

        assert!(jwt_config.validate_token(&token).is_err());
    }
}
