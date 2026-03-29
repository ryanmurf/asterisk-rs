//! Port of asterisk/tests/test_stasis.c
//!
//! Tests Stasis message bus: topic creation, subscribe/unsubscribe,
//! publish/receive, cache operations, and multiple subscribers.

use asterisk_core::stasis::{self, StasisCache, StasisMessage, Topic};
use std::any::Any;

/// A test message type for stasis testing.
#[derive(Debug, Clone)]
struct TestMessage {
    text: String,
}

impl StasisMessage for TestMessage {
    fn message_type(&self) -> &str {
        "TestMessage"
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// Another message type for type-differentiation tests.
#[derive(Debug, Clone)]
struct OtherMessage {
    value: i32,
}

impl StasisMessage for OtherMessage {
    fn message_type(&self) -> &str {
        "OtherMessage"
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// Port of AST_TEST_DEFINE(message_type) from test_stasis.c.
///
/// Test basic topic creation with name.
#[test]
fn test_topic_creation() {
    let topic = Topic::with_name("TestTopic");
    assert_eq!(topic.name(), "TestTopic");
    assert_eq!(topic.subscriber_count(), 0);
}

/// Test topic creation with custom capacity.
#[test]
fn test_topic_creation_with_capacity() {
    let topic = Topic::new("BigTopic", 1024);
    assert_eq!(topic.name(), "BigTopic");
}

/// Port of subscribe/unsubscribe behavior from test_stasis.c.
///
/// Test that subscribing increases subscriber count and dropping decreases it.
#[test]
fn test_subscribe_unsubscribe() {
    let topic = Topic::with_name("SubTest");
    assert_eq!(topic.subscriber_count(), 0);

    let sub1 = topic.subscribe();
    assert_eq!(topic.subscriber_count(), 1);
    assert_eq!(sub1.topic_name(), "SubTest");

    let sub2 = topic.subscribe();
    assert_eq!(topic.subscriber_count(), 2);

    // Subscription IDs should be unique
    assert_ne!(sub1.id(), sub2.id());

    // Drop sub1
    drop(sub1);
    assert_eq!(topic.subscriber_count(), 1);

    // Drop sub2
    drop(sub2);
    assert_eq!(topic.subscriber_count(), 0);
}

/// Port of publish/receive from test_stasis.c.
///
/// Test that publishing a message delivers it to subscribers.
#[tokio::test]
async fn test_publish_and_receive() {
    let topic = Topic::with_name("PubSub");
    let mut sub = topic.subscribe();

    // Publish a message
    let msg = TestMessage {
        text: "Hello Stasis".to_string(),
    };
    stasis::publish(&topic, msg);

    // Subscriber should receive it
    let received = sub.try_recv();
    assert!(received.is_some());

    let received = received.unwrap();
    assert_eq!(received.message_type(), "TestMessage");

    // Downcast to our type
    let test_msg = received.as_any().downcast_ref::<TestMessage>().unwrap();
    assert_eq!(test_msg.text, "Hello Stasis");
}

/// Test that messages published before subscription are not received.
#[tokio::test]
async fn test_no_retroactive_delivery() {
    let topic = Topic::with_name("NoRetro");

    // Publish before subscribing
    stasis::publish(
        &topic,
        TestMessage {
            text: "Before".to_string(),
        },
    );

    // Subscribe after
    let mut sub = topic.subscribe();

    // Should not receive the earlier message
    let received = sub.try_recv();
    assert!(received.is_none());

    // But should receive new messages
    stasis::publish(
        &topic,
        TestMessage {
            text: "After".to_string(),
        },
    );
    let received = sub.try_recv();
    assert!(received.is_some());
}

/// Port of multiple subscribers test from test_stasis.c.
///
/// Test that multiple subscribers all receive the same message.
#[tokio::test]
async fn test_multiple_subscribers() {
    let topic = Topic::with_name("Multi");
    let mut sub1 = topic.subscribe();
    let mut sub2 = topic.subscribe();
    let mut sub3 = topic.subscribe();

    stasis::publish(
        &topic,
        TestMessage {
            text: "Broadcast".to_string(),
        },
    );

    // All three should receive it
    let r1 = sub1.try_recv();
    let r2 = sub2.try_recv();
    let r3 = sub3.try_recv();

    assert!(r1.is_some());
    assert!(r2.is_some());
    assert!(r3.is_some());

    // All should have the same message content
    for r in [r1, r2, r3] {
        let msg = r.unwrap();
        let test_msg = msg.as_any().downcast_ref::<TestMessage>().unwrap();
        assert_eq!(test_msg.text, "Broadcast");
    }
}

/// Port of cache tests from test_stasis.c.
///
/// Test cache insert, update, retrieve, and remove.
#[test]
fn test_cache_insert_and_retrieve() {
    let cache = StasisCache::<String>::new("test_cache");

    assert_eq!(cache.name(), "test_cache");
    assert!(cache.is_empty());
    assert_eq!(cache.len(), 0);

    // Insert
    let old = cache.update("key1", "value1".to_string());
    assert!(old.is_none()); // no previous value

    // Retrieve
    let val = cache.get("key1");
    assert!(val.is_some());
    assert_eq!(val.unwrap().as_str(), "value1");

    assert_eq!(cache.len(), 1);
    assert!(!cache.is_empty());
}

/// Test cache update (overwrite).
#[test]
fn test_cache_update() {
    let cache = StasisCache::<String>::new("update_cache");

    cache.update("key", "original".to_string());
    assert_eq!(cache.get("key").unwrap().as_str(), "original");

    // Update
    let old = cache.update("key", "updated".to_string());
    assert!(old.is_some());
    assert_eq!(old.unwrap().as_str(), "original");

    assert_eq!(cache.get("key").unwrap().as_str(), "updated");
    assert_eq!(cache.len(), 1); // still only one entry
}

/// Test cache remove.
#[test]
fn test_cache_remove() {
    let cache = StasisCache::<String>::new("remove_cache");

    cache.update("key", "value".to_string());
    assert_eq!(cache.len(), 1);

    let removed = cache.remove("key");
    assert!(removed.is_some());
    assert_eq!(removed.unwrap().as_str(), "value");

    assert!(cache.is_empty());
    assert!(cache.get("key").is_none());
}

/// Test cache remove non-existent key.
#[test]
fn test_cache_remove_nonexistent() {
    let cache = StasisCache::<String>::new("cache");
    let removed = cache.remove("nonexistent");
    assert!(removed.is_none());
}

/// Test cache dump (get all entries).
#[test]
fn test_cache_dump() {
    let cache = StasisCache::<String>::new("dump_cache");

    cache.update("a", "1".to_string());
    cache.update("b", "2".to_string());
    cache.update("c", "3".to_string());

    let entries = cache.dump();
    assert_eq!(entries.len(), 3);

    // Entries should contain all values (order may vary)
    let values: Vec<&str> = entries.iter().map(|e| e.as_str()).collect();
    assert!(values.contains(&"1"));
    assert!(values.contains(&"2"));
    assert!(values.contains(&"3"));
}

/// Test cache clear.
#[test]
fn test_cache_clear() {
    let cache = StasisCache::<String>::new("clear_cache");

    cache.update("a", "1".to_string());
    cache.update("b", "2".to_string());
    assert_eq!(cache.len(), 2);

    cache.clear();
    assert!(cache.is_empty());
    assert_eq!(cache.len(), 0);
}

/// Test multiple message types on same topic.
#[tokio::test]
async fn test_different_message_types_on_topic() {
    let topic = Topic::with_name("Mixed");
    let mut sub = topic.subscribe();

    // Publish different types
    stasis::publish(
        &topic,
        TestMessage {
            text: "text".to_string(),
        },
    );
    stasis::publish(&topic, OtherMessage { value: 42 });

    // Receive first
    let r1 = sub.try_recv().unwrap();
    assert_eq!(r1.message_type(), "TestMessage");
    let tm = r1.as_any().downcast_ref::<TestMessage>().unwrap();
    assert_eq!(tm.text, "text");

    // Receive second
    let r2 = sub.try_recv().unwrap();
    assert_eq!(r2.message_type(), "OtherMessage");
    let om = r2.as_any().downcast_ref::<OtherMessage>().unwrap();
    assert_eq!(om.value, 42);
}

/// Test publishing to topic with no subscribers (no error).
#[test]
fn test_publish_no_subscribers() {
    let topic = Topic::with_name("NoSubs");

    // Should not panic or error
    stasis::publish(
        &topic,
        TestMessage {
            text: "nobody listening".to_string(),
        },
    );
}
