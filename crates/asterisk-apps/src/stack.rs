//! Stack applications - GoSub/Return dialplan subroutine support.
//!
//! Port of app_stack.c from Asterisk C. Provides GoSub(), GoSubIf(),
//! Return(), and StackPop() applications for dialplan subroutine calls,
//! along with LOCAL() function support for subroutine-scoped variables.

use crate::{DialplanApp, PbxExecResult};
use asterisk_core::channel::Channel;
use std::collections::HashMap;
use tracing::{debug, info, warn};

/// A single frame on the GoSub call stack.
///
/// Stores the return address (context, extension, priority) and any
/// local variables scoped to this subroutine invocation.
#[derive(Debug, Clone)]
pub struct GoSubFrame {
    /// Return context.
    pub context: String,
    /// Return extension.
    pub exten: String,
    /// Return priority.
    pub priority: i32,
    /// Local variables for this stack frame. These shadow channel variables
    /// and are restored when Return() is called.
    pub local_variables: HashMap<String, String>,
    /// Saved channel variables that were overridden by LOCAL().
    pub saved_variables: HashMap<String, Option<String>>,
    /// Arguments passed to the subroutine (%ARG1%, %ARG2%, etc.)
    pub arguments: Vec<String>,
}

/// Maximum GoSub stack depth to prevent unbounded memory growth from
/// recursive subroutine calls.
pub const MAX_GOSUB_DEPTH: usize = 128;

/// The GoSub call stack stored as a channel datastore.
///
/// Each channel maintains its own call stack for nested subroutine calls.
#[derive(Debug, Clone, Default)]
pub struct GoSubStack {
    /// Stack frames, most recent at the end.
    pub frames: Vec<GoSubFrame>,
}

impl GoSubStack {
    /// Create a new empty stack.
    pub fn new() -> Self {
        Self { frames: Vec::new() }
    }

    /// Push a new frame onto the stack.
    pub fn push(&mut self, frame: GoSubFrame) {
        self.frames.push(frame);
    }

    /// Pop the top frame from the stack.
    pub fn pop(&mut self) -> Option<GoSubFrame> {
        self.frames.pop()
    }

    /// Peek at the top frame without removing it.
    pub fn peek(&self) -> Option<&GoSubFrame> {
        self.frames.last()
    }

    /// Get the current stack depth.
    pub fn depth(&self) -> usize {
        self.frames.len()
    }

    /// Check if the stack is empty.
    pub fn is_empty(&self) -> bool {
        self.frames.is_empty()
    }
}

/// The datastore key for the GoSub stack on a channel.
pub const GOSUB_STACK_KEY: &str = "gosub_stack";

/// Get (or create) the GoSub stack from a channel's datastores.
fn get_or_create_stack(channel: &mut Channel) -> GoSubStack {
    if let Some(ds) = channel.datastores.get(GOSUB_STACK_KEY) {
        if let Some(stack) = ds.downcast_ref::<GoSubStack>() {
            return stack.clone();
        }
    }
    GoSubStack::new()
}

/// Save the GoSub stack back into the channel's datastores.
fn save_stack(channel: &mut Channel, stack: GoSubStack) {
    channel
        .datastores
        .insert(GOSUB_STACK_KEY.to_string(), Box::new(stack));
}

/// Parsed GoSub destination.
#[derive(Debug, Clone)]
pub struct GoSubDest {
    /// Target context (None = keep current).
    pub context: Option<String>,
    /// Target extension (None = keep current).
    pub exten: Option<String>,
    /// Target priority.
    pub priority: i32,
    /// Arguments passed to the subroutine.
    pub arguments: Vec<String>,
}

impl GoSubDest {
    /// Parse a GoSub destination string.
    ///
    /// Format: `[[context,]exten,]priority[(arg1[,arg2[,...]])]`
    pub fn parse(args: &str) -> Option<Self> {
        let args = args.trim();
        if args.is_empty() {
            return None;
        }

        // Split off arguments in parentheses
        let (location, arguments) = if let Some(paren_pos) = args.find('(') {
            let loc = &args[..paren_pos];
            let arg_str = args[paren_pos + 1..].trim_end_matches(')');
            let arguments: Vec<String> = arg_str
                .split(',')
                .map(|s| s.trim().to_string())
                .collect();
            (loc, arguments)
        } else {
            (args, Vec::new())
        };

        // Parse context,exten,priority
        let parts: Vec<&str> = location.split(',').collect();
        match parts.len() {
            1 => {
                // Just priority
                let priority = parse_priority(parts[0].trim())?;
                Some(Self {
                    context: None,
                    exten: None,
                    priority,
                    arguments,
                })
            }
            2 => {
                // exten,priority
                let exten = parts[0].trim();
                let priority = parse_priority(parts[1].trim())?;
                Some(Self {
                    context: None,
                    exten: Some(exten.to_string()),
                    priority,
                    arguments,
                })
            }
            3 => {
                // context,exten,priority
                let context = parts[0].trim();
                let exten = parts[1].trim();
                let priority = parse_priority(parts[2].trim())?;
                Some(Self {
                    context: if context.is_empty() {
                        None
                    } else {
                        Some(context.to_string())
                    },
                    exten: if exten.is_empty() {
                        None
                    } else {
                        Some(exten.to_string())
                    },
                    priority,
                    arguments,
                })
            }
            _ => None,
        }
    }
}

