//! Progress events + sink trait. App, CLI, and MCP all receive the same
//! feed (plan §3.1.7). Counters match notes §11.

use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Core phases mapped to themed status labels by the wrappers (plan §3.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    /// "Searching for driftwood"
    Enumerating,
    /// "Wading in" — metadata (Spotlight dates, sizes, orphans)
    Wading,
    /// "Following the current" — hard rules + orphan status
    Filtering,
    /// Scoring + banding
    Scoring,
    /// "Traveling to the river" — Stage 2 LLM
    Reasoning,
    /// "Sorting the driftwood" — report assembly
    Assembling,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ScanEvent {
    Phase {
        phase: Phase,
    },
    FilesSearched {
        total: u64,
    },
    BytesSearched {
        total: u64,
    },
    CandidatesFound {
        total: u64,
    },
    RecoverableBytes {
        total: u64,
    },
    /// Stage 2 progress. `judged`/`total` are real middle-band item counts
    /// (cluster propagation included); cost/token totals are actual spend
    /// so far, only finalized when a streamed call ends. This is the only
    /// phase with a knowable denominator — UI must not render a percentage
    /// for the walk/sizing phases from this feed.
    ReasoningProgress {
        judged: u64,
        total: u64,
        cost_usd: f64,
        prompt_tokens: u64,
        completion_tokens: u64,
    },
    /// A representative batch was dispatched (1-based index; `total_batches`
    /// is the deduplicated batch count — the ETA's true denominator).
    BatchStarted {
        index: u64,
        total_batches: u64,
    },
    /// A representative batch settled (success, failure, or abort).
    BatchFinished {
        index: u64,
        total_batches: u64,
    },
    /// Themed-detail feed for disclosure arrows (raw queries, LLM thinking).
    Notice {
        message: String,
    },
    /// Non-fatal problems (permission errors counted as "wading past").
    Warn {
        message: String,
    },
    Error {
        message: String,
    },
}

impl ScanEvent {
    pub fn notice(m: impl Into<String>) -> Self {
        ScanEvent::Notice {
            message: m.into(),
        }
    }
    pub fn warn(m: impl Into<String>) -> Self {
        ScanEvent::Warn {
            message: m.into(),
        }
    }
}

/// Sink for scan progress. Called from async context; implementations must
/// be cheap and non-blocking (channel send, atomic increment).
pub trait EventSink: Send + Sync {
    fn emit(&self, event: ScanEvent);
}

impl EventSink for Arc<dyn EventSink> {
    fn emit(&self, event: ScanEvent) {
        (**self).emit(event);
    }
}

impl<F: Fn(ScanEvent) + Send + Sync> EventSink for F {
    fn emit(&self, event: ScanEvent) {
        (self)(event);
    }
}

/// No-op sink.
pub struct NullSink;

impl EventSink for NullSink {
    fn emit(&self, _event: ScanEvent) {}
}

/// Collects events into a bounded channel consumer-friendly vec (tests).
#[derive(Default)]
pub struct CollectingSink {
    pub events: parking_lot_lite::Mutex<Vec<ScanEvent>>,
}

mod parking_lot_lite {
    /// Minimal mutex substitute to avoid an extra dependency in tests.
    pub struct Mutex<T>(std::sync::Mutex<T>);
    impl<T> Mutex<T> {
        pub fn new(v: T) -> Self {
            Self(std::sync::Mutex::new(v))
        }
        pub fn lock(&self) -> std::sync::MutexGuard<'_, T> {
            self.0.lock().unwrap_or_else(|p| p.into_inner())
        }
    }
    impl<T> Default for Mutex<T>
    where
        T: Default,
    {
        fn default() -> Self {
            Self::new(T::default())
        }
    }
}

impl CollectingSink {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }
    pub fn snapshot(&self) -> Vec<ScanEvent> {
        self.events.lock().clone()
    }
}

impl EventSink for CollectingSink {
    fn emit(&self, event: ScanEvent) {
        self.events.lock().push(event);
    }
}
