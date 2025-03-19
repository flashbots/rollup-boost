use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::{Arc, Mutex};
use tracing::{debug, warn};

use thiserror::Error;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

use redis::{Client, Commands, RedisError};
use tracing::error;

#[derive(Error, Debug)]
pub enum RateLimitError {
    #[error("Rate Limit Reached: {reason}")]
    Limit { reason: String },
}

#[clippy::has_significant_drop]
pub struct Ticket {
    addr: IpAddr,
    _permit: OwnedSemaphorePermit,
    rate_limiter: Arc<dyn RateLimit>,
}

impl Drop for Ticket {
    fn drop(&mut self) {
        self.rate_limiter.release(self.addr)
    }
}

pub trait RateLimit: Send + Sync {
    fn try_acquire(self: Arc<Self>, addr: IpAddr) -> Result<Ticket, RateLimitError>;

    fn release(&self, ticket: IpAddr);
}

struct Inner {
    active_connections: HashMap<IpAddr, usize>,
    semaphore: Arc<Semaphore>,
}

pub struct InMemoryRateLimit {
    per_ip_limit: usize,
    inner: Mutex<Inner>,
}

impl InMemoryRateLimit {
    pub fn new(global_limit: usize, per_ip_limit: usize) -> Self {
        Self {
            per_ip_limit,
            inner: Mutex::new(Inner {
                active_connections: HashMap::new(),
                semaphore: Arc::new(Semaphore::new(global_limit)),
            }),
        }
    }
}

impl RateLimit for InMemoryRateLimit {
    fn try_acquire(self: Arc<Self>, addr: IpAddr) -> Result<Ticket, RateLimitError> {
        let mut inner = self.inner.lock().unwrap();

        let permit =
            inner
                .semaphore
                .clone()
                .try_acquire_owned()
                .map_err(|_| RateLimitError::Limit {
                    reason: "Global limit".to_owned(),
                })?;

        let current_count = match inner.active_connections.get(&addr) {
            Some(count) => *count,
            None => 0,
        };

        if current_count + 1 > self.per_ip_limit {
            debug!(
                message = "Rate limit exceeded, trying to acquire",
                client = addr.to_string()
            );
            return Err(RateLimitError::Limit {
                reason: String::from("IP limit exceeded"),
            });
        }

        let new_count = current_count + 1;

        inner.active_connections.insert(addr, new_count);

        Ok(Ticket {
            addr,
            _permit: permit,
            rate_limiter: self.clone(),
        })
    }

    fn release(&self, addr: IpAddr) {
        let mut inner = self.inner.lock().unwrap();

        let current_count = match inner.active_connections.get(&addr) {
            Some(count) => *count,
            None => 0,
        };

        let new_count = if current_count == 0 {
            warn!(
                message = "ip counting is not accurate -- unexpected underflow",
                client = addr.to_string()
            );
            0
        } else {
            current_count - 1
        };

        if new_count == 0 {
            inner.active_connections.remove(&addr);
        } else {
            inner.active_connections.insert(addr, new_count);
        }
    }
}

pub struct RedisRateLimit {
    redis_client: Client,
    global_limit: usize,
    per_ip_limit: usize,
    semaphore: Arc<Semaphore>,
    key_prefix: String,
}

impl RedisRateLimit {
    pub fn new(
        redis_url: &str,
        global_limit: usize,
        per_ip_limit: usize,
        key_prefix: &str,
    ) -> Result<Self, RedisError> {
        let client = Client::open(redis_url)?;

        Ok(Self {
            redis_client: client,
            global_limit,
            per_ip_limit,
            semaphore: Arc::new(Semaphore::new(global_limit)),
            key_prefix: key_prefix.to_string(),
        })
    }

    /// Get Redis key for tracking global connections
    fn global_key(&self) -> String {
        format!("{}:global:connections", self.key_prefix)
    }