/// Parse a priority string. Supports integer priorities and label 'n'
/// (next priority) as special values.
fn parse_priority(s: &str) -> Option<i32> {
    if s.eq_ignore_ascii_case("n") {
        // 'n' means next priority -- will be resolved at runtime
        return Some(-1);
    }
    s.parse::<i32>().ok()
}

/// The GoSub() dialplan application.
///
/// Jumps to a label in the dialplan, saving the return address on a
/// per-channel call stack. Use Return() to return to the saved position.
///
/// Usage: GoSub([[context,]exten,]priority[(arg1[,arg2[,...]])])
pub struct AppGoSub;

impl DialplanApp for AppGoSub {
    fn name(&self) -> &str {
        "GoSub"
    }

    fn description(&self) -> &str {
        "Jump to label, saving return address"
    }
}

impl AppGoSub {
    /// Execute the GoSub application.
    ///
    /// # Arguments
    /// * `channel` - The current channel
    /// * `args` - Destination: `[[context,]exten,]priority[(args)]`
    pub fn exec(channel: &mut Channel, args: &str) -> PbxExecResult {
        let dest = match GoSubDest::parse(args) {
            Some(d) => d,
            None => {
                warn!("GoSub: requires a destination argument");
                return PbxExecResult::Failed;
            }
        };

        // Save the current position as the return address
        let frame = GoSubFrame {
            context: channel.context.clone(),
            exten: channel.exten.clone(),
            priority: channel.priority,
            local_variables: HashMap::new(),
            saved_variables: HashMap::new(),
            arguments: dest.arguments.clone(),
        };

        let mut stack = get_or_create_stack(channel);

        if stack.depth() >= MAX_GOSUB_DEPTH {
            warn!(
                "GoSub: channel '{}' exceeded maximum stack depth of {}",
                channel.name, MAX_GOSUB_DEPTH
            );
            return PbxExecResult::Failed;
        }

        stack.push(frame);

        info!(
            "GoSub: channel '{}' jumping to {:?},{:?},{} (stack depth: {})",
            channel.name,
            dest.context,
            dest.exten,
            dest.priority,
            stack.depth()
        );

        // Set arguments as channel variables ARG1, ARG2, etc.
        // Also set ARGC to the number of arguments
        channel.set_variable("ARGC", dest.arguments.len().to_string());
        for (i, arg) in dest.arguments.iter().enumerate() {
            channel.set_variable(format!("ARG{}", i + 1), arg);
        }

        // Update channel position
        if let Some(ref context) = dest.context {
            channel.context = context.clone();
        }
        if let Some(ref exten) = dest.exten {
            channel.exten = exten.clone();
        }
        channel.priority = dest.priority;

        save_stack(channel, stack);

        PbxExecResult::Success
    }
}

/// The Return() dialplan application.
///
/// Returns from a GoSub() subroutine, jumping back to the saved
/// return address. An optional value is saved in GOSUB_RETVAL.
///
/// Usage: Return([value])
pub struct AppReturn;

impl DialplanApp for AppReturn {
    fn name(&self) -> &str {
        "Return"
    }

    fn description(&self) -> &str {
        "Return from gosub routine"
    }
}

