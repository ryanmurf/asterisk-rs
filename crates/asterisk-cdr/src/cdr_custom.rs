//! Custom format CDR backend.
//!
//! Port of cdr/cdr_custom.c from Asterisk C.
//!
//! Supports configurable output templates with variable substitution.
//! Each output definition specifies a filename and a template string
//! containing ${CDR(field)} references that get expanded at log time.

use crate::{Cdr, CdrBackend, CdrError};
use parking_lot::Mutex;
use std::io::Write;
use std::path::{Path, PathBuf};
use tracing::debug;

/// A single custom CDR output configuration.
///
/// Each configuration maps to one output file with its own template.
#[derive(Debug, Clone)]
pub struct CustomCdrOutput {
    /// Name of the output (section name in config)
    pub name: String,
    /// Path to the output file
    pub file_path: PathBuf,
    /// Template string with ${CDR(field)} placeholders
    pub template: String,
}

/// Configuration for the custom CDR backend.
#[derive(Debug, Clone)]
pub struct CustomCdrConfig {
    /// Base log directory
    pub log_dir: PathBuf,
    /// List of configured outputs
    pub outputs: Vec<CustomCdrOutput>,
}

impl Default for CustomCdrConfig {
    fn default() -> Self {
        Self {
            log_dir: PathBuf::from("/var/log/asterisk/cdr-custom"),
            outputs: Vec::new(),
        }
    }
}

/// Custom format CDR backend.
///
/// Writes CDR records to files using configurable templates.
/// Supports multiple output files with different formats.
///
/// Template variables:
///   ${CDR(src)}         - source/caller number
///   ${CDR(dst)}         - destination extension
///   ${CDR(channel)}     - calling channel
///   ${CDR(dstchannel)}  - destination channel
///   ${CDR(disposition)} - call disposition
///   ${CDR(duration)}    - total duration in seconds
///   ${CDR(billsec)}     - billable seconds
///   ${CDR(start)}       - start time
///   ${CDR(answer)}      - answer time
///   ${CDR(end)}         - end time
///   ${CDR(accountcode)} - account code
///   ${CDR(uniqueid)}    - unique ID
///   ${CDR(userfield)}   - user field
///   ${CDR(lastapp)}     - last application
///   ${CDR(lastdata)}    - last application data
pub struct CustomCdrBackend {
    config: CustomCdrConfig,
    write_lock: Mutex<()>,
}

impl CustomCdrBackend {
    /// Create a new custom CDR backend with the given configuration.
    pub fn with_config(config: CustomCdrConfig) -> Self {
        Self {
            config,
            write_lock: Mutex::new(()),
        }
    }

    /// Create a backend with a single output.
    pub fn single_output(name: &str, file_path: PathBuf, template: &str) -> Self {
        let config = CustomCdrConfig {
            log_dir: file_path
                .parent()
                .unwrap_or(Path::new("/var/log/asterisk"))
                .to_path_buf(),
            outputs: vec![CustomCdrOutput {
                name: name.to_string(),
                file_path,
                template: template.to_string(),
            }],
        };
        Self::with_config(config)
    }

    /// Expand template variables in a string using CDR field values.
    fn expand_template(template: &str, cdr: &Cdr) -> String {
        let mut result = template.to_string();

        // Replace each ${CDR(field)} with the corresponding CDR value
        let replacements = [
            ("${CDR(src)}", cdr.src.as_str()),
            ("${CDR(dst)}", cdr.dst.as_str()),
            ("${CDR(dcontext)}", cdr.dst_context.as_str()),
            ("${CDR(channel)}", cdr.channel.as_str()),
            ("${CDR(dstchannel)}", cdr.dst_channel.as_str()),
            ("${CDR(lastapp)}", cdr.last_app.as_str()),
            ("${CDR(lastdata)}", cdr.last_data.as_str()),
            ("${CDR(disposition)}", cdr.disposition.as_str()),
            ("${CDR(amaflags)}", cdr.ama_flags.as_str()),
            ("${CDR(accountcode)}", cdr.account_code.as_str()),
            ("${CDR(uniqueid)}", cdr.unique_id.as_str()),
            ("${CDR(userfield)}", cdr.user_field.as_str()),
            ("${CDR(linkedid)}", cdr.linked_id.as_str()),
            ("${CDR(peeraccount)}", cdr.peer_account.as_str()),
            ("${CDR(clid)}", cdr.caller_id.as_str()),
        ];

        for (var, val) in &replacements {
            result = result.replace(var, val);
        }

        // Handle numeric fields
        result = result.replace("${CDR(duration)}", &cdr.duration.to_string());
        result = result.replace("${CDR(billsec)}", &cdr.billsec.to_string());
        result = result.replace("${CDR(sequence)}", &cdr.sequence.to_string());

        result
    }

    /// Write a line to a file.
    fn write_to_file(path: &Path, line: &str) -> Result<(), CdrError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)?;

        file.write_all(line.as_bytes())?;
        file.write_all(b"\n")?;
        file.flush()?;

        Ok(())
    }
}

impl CdrBackend for CustomCdrBackend {
    fn name(&self) -> &str {
        "custom"
    }

    fn log(&self, cdr: &Cdr) -> Result<(), CdrError> {
        let _lock = self.write_lock.lock();

        for output in &self.config.outputs {
            let expanded = Self::expand_template(&output.template, cdr);
            debug!(
                "CDR custom [{}]: writing to '{}'",
                output.name,
                output.file_path.display()
            );
            Self::write_to_file(&output.file_path, &expanded)?;
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::CdrDisposition;

    #[test]
    fn test_expand_template() {
        let mut cdr = Cdr::new("SIP/alice-001".to_string(), "uid-123".to_string());
        cdr.src = "5551234".to_string();
        cdr.dst = "100".to_string();
        cdr.disposition = CdrDisposition::Answered;
        cdr.duration = 120;
        cdr.billsec = 90;

        let template = "\"${CDR(src)}\",\"${CDR(dst)}\",\"${CDR(channel)}\",${CDR(duration)},${CDR(billsec)},\"${CDR(disposition)}\"";
        let result = CustomCdrBackend::expand_template(template, &cdr);

        assert!(result.contains("\"5551234\""));
        assert!(result.contains("\"100\""));
        assert!(result.contains("\"SIP/alice-001\""));
        assert!(result.contains("120"));
        assert!(result.contains("90"));
        assert!(result.contains("\"ANSWERED\""));
    }

    #[test]
    fn test_expand_template_no_vars() {
        let cdr = Cdr::new("SIP/test".to_string(), "uid".to_string());
        let template = "static text with no variables";
        let result = CustomCdrBackend::expand_template(template, &cdr);
        assert_eq!(result, template);
    }
}
