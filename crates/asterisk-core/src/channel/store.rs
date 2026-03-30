//! Global channel container -- the single registry of all active channels.
//!
//! Mirrors C Asterisk's `channels` ao2 container accessed via
//! `ast_channel_callback`, `ast_channel_get_by_name`, etc.
//! Channels are registered at allocation time and automatically removed when
//! the `Channel` is hung up via `deregister`.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, LazyLock};
use std::time::{SystemTime, UNIX_EPOCH};

use dashmap::DashMap;
use parking_lot::Mutex;

use super::Channel;

/// Global channel store -- singleton.
static CHANNEL_STORE: LazyLock<ChannelStore> = LazyLock::new(ChannelStore::new);

/// Monotonically-increasing counter for the numeric suffix of unique IDs.
/// Combined with epoch seconds this mirrors C Asterisk's
/// `epoch.counter` unique-ID format.
static UNIQUE_COUNTER: AtomicU64 = AtomicU64::new(1);

/// Thread-safe container for all active channels.
///
/// Channels are indexed by both `name` and `unique_id` for O(1) lookup.
pub struct ChannelStore {
    /// Primary index: unique_id -> channel
    by_uniqueid: DashMap<String, Arc<Mutex<Channel>>>,
    /// Secondary index: name -> unique_id (so we can look up by name and then
    /// dereference into the primary map).
    name_to_uniqueid: DashMap<String, String>,
}

impl ChannelStore {
    fn new() -> Self {
        Self {
            by_uniqueid: DashMap::new(),
            name_to_uniqueid: DashMap::new(),
        }
    }

    /// Register a channel in the store.  Panics on duplicate unique_id.
    fn register(&self, channel: Arc<Mutex<Channel>>) {
        let guard = channel.lock();
        let uid = guard.unique_id.0.clone();
        let name = guard.name.clone();
        drop(guard);

        if self.by_uniqueid.contains_key(&uid) {
            tracing::error!(unique_id = %uid, "attempt to register duplicate channel unique_id");
            return;
        }
        self.name_to_uniqueid.insert(name, uid.clone());
        self.by_uniqueid.insert(uid, channel);
    }

    /// Remove a channel from the store by its unique_id.
    fn deregister(&self, unique_id: &str) {
        if let Some((_, chan)) = self.by_uniqueid.remove(unique_id) {
            let guard = chan.lock();
            self.name_to_uniqueid.remove(&guard.name);
        }
    }

    fn find_by_uniqueid(&self, uid: &str) -> Option<Arc<Mutex<Channel>>> {
        self.by_uniqueid.get(uid).map(|r| Arc::clone(r.value()))
    }

    fn find_by_name(&self, name: &str) -> Option<Arc<Mutex<Channel>>> {
        let uid = self.name_to_uniqueid.get(name)?;
        self.by_uniqueid.get(uid.value()).map(|r| Arc::clone(r.value()))
    }

    fn count(&self) -> usize {
        self.by_uniqueid.len()
    }
}

// ---------------------------------------------------------------------------
// Public free-function API
// ---------------------------------------------------------------------------

/// Generate a unique-ID string in `epoch.counter` format, exactly like C
/// Asterisk's `ast_channel_uniqueid`.
pub fn generate_uniqueid() -> String {
    let epoch = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let seq = UNIQUE_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{}.{}", epoch, seq)
}

/// Allocate a new channel, assign a unique ID, and register it in the global
/// store.  Returns an `Arc<Mutex<Channel>>` that is already tracked.
pub fn alloc_channel(name: impl Into<String>) -> Arc<Mutex<Channel>> {
    let name = name.into();
    let uid = generate_uniqueid();
    let mut channel = Channel::new(&name);
    channel.unique_id = super::ChannelId(uid.clone());
    channel.linkedid = uid.clone();
    let arc = Arc::new(Mutex::new(channel));
    CHANNEL_STORE.register(Arc::clone(&arc));
    tracing::debug!(channel = %name, "channel allocated");

    // Emit Newchannel AMI event via the channel event publisher
    super::publish_channel_event("Newchannel", &[
        ("Channel", &name),
        ("ChannelState", "0"),
        ("ChannelStateDesc", "Down"),
        ("CallerIDNum", ""),
        ("Uniqueid", &uid),
        ("Linkedid", &uid),
    ]);

    arc
}

