//! Explicit operation state for asynchronous forge writes. Pending operations block
//! resubmission so double Enter or repeated clicks cannot duplicate a write.

use std::sync::atomic::{AtomicU64, Ordering};

static OP_COUNTER: AtomicU64 = AtomicU64::new(1);

/// Correlation id for one write. Generated on submit and echoed on completion so results
/// can never be applied to a stale overlay.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub struct OpId(pub u64);
impl OpId {
    pub fn next() -> Self {
        Self(OP_COUNTER.fetch_add(1, Ordering::Relaxed))
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum WriteState<T> {
    Idle,
    Pending,
    Success(T),
    Failed(String),
}
impl<T> WriteState<T> {
    pub fn is_pending(&self) -> bool {
        matches!(self, Self::Pending)
    }
    pub fn is_idle(&self) -> bool {
        matches!(self, Self::Idle)
    }
    pub fn label(&self) -> &str {
        match self {
            Self::Idle => "",
            Self::Pending => "in progress…",
            Self::Success(_) => "done",
            Self::Failed(_) => "failed",
        }
    }
    pub fn result(self) -> Option<Result<T, String>> {
        match self {
            Self::Idle | Self::Pending => None,
            Self::Success(value) => Some(Ok(value)),
            Self::Failed(error) => Some(Err(error)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn op_ids_are_unique() {
        let a = OpId::next();
        let b = OpId::next();
        assert_ne!(a, b);
    }

    #[test]
    fn pending_blocks_and_idle_does_not() {
        let state: WriteState<()> = WriteState::Idle;
        assert!(state.is_idle());
        assert!(!state.is_pending());
        let state: WriteState<()> = WriteState::Pending;
        assert!(state.is_pending());
    }
}
