use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::{Arc, Mutex};
use tracing::{debug, warn};

use thiserror::Error;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

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
}
