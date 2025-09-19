use std::{error, fmt};
use tower::BoxError;

/// This error is substitution for tower Elapsed error, because it's pub(super) and cannot be used
#[derive(Debug, Default)]
pub struct Elapsed;

impl fmt::Display for Elapsed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.pad("request timed out")
    }
}

impl error::Error for Elapsed {}

#[derive(Debug)]
pub enum HyperError {
    Hyper(Box<hyper::Error>),
    Timeout,
}

impl fmt::Display for HyperError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Hyper(e) => e.fmt(f),
            Self::Timeout => Elapsed.fmt(f),
        }
    }
}

impl error::Error for HyperError {
    fn source(&self) -> Option<&(dyn error::Error + 'static)> {
        match self {
            Self::Hyper(e) => e.source(),
            Self::Timeout => Elapsed.source(),
        }
    }
}

// We have 2 possible errors, one is hyper::Error by all layers except for timeout and another is timeout error.
// If another layers with custom errors are added this function need to be extended
pub fn restore_error(error: BoxError) -> HyperError {
    error
        .downcast::<hyper::Error>()
        .map(HyperError::Hyper)
        .unwrap_or_else(|_| HyperError::Timeout)
}
