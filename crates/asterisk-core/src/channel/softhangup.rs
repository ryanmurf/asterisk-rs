//! Softhangup system -- deferred/cooperative hangup flags.
//!
//! Mirrors C Asterisk's `AST_SOFTHANGUP_*` flags, `ast_softhangup_nolock`,
//! `ast_check_hangup`, and `ast_channel_clear_softhangup` (channel.c/channel.h).
//!
//! A soft hangup tells the channel to hang up at the next convenient point
//! (typically on the next `ast_read()`), rather than tearing it down
//! immediately.  Different flag values indicate *why* the hangup was
//! requested, and the PBX engine uses this to decide what to do (e.g.
//! `AST_SOFTHANGUP_ASYNCGOTO` means "don't actually hang up, just break
//! out so we can redirect").

/// Soft hangup requested by device or other internal reason.
/// Actual hangup needed.
pub const AST_SOFTHANGUP_DEV: u32 = 1 << 0;

/// Used to break the normal frame flow so an async goto can be done instead
/// of actually hanging up.
pub const AST_SOFTHANGUP_ASYNCGOTO: u32 = 1 << 1;

/// Soft hangup requested by system shutdown.  Actual hangup needed.
pub const AST_SOFTHANGUP_SHUTDOWN: u32 = 1 << 2;

/// Used to break the normal frame flow after a timeout so an implicit async
/// goto can be done to the 'T' exten if it exists instead of actually
/// hanging up.
pub const AST_SOFTHANGUP_TIMEOUT: u32 = 1 << 3;

/// Soft hangup requested by application/channel-driver being unloaded.
/// Actual hangup needed.
pub const AST_SOFTHANGUP_APPUNLOAD: u32 = 1 << 4;

/// Soft hangup requested by non-associated party.  Actual hangup needed.
pub const AST_SOFTHANGUP_EXPLICIT: u32 = 1 << 5;

/// Used to indicate the channel is currently executing hangup logic in the
/// dialplan.  The channel has been hung up when this is set.
pub const AST_SOFTHANGUP_HANGUP_EXEC: u32 = 1 << 7;

/// All softhangup flags combined -- used with `clear_softhangup` to clear
/// everything.
pub const AST_SOFTHANGUP_ALL: u32 = 0xFFFFFFFF;

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::channel::Channel;

    #[test]
    fn softhangup_sets_flags() {
        let mut ch = Channel::new("Test/sh-set");
        assert!(!ch.check_hangup());

        ch.softhangup(AST_SOFTHANGUP_DEV);
        assert!(ch.check_hangup());
        assert!(ch.softhangup_flags & AST_SOFTHANGUP_DEV != 0);
    }

    #[test]
    fn softhangup_multiple_flags() {
        let mut ch = Channel::new("Test/sh-multi");
        ch.softhangup(AST_SOFTHANGUP_DEV);
        ch.softhangup(AST_SOFTHANGUP_SHUTDOWN);

        assert!(ch.softhangup_flags & AST_SOFTHANGUP_DEV != 0);
        assert!(ch.softhangup_flags & AST_SOFTHANGUP_SHUTDOWN != 0);
        assert!(ch.check_hangup());
    }

    #[test]
    fn clear_softhangup_single_flag() {
        let mut ch = Channel::new("Test/sh-clear");
        ch.softhangup(AST_SOFTHANGUP_DEV | AST_SOFTHANGUP_SHUTDOWN);
        assert!(ch.check_hangup());

        ch.clear_softhangup(AST_SOFTHANGUP_DEV);
        // SHUTDOWN still set
        assert!(ch.check_hangup());
        assert!(ch.softhangup_flags & AST_SOFTHANGUP_DEV == 0);
        assert!(ch.softhangup_flags & AST_SOFTHANGUP_SHUTDOWN != 0);
    }

    #[test]
    fn clear_softhangup_all() {
        let mut ch = Channel::new("Test/sh-clearall");
        ch.softhangup(AST_SOFTHANGUP_DEV | AST_SOFTHANGUP_TIMEOUT | AST_SOFTHANGUP_EXPLICIT);
        assert!(ch.check_hangup());

        ch.clear_softhangup(AST_SOFTHANGUP_ALL);
        assert!(!ch.check_hangup());
        assert_eq!(ch.softhangup_flags, 0);
    }

    #[test]
    fn asyncgoto_is_not_real_hangup_but_check_returns_true() {
        // In C Asterisk, AST_SOFTHANGUP_ASYNCGOTO causes check_hangup to
        // return true (it breaks the read loop), but the PBX engine treats
        // it specially and doesn't actually hang up.
        let mut ch = Channel::new("Test/sh-async");
        ch.softhangup(AST_SOFTHANGUP_ASYNCGOTO);
        assert!(ch.check_hangup());
    }
}
