//! Cancellation for blocking media reads owned by a bridge lifetime.

use dashmap::mapref::entry::Entry;
use dashmap::DashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock};
use tokio::sync::Notify;

static CHANNEL_LIFETIMES: LazyLock<DashMap<String, BridgeLifetime>> =
    LazyLock::new(DashMap::new);

#[derive(Debug)]
struct BridgeLifetimeInner {
    cancelled: AtomicBool,
    notify: Notify,
}

/// A cloneable, race-safe cancellation signal shared by both media pumps.
#[derive(Debug, Clone)]
pub struct BridgeLifetime {
    inner: Arc<BridgeLifetimeInner>,
}

impl BridgeLifetime {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(BridgeLifetimeInner {
                cancelled: AtomicBool::new(false),
                notify: Notify::new(),
            }),
        }
    }

    /// Cancel once and wake every blocked media read selected against this lifetime.
    pub fn cancel(&self) {
        if !self.inner.cancelled.swap(true, Ordering::AcqRel) {
            self.inner.notify.notify_waiters();
        }
    }

    pub fn is_cancelled(&self) -> bool {
        self.inner.cancelled.load(Ordering::Acquire)
    }

    /// Wait until cancellation without losing a notification to a registration race.
    pub async fn cancelled(&self) {
        let notified = self.inner.notify.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        if self.is_cancelled() {
            return;
        }
        notified.await;
    }

    fn same_instance(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.inner, &other.inner)
    }

    /// Associate channel names with this lifetime. Dropping the returned
    /// registration removes only mappings that still point to this instance.
    pub fn register_channels<I, S>(&self, channel_names: I) -> BridgeLifetimeRegistration
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let channel_names: Vec<String> = channel_names.into_iter().map(Into::into).collect();
        for channel_name in &channel_names {
            CHANNEL_LIFETIMES.insert(channel_name.clone(), self.clone());
        }
        BridgeLifetimeRegistration {
            lifetime: self.clone(),
            channel_names,
        }
    }
}

impl Default for BridgeLifetime {
    fn default() -> Self {
        Self::new()
    }
}

/// Cancel the active bridge containing `channel_name`, if any.
pub fn cancel_for_channel(channel_name: &str) -> bool {
    let Some(lifetime) = CHANNEL_LIFETIMES.get(channel_name) else {
        return false;
    };
    lifetime.cancel();
    true
}

/// RAII cleanup for the global channel-to-bridge cancellation lookup.
#[derive(Debug)]
pub struct BridgeLifetimeRegistration {
    lifetime: BridgeLifetime,
    channel_names: Vec<String>,
}

impl Drop for BridgeLifetimeRegistration {
    fn drop(&mut self) {
        for channel_name in &self.channel_names {
            if let Entry::Occupied(entry) = CHANNEL_LIFETIMES.entry(channel_name.clone()) {
                if entry.get().same_instance(&self.lifetime) {
                    entry.remove();
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test]
    async fn channel_cancellation_wakes_every_waiter_and_registration_cleans_up() {
        let lifetime = BridgeLifetime::new();
        let registration = lifetime.register_channels(["PJSIP/a", "PJSIP/b"]);
        let first = lifetime.clone();
        let second = lifetime.clone();
        let first_waiter = tokio::spawn(async move { first.cancelled().await });
        let second_waiter = tokio::spawn(async move { second.cancelled().await });

        assert!(cancel_for_channel("PJSIP/a"));
        tokio::time::timeout(Duration::from_millis(100), first_waiter)
            .await
            .expect("first waiter leaked")
            .unwrap();
        tokio::time::timeout(Duration::from_millis(100), second_waiter)
            .await
            .expect("second waiter leaked")
            .unwrap();

        drop(registration);
        assert!(!cancel_for_channel("PJSIP/a"));
        assert!(!cancel_for_channel("PJSIP/b"));
    }

    #[test]
    fn old_registration_cannot_remove_replacement_lifetime() {
        let old = BridgeLifetime::new();
        let old_registration = old.register_channels(["PJSIP/reused"]);
        let replacement = BridgeLifetime::new();
        let replacement_registration = replacement.register_channels(["PJSIP/reused"]);

        drop(old_registration);
        assert!(cancel_for_channel("PJSIP/reused"));
        assert!(replacement.is_cancelled());

        drop(replacement_registration);
    }
}