impl AppReturn {
    /// Execute the Return application.
    ///
    /// # Arguments
    /// * `channel` - The current channel
    /// * `args` - Optional return value
    pub fn exec(channel: &mut Channel, args: &str) -> PbxExecResult {
        let mut stack = get_or_create_stack(channel);

        let frame = match stack.pop() {
            Some(f) => f,
            None => {
                warn!(
                    "Return: called on channel '{}' with no GoSub stack",
                    channel.name
                );
                return PbxExecResult::Failed;
            }
        };

        // Save return value if provided
        if !args.trim().is_empty() {
            channel.set_variable("GOSUB_RETVAL", args.trim());
        }

        // Restore any saved variables from LOCAL() overrides
        for (name, original_value) in &frame.saved_variables {
            match original_value {
                Some(val) => {
                    channel.set_variable(name, val);
                }
                None => {
                    channel.variables.remove(name);
                }
            }
        }

        // Restore the return address
        channel.context = frame.context.clone();
        channel.exten = frame.exten.clone();
        channel.priority = frame.priority;

        info!(
            "Return: channel '{}' returning to {}@{} priority {} (stack depth: {})",
            channel.name, frame.exten, frame.context, frame.priority, stack.depth()
        );

        save_stack(channel, stack);

        PbxExecResult::Success
    }
}

/// The StackPop() dialplan application.
///
/// Removes the top entry from the GoSub stack without returning to it.
/// Useful for discarding a return address when you want to continue
/// execution at the current location.
///
/// Usage: StackPop()
pub struct AppStackPop;

impl DialplanApp for AppStackPop {
    fn name(&self) -> &str {
        "StackPop"
    }

    fn description(&self) -> &str {
        "Remove one address from gosub stack"
    }
}

impl AppStackPop {
    /// Execute the StackPop application.
    pub fn exec(channel: &mut Channel, _args: &str) -> PbxExecResult {
        let mut stack = get_or_create_stack(channel);

        if let Some(frame) = stack.pop() {
            debug!(
                "StackPop: removed return address {}@{} priority {} from channel '{}'",
                frame.exten, frame.context, frame.priority, channel.name
            );
        } else {
            warn!(
                "StackPop: called on channel '{}' with empty GoSub stack",
                channel.name
            );
        }

        save_stack(channel, stack);

        PbxExecResult::Success
    }
}

/// The GoSubIf() dialplan application.
///
/// Conditionally jumps to a label, saving the return address.
///
/// Usage: GoSubIf(condition?labeliftrue:labeliffalse)
pub struct AppGoSubIf;

impl DialplanApp for AppGoSubIf {
    fn name(&self) -> &str {
        "GoSubIf"
    }

    fn description(&self) -> &str {
        "Conditionally jump to label, saving return address"
    }
}

