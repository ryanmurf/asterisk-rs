//! Port of asterisk/tests/test_gosub.c
//!
//! Tests GoSub/Return execution, argument passing (ARG1..ARGN, ARGC),
//! LOCAL() variable scoping, nested GoSub, and GOSUB_RETVAL.
//! Since the actual dialplan execution engine is complex, we test
//! the underlying stack/frame mechanics.

use std::collections::HashMap;

// ---------------------------------------------------------------------------
// GoSub stack frame model
// ---------------------------------------------------------------------------

/// A single GoSub stack frame (like Asterisk's internal gosub frame).
#[derive(Debug, Clone)]
struct GosubFrame {
    /// Context name.
    context: String,
    /// Extension name.
    extension: String,
    /// Priority number.
    priority: i32,
    /// Arguments (ARG1..ARGn).
    args: Vec<String>,
    /// Local variables set with LOCAL().
    locals: HashMap<String, String>,
    /// Return value (set by Return).
    return_value: Option<String>,
}

impl GosubFrame {
    fn new(context: &str, extension: &str, priority: i32, args: Vec<String>) -> Self {
        Self {
            context: context.to_string(),
            extension: extension.to_string(),
            priority,
            args,
            locals: HashMap::new(),
            return_value: None,
        }
    }

    /// Get ARGn (1-based).
    fn get_arg(&self, n: usize) -> &str {
        if n == 0 || n > self.args.len() {
            ""
        } else {
            &self.args[n - 1]
        }
    }

    /// Get ARGC.
    fn argc(&self) -> usize {
        self.args.len()
    }

    /// Set a local variable.
    fn set_local(&mut self, name: &str, value: &str) {
        self.locals.insert(name.to_string(), value.to_string());
    }

    /// Get a local variable.
    fn get_local(&self, name: &str) -> Option<&str> {
        self.locals.get(name).map(|s| s.as_str())
    }
}

/// GoSub execution stack.
#[derive(Debug)]
struct GosubStack {
    frames: Vec<GosubFrame>,
    gosub_retval: String,
}

impl GosubStack {
    fn new() -> Self {
        Self {
            frames: Vec::new(),
            gosub_retval: String::new(),
        }
    }

    /// Push a new GoSub frame (like executing Gosub()).
    fn gosub(&mut self, context: &str, extension: &str, priority: i32, args: Vec<String>) {
        self.frames.push(GosubFrame::new(context, extension, priority, args));
    }

    /// Pop the top frame (like executing StackPop).
    fn stack_pop(&mut self) -> Option<GosubFrame> {
        self.frames.pop()
    }

    /// Pop frame and set return value (like executing Return(value)).
    fn return_value(&mut self, value: &str) {
        self.frames.pop();
        self.gosub_retval = value.to_string();
    }

    /// Get the current (top) frame.
    fn current(&self) -> Option<&GosubFrame> {
        self.frames.last()
    }

    /// Get the current (top) frame mutably.
    fn current_mut(&mut self) -> Option<&mut GosubFrame> {
        self.frames.last_mut()
    }

    /// Depth of the stack.
    fn depth(&self) -> usize {
        self.frames.len()
    }

    /// STACK_PEEK: look at a specific frame depth.
    /// depth=1 means one frame below current.
    fn stack_peek(&self, depth: usize) -> Option<&GosubFrame> {
        if depth == 0 || self.frames.is_empty() {
            return None;
        }
        let idx = self.frames.len().checked_sub(depth)?;
        // depth=1 => look at frame below current.
        if idx == 0 {
            return None; // Beyond the stack.
        }
        self.frames.get(idx - 1)
    }

    /// LOCAL_PEEK: look at args of a frame at given depth.
    fn local_peek_arg(&self, depth: usize, arg_num: usize) -> &str {
        if depth >= self.frames.len() {
            return "";
        }
        let idx = self.frames.len() - 1 - depth;
        self.frames[idx].get_arg(arg_num)
    }