/// Register an existing `Channel` in the global store.
///
/// This is used when a channel was created by a channel driver (e.g.
/// `LocalChannelDriver::request_pair()`) and needs to be registered in the
/// global store.  Assigns a unique ID if the channel still has its default
/// UUID-based ID, then emits a `Newchannel` AMI event.
///
/// Returns the `Arc<Mutex<Channel>>` that is now tracked in the store.
pub fn register_existing_channel(mut channel: Channel) -> Arc<Mutex<Channel>> {
    // Assign a proper epoch.counter unique ID (overwrite the UUID default)
    let uid = generate_uniqueid();
    let name = channel.name.clone();
    channel.unique_id = super::ChannelId(uid.clone());
    channel.linkedid = uid.clone();

    let state_num = (channel.state as u8).to_string();
    let state_desc = channel.state.to_string();
    let caller_num = channel.caller.id.number.number.clone();

    let arc = Arc::new(Mutex::new(channel));
    CHANNEL_STORE.register(Arc::clone(&arc));
    tracing::debug!(channel = %name, unique_id = %uid, "existing channel registered");

    // Emit Newchannel AMI event
    super::publish_channel_event("Newchannel", &[
        ("Channel", &name),
        ("ChannelState", &state_num),
        ("ChannelStateDesc", &state_desc),
        ("CallerIDNum", &caller_num),
        ("Uniqueid", &uid),
        ("Linkedid", &uid),
    ]);

    arc
}

/// Look up a channel by its channel name (e.g. `SIP/alice-00000001`).
pub fn find_by_name(name: &str) -> Option<Arc<Mutex<Channel>>> {
    CHANNEL_STORE.find_by_name(name)
}

/// Look up a channel by its unique ID.
pub fn find_by_uniqueid(uid: &str) -> Option<Arc<Mutex<Channel>>> {
    CHANNEL_STORE.find_by_uniqueid(uid)
}

/// Find channels currently executing at the given dialplan location.
///
/// NOTE: We first collect all Arc references from the DashMap, then
/// release the DashMap shard locks, and only then lock each Channel
/// individually. This prevents a potential deadlock where we hold a
/// DashMap shard lock while trying to acquire a Channel mutex (while
/// another thread might hold the Channel mutex and need the DashMap).
pub fn find_by_exten(context: &str, exten: &str) -> Vec<Arc<Mutex<Channel>>> {
    // Phase 1: collect all channel Arcs without locking any Channel.
    let all_channels: Vec<Arc<Mutex<Channel>>> = CHANNEL_STORE
        .by_uniqueid
        .iter()
        .map(|entry| Arc::clone(entry.value()))
        .collect();

    // Phase 2: now lock each channel individually (DashMap shard locks released).
    let mut results = Vec::new();
    for chan_arc in all_channels {
        let chan = chan_arc.lock();
        if chan.context == context && chan.exten == exten {
            results.push(Arc::clone(&chan_arc));
        }
    }
    results
}

/// Iterate over all active channels, calling `f` for each one.
/// The callback receives an `Arc<Mutex<Channel>>`.
///
/// NOTE: The DashMap iteration is completed first (collecting Arcs),
/// then the callback is invoked. This prevents holding DashMap shard
/// locks while the callback potentially acquires other locks.
pub fn for_each<F>(f: F)
where
    F: Fn(&Arc<Mutex<Channel>>),
{
    let channels: Vec<Arc<Mutex<Channel>>> = CHANNEL_STORE
        .by_uniqueid
        .iter()
        .map(|entry| Arc::clone(entry.value()))
        .collect();
    for chan in &channels {
        f(chan);
    }
}