    /// Get Redis key for tracking connections per IP
    fn ip_key(&self, addr: &IpAddr) -> String {
        format!("{}:ip:{}:connections", self.key_prefix, addr)
    }
}

impl RateLimit for RedisRateLimit {
    fn try_acquire(self: Arc<Self>, addr: IpAddr) -> Result<Ticket, RateLimitError> {
        // Get Redis connection first to check current counts
        let mut conn = match self.redis_client.get_connection() {
            Ok(conn) => conn,
            Err(e) => {
                error!(
                    message = "Failed to connect to Redis",
                    error = e.to_string()
                );
                return Err(RateLimitError::Limit {
                    reason: "Redis connection failed".to_string(),
                });
            }
        };

        // Check global count BEFORE incrementing
        let global_connections: usize = conn.get(self.global_key()).unwrap_or(0);

        if global_connections >= self.global_limit {
            debug!(
                message = "Global limit reached",
                global_connections = global_connections,
                global_limit = self.global_limit
            );
            return Err(RateLimitError::Limit {
                reason: "Global connection limit reached".to_string(),
            });
        }

        // Check IP count BEFORE incrementing
        let ip_connections: usize = conn.get(self.ip_key(&addr)).unwrap_or(0);
        if ip_connections >= self.per_ip_limit {
            return Err(RateLimitError::Limit {
                reason: format!("Per-IP connection limit reached for {}", addr),
            });
        }

        // Now try to get the local semaphore permit
        let permit = match self.semaphore.clone().try_acquire_owned() {
            Ok(permit) => permit,
            Err(_) => {
                return Err(RateLimitError::Limit {
                    reason: "Global connection limit reached on this instance".to_string(),
                });
            }
        };

        // Increment IP counter
        let ip_connections: usize = match conn.incr(self.ip_key(&addr), 1) {
            Ok(count) => count,
            Err(e) => {
                error!(
                    message = "Failed to increment IP counter in Redis",
                    error = e.to_string()
                );
                return Err(RateLimitError::Limit {
                    reason: "Redis operation failed".to_string(),
                });
            }
        };

        // Increment global counter
        let global_connections: usize = match conn.incr(self.global_key(), 1) {
            Ok(count) => {
                debug!(
                    message = "Incremented global counter",
                    global_connections = count,
                    global_limit = self.global_limit
                );
                count
            }
            Err(e) => {
                // Roll back IP counter increment
                let _: Result<(), _> = conn.decr(self.ip_key(&addr), 1);

                error!(
                    message = "Failed to increment global counter in Redis",
                    error = e.to_string()
                );
                return Err(RateLimitError::Limit {
                    reason: "Redis operation failed".to_string(),
                });
            }
        };

        debug!(
            message = "Connection established",
            ip = addr.to_string(),
            ip_connections = ip_connections,
            global_connections = global_connections
        );

        Ok(Ticket {
            addr,
            _permit: permit,
            rate_limiter: self,
        })
    }

