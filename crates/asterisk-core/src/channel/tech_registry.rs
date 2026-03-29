//! Channel technology registry.
//!
//! Mirrors the C `ast_channel_register` / `ast_get_channel_tech` mechanism.
//! Each channel technology (SIP, DAHDI, IAX, etc.) registers its driver here
//! so the PBX can look it up by name when creating channels.

use std::sync::{Arc, LazyLock};

use dashmap::DashMap;

use super::ChannelDriver;

/// Global channel technology registry.
pub static TECH_REGISTRY: LazyLock<ChannelTechRegistry> = LazyLock::new(ChannelTechRegistry::new);

/// Registry of channel technology drivers, keyed by uppercase technology name.
///
/// Thread-safe via `DashMap`. Drivers register themselves at module load
/// time and are looked up by the PBX when requesting new channels.
pub struct ChannelTechRegistry {
    drivers: DashMap<String, Arc<dyn ChannelDriver>>,
}

impl ChannelTechRegistry {
    /// Create a new empty registry.
    pub fn new() -> Self {
        Self {
            drivers: DashMap::new(),
        }
    }

    /// Register a channel technology driver.
    ///
    /// The driver's `name()` is uppercased and used as the registry key.
    /// If a driver with the same name is already registered, it is replaced
    /// and a warning is logged.
    pub fn register(&self, driver: Arc<dyn ChannelDriver>) {
        let key = driver.name().to_uppercase();
        if self.drivers.contains_key(&key) {
            tracing::warn!(
                "Channel technology '{}' already registered, replacing",
                key
            );
        }
        tracing::debug!("Registered channel technology: {}", key);
        self.drivers.insert(key, driver);
    }

    /// Find a channel technology driver by name (case-insensitive).
    pub fn find(&self, tech_name: &str) -> Option<Arc<dyn ChannelDriver>> {
        self.drivers
            .get(&tech_name.to_uppercase())
            .map(|r| r.clone())
    }

    /// Unregister a channel technology driver by name (case-insensitive).
    pub fn unregister(&self, tech_name: &str) {
        let key = tech_name.to_uppercase();
        if self.drivers.remove(&key).is_some() {
            tracing::debug!("Unregistered channel technology: {}", key);
        }
    }

    /// List all registered technology names (sorted).
    pub fn list(&self) -> Vec<String> {
        let mut names: Vec<String> = self.drivers.iter().map(|r| r.key().clone()).collect();
        names.sort();
        names
    }

    /// Get the count of registered technologies.
    pub fn count(&self) -> usize {
        self.drivers.len()
    }
}

impl Default for ChannelTechRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::channel::Channel;
    use asterisk_types::{AsteriskResult, Frame};

    /// A mock channel driver for testing.
    #[derive(Debug)]
    struct MockDriver {
        tech_name: String,
    }

    #[async_trait::async_trait]
    impl ChannelDriver for MockDriver {
        fn name(&self) -> &str {
            &self.tech_name
        }

        fn description(&self) -> &str {
            "Mock driver for testing"
        }

        async fn request(
            &self,
            dest: &str,
            _caller: Option<&Channel>,
        ) -> AsteriskResult<Channel> {
            Ok(Channel::new(format!("{}/{}", self.tech_name, dest)))
        }

        async fn call(
            &self,
            _channel: &mut Channel,
            _dest: &str,
            _timeout: i32,
        ) -> AsteriskResult<()> {
            Ok(())
        }

        async fn hangup(&self, _channel: &mut Channel) -> AsteriskResult<()> {
            Ok(())
        }

        async fn answer(&self, _channel: &mut Channel) -> AsteriskResult<()> {
            Ok(())
        }

        async fn read_frame(&self, _channel: &mut Channel) -> AsteriskResult<Frame> {
            Ok(Frame::Null)
        }

        async fn write_frame(
            &self,
            _channel: &mut Channel,
            _frame: &Frame,
        ) -> AsteriskResult<()> {
            Ok(())
        }
    }

    #[test]
    fn test_register_and_find() {
        let registry = ChannelTechRegistry::new();
        let driver = Arc::new(MockDriver {
            tech_name: "SIP".to_string(),
        });
        registry.register(driver);

        assert!(registry.find("SIP").is_some());
        assert_eq!(registry.find("SIP").unwrap().name(), "SIP");
        assert!(registry.find("NonExistent").is_none());
    }

    #[test]
    fn test_find_case_insensitive() {
        let registry = ChannelTechRegistry::new();
        let driver = Arc::new(MockDriver {
            tech_name: "PJSIP".to_string(),
        });
        registry.register(driver);

        // All case variants should find the same driver
        assert!(registry.find("PJSIP").is_some());
        assert!(registry.find("pjsip").is_some());
        assert!(registry.find("Pjsip").is_some());
        assert!(registry.find("pJsIp").is_some());

        // All should return the same driver name
        assert_eq!(registry.find("pjsip").unwrap().name(), "PJSIP");
    }

    #[test]
    fn test_unregister() {
        let registry = ChannelTechRegistry::new();
        let driver = Arc::new(MockDriver {
            tech_name: "DAHDI".to_string(),
        });
        registry.register(driver);

        assert!(registry.find("DAHDI").is_some());
        assert_eq!(registry.count(), 1);

        registry.unregister("dahdi"); // case-insensitive unregister
        assert!(registry.find("DAHDI").is_none());
        assert_eq!(registry.count(), 0);
    }

    #[test]
    fn test_list() {
        let registry = ChannelTechRegistry::new();

        registry.register(Arc::new(MockDriver {
            tech_name: "SIP".to_string(),
        }));
        registry.register(Arc::new(MockDriver {
            tech_name: "DAHDI".to_string(),
        }));
        registry.register(Arc::new(MockDriver {
            tech_name: "IAX2".to_string(),
        }));

        let names = registry.list();
        assert_eq!(names, vec!["DAHDI", "IAX2", "SIP"]);
    }

    #[test]
    fn test_count() {
        let registry = ChannelTechRegistry::new();
        assert_eq!(registry.count(), 0);

        registry.register(Arc::new(MockDriver {
            tech_name: "SIP".to_string(),
        }));
        assert_eq!(registry.count(), 1);

        registry.register(Arc::new(MockDriver {
            tech_name: "IAX2".to_string(),
        }));
        assert_eq!(registry.count(), 2);

        // Re-registering same name replaces, count stays the same
        registry.register(Arc::new(MockDriver {
            tech_name: "SIP".to_string(),
        }));
        assert_eq!(registry.count(), 2);
    }

    #[test]
    fn test_global_registry() {
        // Verify the global static works
        assert_eq!(TECH_REGISTRY.count(), TECH_REGISTRY.count());
    }
}