/// Collect snapshots of every active channel -- useful for `core show channels`.
pub fn all_channels() -> Vec<Arc<Mutex<Channel>>> {
    CHANNEL_STORE
        .by_uniqueid
        .iter()
        .map(|entry| Arc::clone(entry.value()))
        .collect()
}

/// Return the number of currently active channels.
pub fn count() -> usize {
    CHANNEL_STORE.count()
}

/// Remove a channel from the global store.  Called during hangup.
pub fn deregister(unique_id: &str) {
    CHANNEL_STORE.deregister(unique_id);
}

/// Update the name index when a channel's name changes (e.g. masquerade).
pub fn update_name(old_name: &str, new_name: &str, unique_id: &str) {
    CHANNEL_STORE.name_to_uniqueid.remove(old_name);
    CHANNEL_STORE
        .name_to_uniqueid
        .insert(new_name.to_string(), unique_id.to_string());
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: create a channel with a unique name for testing.
    fn test_alloc(suffix: &str) -> (Arc<Mutex<Channel>>, String) {
        let name = format!("Test/test-{}-{}", suffix, UNIQUE_COUNTER.load(Ordering::Relaxed));
        let arc = alloc_channel(&name);
        let uid = arc.lock().unique_id.0.clone();
        (arc, uid)
    }

    #[test]
    fn alloc_and_find_by_name() {
        let (arc, _uid) = test_alloc("name");
        let name = arc.lock().name.clone();
        let found = find_by_name(&name);
        assert!(found.is_some(), "should find by name");
        let found = found.unwrap();
        assert_eq!(found.lock().name, name);
    }

    #[test]
    fn alloc_and_find_by_uniqueid() {
        let (_arc, uid) = test_alloc("uid");
        let found = find_by_uniqueid(&uid);
        assert!(found.is_some(), "should find by unique_id");
    }

    #[test]
    fn deregister_removes_channel() {
        let (arc, uid) = test_alloc("dereg");
        let name = arc.lock().name.clone();
        deregister(&uid);
        assert!(find_by_uniqueid(&uid).is_none());
        assert!(find_by_name(&name).is_none());
    }

    #[test]
    fn count_tracks_channels() {
        let before = count();
        let (_a, uid_a) = test_alloc("cnt_a");
        let (_b, uid_b) = test_alloc("cnt_b");
        // Count should have increased by at least 2
        let after_alloc = count();
        assert!(
            after_alloc >= before + 2,
            "count should increase by at least 2: was {}, now {}",
            before,
            after_alloc,
        );
        deregister(&uid_a);
        deregister(&uid_b);
        // Verify the channels we just deregistered are actually gone.
        // We cannot rely on exact count comparisons because parallel tests
        // may be concurrently allocating/deallocating channels in the global store.
        assert!(find_by_uniqueid(&uid_a).is_none(), "uid_a should be gone after deregister");
        assert!(find_by_uniqueid(&uid_b).is_none(), "uid_b should be gone after deregister");
    }

    #[test]
    fn find_by_exten_works() {
        let (arc, uid) = test_alloc("exten");
        {
            let mut ch = arc.lock();
            ch.context = "from-internal".to_string();
            ch.exten = "100".to_string();
        }
        let results = find_by_exten("from-internal", "100");
        assert!(!results.is_empty());
        deregister(&uid);
    }

    #[test]
    fn uniqueid_format() {
        let uid = generate_uniqueid();
        let parts: Vec<&str> = uid.split('.').collect();
        assert_eq!(parts.len(), 2, "unique_id should be epoch.counter");
        assert!(parts[0].parse::<u64>().is_ok());
        assert!(parts[1].parse::<u64>().is_ok());
    }
}
