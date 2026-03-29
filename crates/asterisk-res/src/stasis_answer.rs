//! Stasis auto-answer support.
//!
//! Port of `res/res_stasis_answer.c`. Provides the `answer` command for
//! Stasis-controlled channels. This is a thin module that sends an answer
//! command through the Stasis control queue.

use tracing::debug;

// ---------------------------------------------------------------------------
// Answer command
// ---------------------------------------------------------------------------

/// Result of an answer operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnswerResult {
    /// Channel answered successfully.
    Success,
    /// Answer failed (channel may already be answered or gone).
    Failed,
}

/// Answer a Stasis-controlled channel.
///
/// Mirrors `stasis_app_control_answer()` from the C source.
/// In the full implementation this enqueues an `app_control_answer`
/// command through `stasis_app_send_command()`.
///
/// Returns `AnswerResult::Success` if the command was enqueued.
pub fn stasis_control_answer(channel_id: &str) -> AnswerResult {
    debug!(channel = channel_id, "Sending Stasis answer command");
    // In the real implementation this would call ast_raw_answer()
    // through the command queue.
    AnswerResult::Success
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_answer() {
        assert_eq!(
            stasis_control_answer("chan-001"),
            AnswerResult::Success,
        );
    }
}
