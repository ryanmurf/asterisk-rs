//! Stasis Message Bus -- a loosely-typed pub/sub message distribution system.
//!
//! Modeled after Asterisk's stasis.h. Topics can be subscribed to, messages
//! published, and caches maintained for snapshot queries.

use dashmap::DashMap;
use std::any::Any;
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::broadcast;

/// A Stasis message that can be published on a topic.
///
/// All stasis messages must be Send + Sync + Any so they can be dispatched
/// across threads and downcast by subscribers.
pub trait StasisMessage: Send + Sync + Any + fmt::Debug {
    /// The message type name (for debugging and filtering).
    fn message_type(&self) -> &str;

    /// Upcast to Any for downcasting by subscribers.
    fn as_any(&self) -> &dyn Any;
}

/// A boxed stasis message suitable for broadcast.
pub type BoxedMessage = Arc<dyn StasisMessage>;

/// A topic that messages can be published to and subscribed from.
///
/// Uses a tokio broadcast channel internally for fan-out delivery.
pub struct Topic {
    name: String,
    sender: broadcast::Sender<BoxedMessage>,
}

impl Topic {
    /// Create a new topic with the given name and buffer capacity.
    pub fn new(name: impl Into<String>, capacity: usize) -> Self {
        let (sender, _) = broadcast::channel(capacity);
        Topic {
            name: name.into(),
            sender,
        }
    }

    /// Create a topic with a default capacity of 256 messages.
    pub fn with_name(name: impl Into<String>) -> Self {
        Self::new(name, 256)
    }

    /// Get the topic name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Publish a message to this topic. All current subscribers will receive it.
    pub fn publish(&self, message: BoxedMessage) {
        // Ignore send errors (no subscribers)
        let _ = self.sender.send(message);
    }

    /// Subscribe to this topic. Returns a Subscription that will receive
    /// all messages published after this point.
    pub fn subscribe(&self) -> Subscription {
        static NEXT_ID: AtomicU64 = AtomicU64::new(1);
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        Subscription {
            id,
            topic_name: self.name.clone(),
            receiver: self.sender.subscribe(),
        }
    }

    /// Get the current number of active subscribers.
    pub fn subscriber_count(&self) -> usize {
        self.sender.receiver_count()
    }
}

impl fmt::Debug for Topic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Topic")
            .field("name", &self.name)
            .field("subscribers", &self.sender.receiver_count())
            .finish()
    }
}

/// A subscription to a stasis topic.
///
/// Drop the subscription to unsubscribe.
pub struct Subscription {
    id: u64,
    topic_name: String,
    receiver: broadcast::Receiver<BoxedMessage>,
}

impl Subscription {
    /// Get the unique subscription ID.
    pub fn id(&self) -> u64 {
        self.id
    }

    /// Get the name of the topic this subscription is for.
    pub fn topic_name(&self) -> &str {
        &self.topic_name
    }

    /// Receive the next message. Returns None if the topic is closed.
    pub async fn recv(&mut self) -> Option<BoxedMessage> {
        loop {
            match self.receiver.recv().await {
                Ok(msg) => return Some(msg),
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    tracing::warn!(
                        subscription_id = self.id,
                        topic = %self.topic_name,
                        lagged = n,
                        "stasis subscription lagged, skipped messages"
                    );
                    // Continue to receive the next available message
                    continue;
                }
                Err(broadcast::error::RecvError::Closed) => return None,
            }
        }
    }

    /// Try to receive a message without blocking.
    pub fn try_recv(&mut self) -> Option<BoxedMessage> {
        loop {
            match self.receiver.try_recv() {
                Ok(msg) => return Some(msg),
                Err(broadcast::error::TryRecvError::Lagged(_)) => continue,
                Err(_) => return None,
            }
        }
    }
}

impl fmt::Debug for Subscription {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Subscription")
            .field("id", &self.id)
            .field("topic_name", &self.topic_name)
            .finish()
    }
}

/// A thread-safe cache for stasis messages, keyed by an arbitrary string key.
///
/// This is used to cache the latest snapshot of an entity (e.g. channel snapshot,
/// bridge snapshot) so it can be queried without locking the original object.
pub struct StasisCache<V: Send + Sync + 'static> {
    name: String,
    entries: DashMap<String, Arc<V>>,
}

impl<V: Send + Sync + 'static> StasisCache<V> {
    /// Create a new cache with the given name.
    pub fn new(name: impl Into<String>) -> Self {
        StasisCache {
            name: name.into(),
            entries: DashMap::new(),
        }
    }

    /// Get the cache name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Insert or update a cache entry.
    pub fn update(&self, key: impl Into<String>, value: V) -> Option<Arc<V>> {
        self.entries.insert(key.into(), Arc::new(value))
    }

    /// Get a cached entry by key.
    pub fn get(&self, key: &str) -> Option<Arc<V>> {
        self.entries.get(key).map(|entry| Arc::clone(entry.value()))
    }

    /// Remove a cached entry.
    pub fn remove(&self, key: &str) -> Option<Arc<V>> {
        self.entries.remove(key).map(|(_, v)| v)
    }

    /// Get all cached entries.
    pub fn dump(&self) -> Vec<Arc<V>> {
        self.entries
            .iter()
            .map(|entry| Arc::clone(entry.value()))
            .collect()
    }

    /// Get the number of entries in the cache.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Returns true if the cache is empty.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Clear all entries from the cache.
    pub fn clear(&self) {
        self.entries.clear();
    }
}

impl<V: Send + Sync + fmt::Debug + 'static> fmt::Debug for StasisCache<V> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StasisCache")
            .field("name", &self.name)
            .field("entries", &self.entries.len())
            .finish()
    }
}

/// Convenience function to publish a message to a topic.
pub fn publish(topic: &Topic, message: impl StasisMessage + 'static) {
    topic.publish(Arc::new(message));
}

/// Convenience function to subscribe to a topic.
pub fn subscribe(topic: &Topic) -> Subscription {
    topic.subscribe()
}
