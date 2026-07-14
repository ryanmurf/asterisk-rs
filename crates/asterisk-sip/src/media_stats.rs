//! Per-call RTP observability shared by the SIP driver and AMI.

use crate::rtp::{RtpStats, RtpStatsSnapshot};
use parking_lot::Mutex;
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, LazyLock};

/// Completed calls retained for post-call proof queries.
const COMPLETED_HISTORY_LIMIT: usize = 256;

/// An RTP statistics snapshot associated with one SIP channel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallMediaStats {
    pub channel: String,
    pub unique_id: Option<String>,
    pub active: bool,
    pub rtp: RtpStatsSnapshot,
}

struct MediaStatsRecord {
    unique_id: Option<String>,
    stats: Arc<RtpStats>,
    active: bool,
}

#[derive(Default)]
struct MediaStatsRegistry {
    by_channel: HashMap<String, MediaStatsRecord>,
    channel_by_unique_id: HashMap<String, String>,
    completed: VecDeque<String>,
}

static MEDIA_STATS: LazyLock<Mutex<MediaStatsRegistry>> =
    LazyLock::new(|| Mutex::new(MediaStatsRegistry::default()));

/// Associate a live RTP counter set with a channel. Re-registering refreshes
/// the Uniqueid after the core channel store assigns its final identity.
pub(crate) fn register_channel_media_stats(
    channel: &str,
    unique_id: Option<&str>,
    stats: Arc<RtpStats>,
) {
    let mut registry = MEDIA_STATS.lock();
    if let Some(previous) = registry.by_channel.remove(channel) {
        if let Some(previous_id) = previous.unique_id {
            registry.channel_by_unique_id.remove(&previous_id);
        }
    }

    let unique_id = unique_id
        .filter(|unique_id| !unique_id.is_empty())
        .map(str::to_string);
    if let Some(unique_id) = &unique_id {
        registry
            .channel_by_unique_id
            .insert(unique_id.clone(), channel.to_string());
    }
    registry.by_channel.insert(
        channel.to_string(),
        MediaStatsRecord {
            unique_id,
            stats,
            active: true,
        },
    );
}

/// Mark a channel complete while retaining its final atomics for a bounded
/// post-call history. This lets test automation query proof after hangup.
pub(crate) fn complete_channel_media_stats(channel: &str) {
    let mut registry = MEDIA_STATS.lock();
    let Some(record) = registry.by_channel.get_mut(channel) else {
        return;
    };
    if !record.active {
        return;
    }
    record.active = false;
    registry.completed.push_back(channel.to_string());

    while registry.completed.len() > COMPLETED_HISTORY_LIMIT {
        let Some(expired_channel) = registry.completed.pop_front() else {
            break;
        };
        let Some(expired_id) = registry
            .by_channel
            .get(&expired_channel)
            .filter(|record| !record.active)
            .map(|record| record.unique_id.clone())
        else {
            continue;
        };
        registry.by_channel.remove(&expired_channel);
        if let Some(expired_id) = expired_id {
            registry.channel_by_unique_id.remove(&expired_id);
        }
    }
}

/// Look up an active or recently completed call by channel name or Uniqueid.
pub fn lookup_call_media_stats(channel_or_unique_id: &str) -> Option<CallMediaStats> {
    let registry = MEDIA_STATS.lock();
    let channel = if registry.by_channel.contains_key(channel_or_unique_id) {
        channel_or_unique_id
    } else {
        registry
            .channel_by_unique_id
            .get(channel_or_unique_id)?
            .as_str()
    };
    let record = registry.by_channel.get(channel)?;
    Some(CallMediaStats {
        channel: channel.to_string(),
        unique_id: record.unique_id.clone(),
        active: record.active,
        rtp: record.stats.snapshot(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEST_ID: AtomicU64 = AtomicU64::new(1);

    #[test]
    fn completed_call_remains_queryable_by_channel_and_unique_id() {
        let id = TEST_ID.fetch_add(1, Ordering::Relaxed);
        let channel = format!("PJSIP/media-stats-{id}");
        let unique_id = format!("test.{id}");
        let stats = Arc::new(RtpStats::default());
        stats.voice_frames_sent.store(7, Ordering::Relaxed);

        register_channel_media_stats(&channel, Some(&unique_id), stats);
        assert!(lookup_call_media_stats(&channel).unwrap().active);

        complete_channel_media_stats(&channel);
        let completed = lookup_call_media_stats(&unique_id).unwrap();
        assert!(!completed.active);
        assert_eq!(completed.channel, channel);
        assert_eq!(completed.rtp.voice_frames_sent, 7);
    }
}
