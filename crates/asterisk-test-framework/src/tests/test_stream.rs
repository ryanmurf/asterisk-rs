//! Port of asterisk/tests/test_stream.c
//!
//! Tests media stream and stream topology operations. Since the Rust
//! codebase does not yet have a dedicated `ast_stream` module, this file
//! defines local equivalents of stream and topology, and tests the core
//! operations from the C test suite:
//! - Stream creation with media type
//! - Stream type/state/name getters/setters
//! - Stream metadata
//! - Topology creation, append, set, delete, clone
//! - Stream position tracking
//! - Multi-stream topology operations

use asterisk_types::MediaType;
use std::collections::HashMap;

// ---------------------------------------------------------------------------
// Local stream types (port of ast_stream / ast_stream_topology)
// ---------------------------------------------------------------------------

/// Stream state, mirroring `ast_stream_state`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StreamState {
    Inactive,
    SendRecv,
    SendOnly,
    RecvOnly,
    Removed,
}

/// A media stream, mirroring `ast_stream`.
#[derive(Debug, Clone)]
struct Stream {
    name: String,
    media_type: MediaType,
    state: StreamState,
    position: usize,
    metadata: HashMap<String, String>,
}

impl Stream {
    fn new(name: &str, media_type: MediaType) -> Self {
        Self {
            name: name.to_string(),
            media_type,
            state: StreamState::Inactive,
            position: 0,
            metadata: HashMap::new(),
        }
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn media_type(&self) -> MediaType {
        self.media_type
    }

    fn state(&self) -> StreamState {
        self.state
    }

    fn set_state(&mut self, state: StreamState) {
        self.state = state;
    }

    fn set_type(&mut self, media_type: MediaType) {
        self.media_type = media_type;
    }

    fn position(&self) -> usize {
        self.position
    }

    fn set_metadata(&mut self, key: &str, value: Option<&str>) {
        match value {
            Some(v) => {
                self.metadata.insert(key.to_string(), v.to_string());
            }
            None => {
                self.metadata.remove(key);
            }
        }
    }

    fn get_metadata(&self, key: &str) -> Option<&str> {
        self.metadata.get(key).map(|s| s.as_str())
    }
}

/// A stream topology, mirroring `ast_stream_topology`.
#[derive(Debug, Clone)]
struct StreamTopology {
    streams: Vec<Stream>,
}

impl StreamTopology {
    fn new() -> Self {
        Self {
            streams: Vec::new(),
        }
    }

    fn count(&self) -> usize {
        self.streams.len()
    }

    fn append(&mut self, mut stream: Stream) -> usize {
        let pos = self.streams.len();
        stream.position = pos;
        self.streams.push(stream);
        pos
    }

    fn get(&self, index: usize) -> Option<&Stream> {
        self.streams.get(index)
    }

    fn set_stream(&mut self, index: usize, mut stream: Stream) {
        stream.position = index;
        if index < self.streams.len() {
            self.streams[index] = stream;
        } else {
            // Extend to fit.
            while self.streams.len() < index {
                let mut placeholder = Stream::new("", MediaType::Unknown);
                placeholder.position = self.streams.len();
                self.streams.push(placeholder);
            }
            self.streams.push(stream);
        }
    }

    fn del_stream(&mut self, index: usize) -> Result<(), ()> {
        if index >= self.streams.len() {
            return Err(());
        }
        self.streams.remove(index);
        // Update positions.
        for (i, s) in self.streams.iter_mut().enumerate() {
            s.position = i;
        }
        Ok(())
    }