    fn release(&self, addr: IpAddr) {
        match self.redis_client.get_connection() {
            Ok(mut conn) => {
                // Decrement IP counter
                let ip_connections: Result<usize, RedisError> = conn.decr(self.ip_key(&addr), 1);

                if let Err(ref e) = ip_connections {
                    error!(
                        message = "Failed to decrement IP counter in Redis",
                        error = e.to_string()
                    );
                }

                // Decrement global counter
                let global_connections: Result<usize, RedisError> = conn.decr(self.global_key(), 1);

                if let Ok(count) = global_connections {
                    debug!(
                        message = "Decremented global counter on release",
                        global_connections = count,
                        global_limit = self.global_limit
                    );
                } else if let Err(ref e) = global_connections {
                    error!(
                        message = "Failed to decrement global counter in Redis",
                        error = e.to_string()
                    );
                }

                debug!(
                    message = "Connection released",
                    ip = addr.to_string(),
                    ip_connections = ip_connections.unwrap_or(0),
                    global_connections = global_connections.unwrap_or(0)
                );
            }
            Err(e) => {
                error!(
                    message = "Failed to connect to Redis for release",
                    error = e.to_string()
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    const GLOBAL_LIMIT: usize = 3;
    const PER_IP_LIMIT: usize = 2;

    #[tokio::test]
    async fn test_tickets_are_released() {
        let user_1 = IpAddr::from_str("127.0.0.1").unwrap();

        let rate_limiter = Arc::new(InMemoryRateLimit::new(GLOBAL_LIMIT, PER_IP_LIMIT));

        assert_eq!(
            rate_limiter
                .inner
                .lock()
                .unwrap()
                .semaphore
                .available_permits(),
            GLOBAL_LIMIT
        );
        assert_eq!(
            rate_limiter.inner.lock().unwrap().active_connections.len(),
            0
        );

        let c1 = rate_limiter.clone().try_acquire(user_1).unwrap();

        assert_eq!(
            rate_limiter
                .inner
                .lock()
                .unwrap()
                .semaphore
                .available_permits(),
            GLOBAL_LIMIT - 1
        );
        assert_eq!(
            rate_limiter.inner.lock().unwrap().active_connections.len(),
            1
        );
        assert_eq!(
            rate_limiter.inner.lock().unwrap().active_connections[&user_1],
            1
        );

        drop(c1);

        assert_eq!(
            rate_limiter
                .inner
                .lock()
                .unwrap()
                .semaphore
                .available_permits(),
            GLOBAL_LIMIT
        );
        assert_eq!(
            rate_limiter.inner.lock().unwrap().active_connections.len(),
            0
        );
    }

    #[tokio::test]
    async fn test_global_rate_limits() {
        let user_1 = IpAddr::from_str("127.0.0.1").unwrap();
        let user_2 = IpAddr::from_str("128.0.0.1").unwrap();

        let rate_limiter = Arc::new(InMemoryRateLimit::new(GLOBAL_LIMIT, PER_IP_LIMIT));

        let _c1 = rate_limiter.clone().try_acquire(user_1).unwrap();

        let _c2 = rate_limiter.clone().try_acquire(user_2).unwrap();

        let _c3 = rate_limiter.clone().try_acquire(user_1).unwrap();

        assert_eq!(
            rate_limiter
                .inner
                .lock()
                .unwrap()
                .semaphore
                .available_permits(),
            0
        );

        let c4 = rate_limiter.clone().try_acquire(user_2);
        assert!(c4.is_err());
        assert_eq!(
            c4.err().unwrap().to_string(),
            "Rate Limit Reached: Global limit"
        );

        drop(_c3);

        let c4 = rate_limiter.clone().try_acquire(user_2);
        assert!(c4.is_ok());
    }

    #[tokio::test]
    async fn test_per_ip_limits() {
        let user_1 = IpAddr::from_str("127.0.0.1").unwrap();
        let user_2 = IpAddr::from_str("127.0.0.2").unwrap();

        let rate_limiter = Arc::new(InMemoryRateLimit::new(GLOBAL_LIMIT, PER_IP_LIMIT));

        let _c1 = rate_limiter.clone().try_acquire(user_1).unwrap();
        let _c2 = rate_limiter.clone().try_acquire(user_1).unwrap();

        assert_eq!(
            rate_limiter.inner.lock().unwrap().active_connections[&user_1],
            2
        );

        let c3 = rate_limiter.clone().try_acquire(user_1);
        assert!(c3.is_err());
        assert_eq!(
            c3.err().unwrap().to_string(),
            "Rate Limit Reached: IP limit exceeded"
        );

        // While the first IP is limited, the second isn't
        let c4 = rate_limiter.clone().try_acquire(user_2);
        assert!(c4.is_ok());
    }

    #[tokio::test]
    #[cfg(all(feature = "integration", test))]
    async fn test_redis_rate_limits_with_mock() {
        use redis_test::server::RedisServer;

        // Start a mock Redis server
        let server = RedisServer::new();
        let client_addr = format!("redis://{}", server.client_addr());
        let client = redis::Client::open(client_addr.as_str()).unwrap();

        // Wait for the server to start
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        let rate_limiter = Arc::new(RedisRateLimit::new(&client_addr, 5, 1, "test").unwrap());

        // Test IP addresses
        let user_1 = IpAddr::from_str("127.0.0.1").unwrap();
        let user_2 = IpAddr::from_str("127.0.0.2").unwrap();

        // Test 1: Acquire a connection successfully
        let ticket1 = rate_limiter.clone().try_acquire(user_1).unwrap();

        // Verify Redis state manually
        {
            let mut conn = client.get_connection().unwrap();
            let global_count: usize = redis::cmd("GET")
                .arg("test:global:connections")
                .query(&mut conn)
                .unwrap();
            let ip_count: usize = redis::cmd("GET")
                .arg("test:ip:127.0.0.1:connections")
                .query(&mut conn)
                .unwrap();

            assert_eq!(global_count, 1);
            assert_eq!(ip_count, 1);
        }

        // Test 3: Third connection for same IP should fail (exceeds per-IP limit)
        let result = rate_limiter.clone().try_acquire(user_1);
        assert!(result.is_err());
        if let Err(RateLimitError::Limit { reason }) = result {
            assert!(reason.contains("Per-IP connection limit"));
        } else {
            panic!("Expected a RateLimitError::Limit");
        }

        // Test 4: Different IP should work
        let ticket2 = rate_limiter.clone().try_acquire(user_2).unwrap();

        // Verify counts after multiple operations
        {
            let mut conn = client.get_connection().unwrap();
            let global_count: usize = redis::cmd("GET")
                .arg("test:global:connections")
                .query(&mut conn)
                .unwrap();
            let ip1_count: usize = redis::cmd("GET")
                .arg("test:ip:127.0.0.1:connections")
                .query(&mut conn)
                .unwrap();
            let ip2_count: usize = redis::cmd("GET")
                .arg("test:ip:127.0.0.2:connections")
                .query(&mut conn)
                .unwrap();

            assert_eq!(global_count, 2);
            assert_eq!(ip1_count, 1);
            assert_eq!(ip2_count, 1);
        }

        // Test 5: Test release by dropping tickets
        drop(ticket1);

        // Verify that counters were decremented
        {
            let mut conn = client.get_connection().unwrap();
            let global_count: usize = redis::cmd("GET")
                .arg("test:global:connections")
                .query(&mut conn)
                .unwrap();
            let ip1_count: usize = redis::cmd("GET")
                .arg("test:ip:127.0.0.1:connections")
                .query(&mut conn)
                .unwrap();

            assert_eq!(global_count, 1);
            assert_eq!(ip1_count, 0);
        }

        // Test 6: Now we should be able to acquire another connection for user_1
        let _ = rate_limiter.clone().try_acquire(user_1).unwrap();

        // Test 7: Test global limit
        let rate_limiter_small = Arc::new(
            RedisRateLimit::new(
                &client_addr,
                3, // smaller global limit
                5, // large per-IP limit
                "test2",
            )
            .unwrap(),
        );

        // Acquire connections up to the global limit
        let _t1 = rate_limiter_small.clone().try_acquire(user_1).unwrap();
        let _t2 = rate_limiter_small.clone().try_acquire(user_1).unwrap();
        let _t3 = rate_limiter_small.clone().try_acquire(user_1).unwrap();

        // This should fail due to global limit
        let result = rate_limiter_small.clone().try_acquire(user_1);
        assert!(result.is_err());
        if let Err(RateLimitError::Limit { reason }) = result {
            assert!(reason.contains("Global connection limit"));
        } else {
            panic!("Expected a RateLimitError::Limit");
        }

        drop(ticket2);
    }
}