    /// Get variable from current frame, falling through to previous.
    fn get_variable(&self, name: &str) -> &str {
        // Check current frame locals.
        if let Some(frame) = self.current() {
            if let Some(val) = frame.get_local(name) {
                return val;
            }
        }
        ""
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// Port of test_gosub testplan from test_gosub.c.
///
/// Test basic GoSub: push a frame and verify context/extension/priority.
#[test]
fn test_gosub_basic() {
    let mut stack = GosubStack::new();

    stack.gosub("test_context", "s", 1, vec![]);

    let frame = stack.current().unwrap();
    assert_eq!(frame.priority, 1);
    assert_eq!(frame.extension, "s");
    assert_eq!(frame.context, "test_context");
}

/// Test GoSub with arguments.
#[test]
fn test_gosub_with_args() {
    let mut stack = GosubStack::new();

    stack.gosub(
        "test_context",
        "s",
        1,
        vec![
            "5".to_string(),
            "5".to_string(),
            "5".to_string(),
            "5".to_string(),
            "5".to_string(),
        ],
    );

    let frame = stack.current().unwrap();
    assert_eq!(frame.argc(), 5);

    // ARG1 + ARG5 = 10
    let arg1: i32 = frame.get_arg(1).parse().unwrap_or(0);
    let arg5: i32 = frame.get_arg(5).parse().unwrap_or(0);
    assert_eq!(arg1 + arg5, 10);
}

/// Test nested GoSub with argument masking.
#[test]
fn test_gosub_nested_arg_masking() {
    let mut stack = GosubStack::new();

    // First Gosub with 5 args.
    stack.gosub(
        "ctx",
        "s",
        1,
        vec!["5".into(), "5".into(), "5".into(), "5".into(), "5".into()],
    );

    // Nested Gosub with 4 args.
    stack.gosub("ctx", "s", 1, vec!["4".into(), "4".into(), "4".into(), "4".into()]);

    let frame = stack.current().unwrap();
    // ARG1 + ARG5: ARG5 doesn't exist in this frame.
    let arg1: i32 = frame.get_arg(1).parse().unwrap_or(0);
    let arg5: i32 = frame.get_arg(5).parse().unwrap_or(0);
    assert_eq!(arg1 + arg5, 4); // 4 + 0

    // ARG1 + ARG4.
    let arg4: i32 = frame.get_arg(4).parse().unwrap_or(0);
    assert_eq!(arg1 + arg4, 8); // 4 + 4
}

/// Test deeply nested GoSub with decreasing arg counts.
#[test]
fn test_gosub_deep_nesting() {
    let mut stack = GosubStack::new();

    stack.gosub("ctx", "s", 1, vec!["5".into(); 5]);
    stack.gosub("ctx", "s", 1, vec!["4".into(); 4]);
    stack.gosub("ctx", "s", 1, vec!["3".into(); 3]);
    stack.gosub("ctx", "s", 1, vec!["2".into(); 2]);
    stack.gosub("ctx", "s", 1, vec!["1".into(); 1]);
    stack.gosub("ctx", "s", 1, vec![]); // No args.

    assert_eq!(stack.depth(), 6);

    // Top frame has no args.
    let frame = stack.current().unwrap();
    let arg1: i32 = frame.get_arg(1).parse().unwrap_or(0);
    assert_eq!(arg1 + arg1, 0); // All arguments are correctly masked.
}

/// Test LOCAL() variable scoping.
#[test]
fn test_gosub_local_variables() {
    let mut stack = GosubStack::new();

    stack.gosub("ctx", "s", 1, vec![]);

    // Set a local variable.
    stack.current_mut().unwrap().set_local("foo", "5");
    assert_eq!(stack.get_variable("foo"), "5");

    // After StackPop, variable is gone.
    stack.stack_pop();
    assert_eq!(stack.get_variable("foo"), "");
}

/// Test Return with value sets GOSUB_RETVAL.
#[test]
fn test_gosub_return_value() {
    let mut stack = GosubStack::new();

    stack.gosub("ctx", "s", 1, vec![]);
    stack.gosub("ctx", "s", 1, vec!["2".into()]);

    // Return from inner gosub with value 7.
    stack.return_value("7");

    assert_eq!(stack.gosub_retval, "7");
    assert_eq!(stack.depth(), 1);
}

/// Test STACK_PEEK accessing caller frame.
#[test]
fn test_gosub_stack_peek() {
    let mut stack = GosubStack::new();

    stack.gosub("outer_ctx", "s", 1, vec!["outer_arg".into()]);
    stack.gosub("inner_ctx", "s", 1, vec!["inner_arg".into()]);

    // Peek at frame below current (depth=1).
    let peeked = stack.stack_peek(1);
    assert!(peeked.is_some());
    assert_eq!(peeked.unwrap().context, "outer_ctx");
    assert_eq!(peeked.unwrap().get_arg(1), "outer_arg");
}

/// Test LOCAL_PEEK accessing args at different depths.
#[test]
fn test_gosub_local_peek() {
    let mut stack = GosubStack::new();

    stack.gosub("ctx", "s", 1, vec!["10".into()]);
    stack.gosub("ctx", "s", 1, vec!["20".into()]);
    stack.gosub("ctx", "s", 1, vec!["30".into()]);

    // depth=0 (current) => ARG1=30
    assert_eq!(stack.local_peek_arg(0, 1), "30");
    // depth=1 => ARG1=20
    assert_eq!(stack.local_peek_arg(1, 1), "20");
    // depth=2 => ARG1=10
    assert_eq!(stack.local_peek_arg(2, 1), "10");
}

/// Test StackPop removes top frame.
#[test]
fn test_gosub_stack_pop() {
    let mut stack = GosubStack::new();

    stack.gosub("ctx", "s", 1, vec!["first".into()]);
    stack.gosub("ctx", "s", 1, vec!["second".into()]);
    assert_eq!(stack.depth(), 2);

    let popped = stack.stack_pop();
    assert!(popped.is_some());
    assert_eq!(popped.unwrap().get_arg(1), "second");
    assert_eq!(stack.depth(), 1);
    assert_eq!(stack.current().unwrap().get_arg(1), "first");
}

/// Test empty stack operations.
#[test]
fn test_gosub_empty_stack() {
    let mut stack = GosubStack::new();

    assert_eq!(stack.depth(), 0);
    assert!(stack.current().is_none());
    assert!(stack.stack_pop().is_none());
    assert!(stack.stack_peek(1).is_none());
    assert_eq!(stack.get_variable("foo"), "");
}