    fn clone_topology(&self) -> Self {
        self.clone()
    }
}

// ---------------------------------------------------------------------------
// Stream creation tests
// ---------------------------------------------------------------------------

/// Port of AST_TEST_DEFINE(stream_create) from test_stream.c.
///
/// Test that creating a stream results in a stream with expected values.
#[test]
fn test_stream_create() {
    let stream = Stream::new("test", MediaType::Audio);

    assert_eq!(stream.state(), StreamState::Inactive);
    assert_eq!(stream.media_type(), MediaType::Audio);
    assert_eq!(stream.name(), "test");
}

/// Port of AST_TEST_DEFINE(stream_create_no_name) from test_stream.c.
///
/// Test creating a stream with an empty name.
#[test]
fn test_stream_create_no_name() {
    let stream = Stream::new("", MediaType::Audio);
    assert_eq!(stream.name(), "");
    assert_eq!(stream.media_type(), MediaType::Audio);
}

// ---------------------------------------------------------------------------
// Stream type setting
// ---------------------------------------------------------------------------

/// Port of AST_TEST_DEFINE(stream_set_type) from test_stream.c.
///
/// Test that changing the type of a stream works.
#[test]
fn test_stream_set_type() {
    let mut stream = Stream::new("test", MediaType::Audio);
    assert_eq!(stream.media_type(), MediaType::Audio);

    stream.set_type(MediaType::Video);
    assert_eq!(stream.media_type(), MediaType::Video);
}

// ---------------------------------------------------------------------------
// Stream state setting
// ---------------------------------------------------------------------------

/// Port of AST_TEST_DEFINE(stream_set_state) from test_stream.c.
///
/// Test that changing the state of a stream works.
#[test]
fn test_stream_set_state() {
    let mut stream = Stream::new("test", MediaType::Audio);
    assert_eq!(stream.state(), StreamState::Inactive);

    stream.set_state(StreamState::SendRecv);
    assert_eq!(stream.state(), StreamState::SendRecv);

    stream.set_state(StreamState::SendOnly);
    assert_eq!(stream.state(), StreamState::SendOnly);

    stream.set_state(StreamState::RecvOnly);
    assert_eq!(stream.state(), StreamState::RecvOnly);

    stream.set_state(StreamState::Removed);
    assert_eq!(stream.state(), StreamState::Removed);
}

// ---------------------------------------------------------------------------
// Stream metadata
// ---------------------------------------------------------------------------

/// Port of AST_TEST_DEFINE(stream_metadata) from test_stream.c.
///
/// Test metadata operations on a stream.
#[test]
fn test_stream_metadata() {
    let mut stream = Stream::new("test", MediaType::Audio);

    // Initially no metadata.
    assert!(stream.get_metadata("track_label").is_none());

    // Set metadata.
    let track_label = "my-track-label-uuid";
    stream.set_metadata("track_label", Some(track_label));
    assert_eq!(stream.get_metadata("track_label"), Some(track_label));

    // Remove metadata by setting to None.
    stream.set_metadata("track_label", None);
    assert!(stream.get_metadata("track_label").is_none());
}

/// Test multiple metadata keys.
#[test]
fn test_stream_multiple_metadata() {
    let mut stream = Stream::new("test", MediaType::Audio);
    stream.set_metadata("key1", Some("value1"));
    stream.set_metadata("key2", Some("value2"));
    stream.set_metadata("key3", Some("value3"));

    assert_eq!(stream.get_metadata("key1"), Some("value1"));
    assert_eq!(stream.get_metadata("key2"), Some("value2"));
    assert_eq!(stream.get_metadata("key3"), Some("value3"));
}

// ---------------------------------------------------------------------------
// Topology creation
// ---------------------------------------------------------------------------

/// Port of AST_TEST_DEFINE(stream_topology_create) from test_stream.c.
///
/// Test that creating a stream topology works.
#[test]
fn test_stream_topology_create() {
    let topology = StreamTopology::new();
    assert_eq!(topology.count(), 0);
}

// ---------------------------------------------------------------------------
// Topology append
// ---------------------------------------------------------------------------

/// Port of AST_TEST_DEFINE(stream_topology_append_stream) from test_stream.c.
///
/// Test that appending streams to a topology works.
#[test]
fn test_stream_topology_append() {
    let mut topology = StreamTopology::new();

    let audio = Stream::new("audio", MediaType::Audio);
    let pos = topology.append(audio);
    assert_eq!(pos, 0);
    assert_eq!(topology.count(), 1);
    assert_eq!(topology.get(0).unwrap().name(), "audio");
    assert_eq!(topology.get(0).unwrap().position(), 0);

    let video = Stream::new("video", MediaType::Video);
    let pos = topology.append(video);
    assert_eq!(pos, 1);
    assert_eq!(topology.count(), 2);
    assert_eq!(topology.get(1).unwrap().name(), "video");
    assert_eq!(topology.get(1).unwrap().position(), 1);
}

// ---------------------------------------------------------------------------
// Topology set stream
// ---------------------------------------------------------------------------

/// Port of AST_TEST_DEFINE(stream_topology_set_stream) from test_stream.c.
///
/// Test setting streams at specific positions in a topology.
#[test]
fn test_stream_topology_set_stream() {
    let mut topology = StreamTopology::new();

    // Set at position 0 (topology grows).
    let audio = Stream::new("audio", MediaType::Audio);
    topology.set_stream(0, audio);
    assert_eq!(topology.count(), 1);
    assert_eq!(topology.get(0).unwrap().media_type(), MediaType::Audio);
    assert_eq!(topology.get(0).unwrap().position(), 0);

    // Replace at position 0 with video.
    let video = Stream::new("video", MediaType::Video);
    topology.set_stream(0, video);
    assert_eq!(topology.count(), 1);
    assert_eq!(topology.get(0).unwrap().media_type(), MediaType::Video);

    // Set at position 1 (extends topology).
    let audio2 = Stream::new("audio2", MediaType::Audio);
    topology.set_stream(1, audio2);
    assert_eq!(topology.count(), 2);
    assert_eq!(topology.get(1).unwrap().media_type(), MediaType::Audio);
    assert_eq!(topology.get(1).unwrap().position(), 1);
}

// ---------------------------------------------------------------------------
// Topology delete stream
// ---------------------------------------------------------------------------

/// Port of AST_TEST_DEFINE(stream_topology_del_stream) from test_stream.c.
///
/// Test deleting streams from a topology.
#[test]
fn test_stream_topology_del_stream() {
    let mut topology = StreamTopology::new();

    // Add several streams of different types.
    let types = [
        MediaType::Unknown,
        MediaType::Audio,
        MediaType::Video,
        MediaType::Image,
        MediaType::Text,
    ];
    for (i, &mt) in types.iter().enumerate() {
        let name = format!("stream{}", i);
        topology.append(Stream::new(&name, mt));
    }
    assert_eq!(topology.count(), 5);

    // Delete outside of range should fail.
    assert!(topology.del_stream(topology.count()).is_err());

    // Delete last stream.
    assert!(topology.del_stream(topology.count() - 1).is_ok());
    assert_eq!(topology.count(), 4);

    // Verify positions are updated.
    for i in 0..topology.count() {
        assert_eq!(topology.get(i).unwrap().position(), i);
    }

    // Delete second stream (index 1).
    assert!(topology.del_stream(1).is_ok());
    assert_eq!(topology.count(), 3);

    // Positions should be updated.
    for i in 0..topology.count() {
        assert_eq!(topology.get(i).unwrap().position(), i);
    }

    // Delete first stream (index 0).
    assert!(topology.del_stream(0).is_ok());
    assert_eq!(topology.count(), 2);
    for i in 0..topology.count() {
        assert_eq!(topology.get(i).unwrap().position(), i);
    }
}

// ---------------------------------------------------------------------------
// Topology clone
// ---------------------------------------------------------------------------

/// Port of AST_TEST_DEFINE(stream_topology_clone) from test_stream.c.
///
/// Test cloning a stream topology.
#[test]
fn test_stream_topology_clone() {
    let mut topology = StreamTopology::new();

    let mut audio = Stream::new("audio", MediaType::Audio);
    audio.set_metadata("track_label", Some("audio-label"));
    topology.append(audio);

    let mut video = Stream::new("video", MediaType::Video);
    video.set_metadata("track_label", Some("video-label"));
    topology.append(video);

    let cloned = topology.clone_topology();

    assert_eq!(cloned.count(), topology.count());

    // Audio stream preserved.
    assert_eq!(
        cloned.get(0).unwrap().media_type(),
        topology.get(0).unwrap().media_type()
    );
    assert_eq!(
        cloned.get(0).unwrap().get_metadata("track_label"),
        topology.get(0).unwrap().get_metadata("track_label")
    );

    // Video stream preserved.
    assert_eq!(
        cloned.get(1).unwrap().media_type(),
        topology.get(1).unwrap().media_type()
    );
    assert_eq!(
        cloned.get(1).unwrap().get_metadata("track_label"),
        topology.get(1).unwrap().get_metadata("track_label")
    );
}

// ---------------------------------------------------------------------------
// Multi-stream topology operations
// ---------------------------------------------------------------------------

/// Test building a topology with many streams.
#[test]
fn test_stream_topology_many_streams() {
    let mut topology = StreamTopology::new();

    for i in 0..20 {
        let mt = if i % 2 == 0 {
            MediaType::Audio
        } else {
            MediaType::Video
        };
        topology.append(Stream::new(&format!("s{}", i), mt));
    }

    assert_eq!(topology.count(), 20);

    for i in 0..20 {
        let s = topology.get(i).unwrap();
        assert_eq!(s.position(), i);
        assert_eq!(s.name(), format!("s{}", i));
    }
}

/// Test that state changes are independent per stream.
#[test]
fn test_stream_independent_state() {
    let mut s1 = Stream::new("audio", MediaType::Audio);
    let mut s2 = Stream::new("video", MediaType::Video);

    s1.set_state(StreamState::SendRecv);
    s2.set_state(StreamState::RecvOnly);

    assert_eq!(s1.state(), StreamState::SendRecv);
    assert_eq!(s2.state(), StreamState::RecvOnly);
}

/// Test empty topology operations don't panic.
#[test]
fn test_stream_topology_empty_operations() {
    let mut topology = StreamTopology::new();
    assert_eq!(topology.count(), 0);
    assert!(topology.get(0).is_none());
    assert!(topology.del_stream(0).is_err());
}