impl AppGoSubIf {
    /// Execute the GoSubIf application.
    ///
    /// # Arguments
    /// * `channel` - The current channel
    /// * `args` - `condition?labeliftrue[:labeliffalse]`
    pub fn exec(channel: &mut Channel, args: &str) -> PbxExecResult {
        let (condition, destinations) = match args.split_once('?') {
            Some((c, d)) => (c.trim(), d),
            None => {
                warn!("GoSubIf: requires condition?labeliftrue[:labeliffalse]");
                return PbxExecResult::Failed;
            }
        };

        // Evaluate condition: non-empty, non-zero string is true
        let is_true = !condition.is_empty() && condition != "0" && !condition.is_empty();

        let target = if is_true {
            // Use label before ':'
            if let Some(colon_pos) = destinations.find(':') {
                &destinations[..colon_pos]
            } else {
                destinations
            }
        } else {
            // Use label after ':'
            if let Some(colon_pos) = destinations.find(':') {
                &destinations[colon_pos + 1..]
            } else {
                // No false label -- just continue
                return PbxExecResult::Success;
            }
        };

        let target = target.trim();
        if target.is_empty() {
            return PbxExecResult::Success;
        }

        debug!(
            "GoSubIf: condition='{}' is {}, jumping to '{}'",
            condition,
            if is_true { "true" } else { "false" },
            target
        );

        AppGoSub::exec(channel, target)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_gosub_dest_priority_only() {
        let dest = GoSubDest::parse("1").unwrap();
        assert!(dest.context.is_none());
        assert!(dest.exten.is_none());
        assert_eq!(dest.priority, 1);
        assert!(dest.arguments.is_empty());
    }

    #[test]
    fn test_parse_gosub_dest_exten_priority() {
        let dest = GoSubDest::parse("handler,1").unwrap();
        assert!(dest.context.is_none());
        assert_eq!(dest.exten.as_deref(), Some("handler"));
        assert_eq!(dest.priority, 1);
    }

    #[test]
    fn test_parse_gosub_dest_full() {
        let dest = GoSubDest::parse("subroutines,validate,1").unwrap();
        assert_eq!(dest.context.as_deref(), Some("subroutines"));
        assert_eq!(dest.exten.as_deref(), Some("validate"));
        assert_eq!(dest.priority, 1);
    }

    #[test]
    fn test_parse_gosub_dest_with_args() {
        let dest = GoSubDest::parse("sub,handler,1(arg1,arg2,arg3)").unwrap();
        assert_eq!(dest.context.as_deref(), Some("sub"));
        assert_eq!(dest.exten.as_deref(), Some("handler"));
        assert_eq!(dest.priority, 1);
        assert_eq!(dest.arguments, vec!["arg1", "arg2", "arg3"]);
    }

    #[test]
    fn test_parse_gosub_dest_empty() {
        assert!(GoSubDest::parse("").is_none());
    }

    #[test]
    fn test_gosub_return_roundtrip() {
        let mut channel = Channel::new("SIP/test-001");
        channel.context = "default".to_string();
        channel.exten = "100".to_string();
        channel.priority = 5;

        // GoSub to subroutine
        let result = AppGoSub::exec(&mut channel, "subroutines,validate,1(hello)");
        assert_eq!(result, PbxExecResult::Success);
        assert_eq!(channel.context, "subroutines");
        assert_eq!(channel.exten, "validate");
        assert_eq!(channel.priority, 1);
        assert_eq!(channel.get_variable("ARG1"), Some("hello"));
        assert_eq!(channel.get_variable("ARGC"), Some("1"));

        // Return
        let result = AppReturn::exec(&mut channel, "42");
        assert_eq!(result, PbxExecResult::Success);
        assert_eq!(channel.context, "default");
        assert_eq!(channel.exten, "100");
        assert_eq!(channel.priority, 5);
        assert_eq!(channel.get_variable("GOSUB_RETVAL"), Some("42"));
    }

    #[test]
    fn test_return_empty_stack() {
        let mut channel = Channel::new("SIP/test-001");
        let result = AppReturn::exec(&mut channel, "");
        assert_eq!(result, PbxExecResult::Failed);
    }

    #[test]
    fn test_nested_gosub() {
        let mut channel = Channel::new("SIP/test-001");
        channel.context = "default".to_string();
        channel.exten = "100".to_string();
        channel.priority = 1;

        // First GoSub
        AppGoSub::exec(&mut channel, "sub,handler1,1");
        assert_eq!(channel.context, "sub");
        assert_eq!(channel.exten, "handler1");

        // Nested GoSub
        AppGoSub::exec(&mut channel, "sub,handler2,1");
        assert_eq!(channel.exten, "handler2");

        // Return from nested
        AppReturn::exec(&mut channel, "");
        assert_eq!(channel.exten, "handler1");

        // Return from first
        AppReturn::exec(&mut channel, "");
        assert_eq!(channel.context, "default");
        assert_eq!(channel.exten, "100");
        assert_eq!(channel.priority, 1);
    }

    #[test]
    fn test_stack_pop() {
        let mut channel = Channel::new("SIP/test-001");
        channel.context = "default".to_string();
        channel.exten = "100".to_string();
        channel.priority = 1;

        AppGoSub::exec(&mut channel, "sub,handler,1");
        assert_eq!(channel.exten, "handler");

        // Pop the stack instead of returning
        AppStackPop::exec(&mut channel, "");

        // Return should now fail (stack is empty)
        let result = AppReturn::exec(&mut channel, "");
        assert_eq!(result, PbxExecResult::Failed);
    }

    #[test]
    fn test_gosubif_true() {
        let mut channel = Channel::new("SIP/test-001");
        channel.context = "default".to_string();
        channel.exten = "100".to_string();
        channel.priority = 1;

        let result = AppGoSubIf::exec(&mut channel, "1?sub,true_handler,1:sub,false_handler,1");
        assert_eq!(result, PbxExecResult::Success);
        assert_eq!(channel.exten, "true_handler");
    }

    #[test]
    fn test_gosubif_false() {
        let mut channel = Channel::new("SIP/test-001");
        channel.context = "default".to_string();
        channel.exten = "100".to_string();
        channel.priority = 1;

        let result = AppGoSubIf::exec(&mut channel, "0?sub,true_handler,1:sub,false_handler,1");
        assert_eq!(result, PbxExecResult::Success);
        assert_eq!(channel.exten, "false_handler");
    }

    #[test]
    fn test_gosubif_false_no_label() {
        let mut channel = Channel::new("SIP/test-001");
        channel.context = "default".to_string();
        channel.exten = "100".to_string();
        channel.priority = 1;

        let result = AppGoSubIf::exec(&mut channel, "0?sub,true_handler,1");
        assert_eq!(result, PbxExecResult::Success);
        // Should stay at current position
        assert_eq!(channel.exten, "100");
    }
}
