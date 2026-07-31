//! Crate-private error context, invariant, and clock helpers.

use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::error::{Result, SearchError};

pub(crate) trait ResultContext<T> {
    fn context<C>(self, context: C) -> Result<T>
    where
        C: Into<String>;

    fn with_context<C, F>(self, context: F) -> Result<T>
    where
        C: Into<String>,
        F: FnOnce() -> C;
}

impl<T, E> ResultContext<T> for std::result::Result<T, E>
where
    E: Into<SearchError>,
{
    fn context<C>(self, context: C) -> Result<T>
    where
        C: Into<String>,
    {
        self.map_err(|source| source.into().context(context))
    }

    fn with_context<C, F>(self, context: F) -> Result<T>
    where
        C: Into<String>,
        F: FnOnce() -> C,
    {
        self.map_err(|source| source.into().context(context()))
    }
}

macro_rules! ensure_valid_index {
    ($condition:expr, $($message:tt)+) => {
        if !$condition {
            return Err(SearchError::corrupt(format!($($message)+)));
        }
    };
}

pub(crate) use ensure_valid_index;
pub(crate) fn mtime_secs(metadata: &fs::Metadata) -> u64 {
    metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}

pub(crate) fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}
