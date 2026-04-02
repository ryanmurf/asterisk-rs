//! Sorcery object persistence framework.
//!
//! Port of `res/res_sorcery_config.c`, `res_sorcery_memory.c`,
//! `res_sorcery_realtime.c`, and `res_sorcery_astdb.c`. Sorcery is Asterisk's
//! object mapping framework for persisting typed objects to various backends
//! (config files, in-memory, realtime databases, internal database).

use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;

use parking_lot::RwLock;
use thiserror::Error;
use tracing::{debug, info, warn};

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Error, Debug)]
pub enum SorceryError {
    #[error("object not found: type={0} id={1}")]
    NotFound(String, String),
    #[error("object already exists: type={0} id={1}")]
    AlreadyExists(String, String),
    #[error("wizard not found: {0}")]
    WizardNotFound(String),
    #[error("wizard error: {0}")]
    WizardError(String),
    #[error("sorcery error: {0}")]
    Other(String),
}

pub type SorceryResult<T> = Result<T, SorceryError>;

// ---------------------------------------------------------------------------
// Sorcery object (generic representation)
// ---------------------------------------------------------------------------

/// A generic sorcery object with typed fields stored as key-value strings.
///
/// In the C source, sorcery objects are opaque `void *` with registered
/// field handlers. Here we use a generic representation.
#[derive(Debug, Clone)]
pub struct SorceryObject {
    /// Unique identifier for this object.
    pub id: String,
    /// Object type name (e.g., "endpoint", "aor", "auth").
    pub object_type: String,
    /// Field values as key-value pairs.
    pub fields: HashMap<String, String>,
}

impl SorceryObject {
    /// Create a new sorcery object.
    pub fn new(object_type: &str, id: &str) -> Self {
        Self {
            id: id.to_string(),
            object_type: object_type.to_string(),
            fields: HashMap::new(),
        }
    }

    /// Set a field value.
    pub fn set_field(&mut self, name: &str, value: &str) {
        self.fields.insert(name.to_string(), value.to_string());
    }

    /// Get a field value.
    pub fn get_field(&self, name: &str) -> Option<&str> {
        self.fields.get(name).map(|s| s.as_str())
    }

    /// Check if a set of criteria fields match this object.
    pub fn matches(&self, criteria: &[(&str, &str)]) -> bool {
        criteria.iter().all(|(key, value)| {
            self.fields
                .get(*key)
                .map(|v| v == *value)
                .unwrap_or(false)
        })
    }

    /// Check if the ID matches a regex pattern.
    pub fn id_matches_prefix(&self, prefix: &str) -> bool {
        self.id.starts_with(prefix)
    }
}

// ---------------------------------------------------------------------------
// Sorcery wizard trait (mirrors `ast_sorcery_wizard`)
// ---------------------------------------------------------------------------

/// Trait for sorcery wizard implementations (backends).
///
/// Mirrors the `ast_sorcery_wizard` struct from the C source, providing
/// CRUD operations plus multi-retrieval methods.
pub trait SorceryWizard: Send + Sync + fmt::Debug {
    /// Wizard name (e.g., "memory", "config", "realtime", "astdb").
    fn name(&self) -> &str;

    /// Create a new object.
    fn create(&self, object: &SorceryObject) -> SorceryResult<()>;

    /// Retrieve an object by type and ID.
    fn retrieve_id(&self, object_type: &str, id: &str) -> SorceryResult<SorceryObject>;

    /// Retrieve an object by matching field criteria.
    fn retrieve_fields(
        &self,
        object_type: &str,
        fields: &[(&str, &str)],
    ) -> SorceryResult<SorceryObject>;

    /// Retrieve multiple objects matching field criteria.
    fn retrieve_multiple(
        &self,
        object_type: &str,
        fields: &[(&str, &str)],
    ) -> SorceryResult<Vec<SorceryObject>>;

    /// Retrieve objects whose ID matches a prefix.
    fn retrieve_prefix(
        &self,
        object_type: &str,
        prefix: &str,
    ) -> SorceryResult<Vec<SorceryObject>>;

    /// Update an existing object.
    fn update(&self, object: &SorceryObject) -> SorceryResult<()>;

    /// Delete an object.
    fn delete(&self, object: &SorceryObject) -> SorceryResult<()>;
}

// ---------------------------------------------------------------------------
// Memory wizard (res_sorcery_memory.c)
// ---------------------------------------------------------------------------

/// In-memory sorcery wizard.
///
/// Port of `res_sorcery_memory.c`. Stores objects in a HashMap, keyed
/// by `"{type}/{id}"`. Fast for testing and caching.
#[derive(Debug)]
pub struct MemorySorcery {
    objects: RwLock<HashMap<String, SorceryObject>>,
}

impl MemorySorcery {
    pub fn new() -> Self {
        Self {
            objects: RwLock::new(HashMap::new()),
        }
    }

    fn make_key(object_type: &str, id: &str) -> String {
        format!("{}/{}", object_type, id)
    }

    /// Get the number of stored objects.
    pub fn count(&self) -> usize {
        self.objects.read().len()
    }
}

impl Default for MemorySorcery {
    fn default() -> Self {
        Self::new()
    }
}

impl SorceryWizard for MemorySorcery {
    fn name(&self) -> &str {
        "memory"
    }

    fn create(&self, object: &SorceryObject) -> SorceryResult<()> {
        let key = Self::make_key(&object.object_type, &object.id);
        let mut objects = self.objects.write();
        if objects.contains_key(&key) {
            return Err(SorceryError::AlreadyExists(
                object.object_type.clone(),
                object.id.clone(),
            ));
        }
        objects.insert(key, object.clone());
        Ok(())
    }

    fn retrieve_id(&self, object_type: &str, id: &str) -> SorceryResult<SorceryObject> {
        let key = Self::make_key(object_type, id);
        self.objects
            .read()
            .get(&key)
            .cloned()
            .ok_or_else(|| SorceryError::NotFound(object_type.to_string(), id.to_string()))
    }

    fn retrieve_fields(
        &self,
        object_type: &str,
        fields: &[(&str, &str)],
    ) -> SorceryResult<SorceryObject> {
        let _prefix = format!("{}/", object_type);
        self.objects
            .read()
            .values()
            .find(|obj| obj.object_type == object_type && obj.matches(fields))
            .cloned()
            .ok_or_else(|| {
                SorceryError::NotFound(object_type.to_string(), format!("fields={:?}", fields))
            })
    }

    fn retrieve_multiple(
        &self,
        object_type: &str,
        fields: &[(&str, &str)],
    ) -> SorceryResult<Vec<SorceryObject>> {
        let results: Vec<SorceryObject> = self
            .objects
            .read()
            .values()
            .filter(|obj| {
                obj.object_type == object_type
                    && (fields.is_empty() || obj.matches(fields))
            })
            .cloned()
            .collect();
        Ok(results)
    }

    fn retrieve_prefix(
        &self,
        object_type: &str,
        prefix: &str,
    ) -> SorceryResult<Vec<SorceryObject>> {
        let results: Vec<SorceryObject> = self
            .objects
            .read()
            .values()
            .filter(|obj| {
                obj.object_type == object_type && obj.id_matches_prefix(prefix)
            })
            .cloned()
            .collect();
        Ok(results)
    }

    fn update(&self, object: &SorceryObject) -> SorceryResult<()> {
        let key = Self::make_key(&object.object_type, &object.id);
        let mut objects = self.objects.write();
        if !objects.contains_key(&key) {
            return Err(SorceryError::NotFound(
                object.object_type.clone(),
                object.id.clone(),
            ));
        }
        objects.insert(key, object.clone());
        Ok(())
    }

    fn delete(&self, object: &SorceryObject) -> SorceryResult<()> {
        let key = Self::make_key(&object.object_type, &object.id);
        self.objects
            .write()
            .remove(&key)
            .ok_or_else(|| {
                SorceryError::NotFound(object.object_type.clone(), object.id.clone())
            })?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Config wizard (res_sorcery_config.c) -- read-only from .conf files
// ---------------------------------------------------------------------------

/// Configuration file sorcery wizard (read-only).
///
/// Port of `res_sorcery_config.c`. Objects are loaded from Asterisk .conf
/// files. This wizard does not support create/update/delete -- objects
/// are loaded at startup or reload.
#[derive(Debug)]
pub struct ConfigSorcery {
    /// Config filename to load from.
    pub filename: String,
    /// Loaded objects (populated on load/reload).
    objects: RwLock<HashMap<String, SorceryObject>>,
    /// Criteria for filtering config sections.
    criteria: RwLock<Vec<(String, String)>>,
}

impl ConfigSorcery {
    pub fn new(filename: &str) -> Self {
        Self {
            filename: filename.to_string(),
            objects: RwLock::new(HashMap::new()),
            criteria: RwLock::new(Vec::new()),
        }
    }

    /// Add criteria for filtering which config sections are loaded.
    pub fn add_criteria(&self, key: &str, value: &str) {
        self.criteria
            .write()
            .push((key.to_string(), value.to_string()));
    }

    /// Load objects from parsed config data (key-value sections).
    pub fn load_from_sections(
        &self,
        object_type: &str,
        sections: &[(String, Vec<(String, String)>)],
    ) {
        let mut objects = self.objects.write();
        objects.clear();
        for (section_name, fields) in sections {
            let mut obj = SorceryObject::new(object_type, section_name);
            for (key, value) in fields {
                obj.set_field(key, value);
            }
            let key = MemorySorcery::make_key(object_type, section_name);
            objects.insert(key, obj);
        }
        info!(
            filename = %self.filename,
            count = objects.len(),
            "Loaded config sorcery objects"
        );
    }
}

impl SorceryWizard for ConfigSorcery {
    fn name(&self) -> &str {
        "config"
    }

    fn create(&self, _object: &SorceryObject) -> SorceryResult<()> {
        Err(SorceryError::Other(
            "config wizard is read-only".to_string(),
        ))
    }

    fn retrieve_id(&self, object_type: &str, id: &str) -> SorceryResult<SorceryObject> {
        let key = MemorySorcery::make_key(object_type, id);
        self.objects
            .read()
            .get(&key)
            .cloned()
            .ok_or_else(|| SorceryError::NotFound(object_type.to_string(), id.to_string()))
    }

    fn retrieve_fields(
        &self,
        object_type: &str,
        fields: &[(&str, &str)],
    ) -> SorceryResult<SorceryObject> {
        self.objects
            .read()
            .values()
            .find(|obj| obj.object_type == object_type && obj.matches(fields))
            .cloned()
            .ok_or_else(|| {
                SorceryError::NotFound(object_type.to_string(), format!("fields={:?}", fields))
            })
    }

    fn retrieve_multiple(
        &self,
        object_type: &str,
        fields: &[(&str, &str)],
    ) -> SorceryResult<Vec<SorceryObject>> {
        let results: Vec<SorceryObject> = self
            .objects
            .read()
            .values()
            .filter(|obj| {
                obj.object_type == object_type
                    && (fields.is_empty() || obj.matches(fields))
            })
            .cloned()
            .collect();
        Ok(results)
    }

    fn retrieve_prefix(
        &self,
        object_type: &str,
        prefix: &str,
    ) -> SorceryResult<Vec<SorceryObject>> {
        let results: Vec<SorceryObject> = self
            .objects
            .read()
            .values()
            .filter(|obj| {
                obj.object_type == object_type && obj.id_matches_prefix(prefix)
            })
            .cloned()
            .collect();
        Ok(results)
    }

    fn update(&self, _object: &SorceryObject) -> SorceryResult<()> {
        Err(SorceryError::Other(
            "config wizard is read-only".to_string(),
        ))
    }

    fn delete(&self, _object: &SorceryObject) -> SorceryResult<()> {
        Err(SorceryError::Other(
            "config wizard is read-only".to_string(),
        ))
    }
}

// ---------------------------------------------------------------------------
// Realtime wizard (res_sorcery_realtime.c) -- delegates to realtime drivers
// ---------------------------------------------------------------------------

/// Realtime sorcery wizard.
///
/// Port of `res_sorcery_realtime.c`. Delegates CRUD operations to whichever
/// realtime configuration driver is configured (ODBC, PostgreSQL, etc.).
/// The `family` is the realtime family name used for engine lookup.
#[derive(Debug)]
pub struct RealtimeSorcery {
    /// Realtime family name (e.g., from sorcery.conf mapping).
    pub family: String,
    /// ID field name in the realtime table.
    pub id_field: String,
}

impl RealtimeSorcery {
    pub fn new(family: &str) -> Self {
        Self {
            family: family.to_string(),
            id_field: "id".to_string(),
        }
    }
}

impl SorceryWizard for RealtimeSorcery {
    fn name(&self) -> &str {
        "realtime"
    }

    fn create(&self, object: &SorceryObject) -> SorceryResult<()> {
        debug!(
            family = %self.family,
            id = %object.id,
            "Realtime sorcery create (stub)"
        );
        Err(SorceryError::WizardError(
            "realtime driver not connected".to_string(),
        ))
    }

    fn retrieve_id(&self, object_type: &str, id: &str) -> SorceryResult<SorceryObject> {
        debug!(
            family = %self.family,
            object_type = object_type,
            id = id,
            "Realtime sorcery retrieve_id (stub)"
        );
        Err(SorceryError::WizardError(
            "realtime driver not connected".to_string(),
        ))
    }

    fn retrieve_fields(
        &self,
        object_type: &str,
        _fields: &[(&str, &str)],
    ) -> SorceryResult<SorceryObject> {
        debug!(
            family = %self.family,
            object_type = object_type,
            "Realtime sorcery retrieve_fields (stub)"
        );
        Err(SorceryError::WizardError(
            "realtime driver not connected".to_string(),
        ))
    }

    fn retrieve_multiple(
        &self,
        object_type: &str,
        _fields: &[(&str, &str)],
    ) -> SorceryResult<Vec<SorceryObject>> {
        debug!(
            family = %self.family,
            object_type = object_type,
            "Realtime sorcery retrieve_multiple (stub)"
        );
        Ok(Vec::new())
    }

    fn retrieve_prefix(
        &self,
        object_type: &str,
        prefix: &str,
    ) -> SorceryResult<Vec<SorceryObject>> {
        debug!(
            family = %self.family,
            object_type = object_type,
            prefix = prefix,
            "Realtime sorcery retrieve_prefix (stub)"
        );
        Ok(Vec::new())
    }

    fn update(&self, object: &SorceryObject) -> SorceryResult<()> {
        debug!(
            family = %self.family,
            id = %object.id,
            "Realtime sorcery update (stub)"
        );
        Err(SorceryError::WizardError(
            "realtime driver not connected".to_string(),
        ))
    }

    fn delete(&self, object: &SorceryObject) -> SorceryResult<()> {
        debug!(
            family = %self.family,
            id = %object.id,
            "Realtime sorcery delete (stub)"
        );
        Err(SorceryError::WizardError(
            "realtime driver not connected".to_string(),
        ))
    }
}

// ---------------------------------------------------------------------------
// AstDB wizard (res_sorcery_astdb.c) -- persists to internal database
// ---------------------------------------------------------------------------

/// AstDB sorcery wizard.
///
/// Port of `res_sorcery_astdb.c`. Persists sorcery objects to the
/// Asterisk internal database (AstDB) using family/key conventions.
/// Objects are serialized as semicolon-delimited field pairs.
#[derive(Debug)]
pub struct AstDbSorcery {
    /// AstDB family prefix for stored objects.
    pub family_prefix: String,
}

impl AstDbSorcery {
    pub fn new(family_prefix: &str) -> Self {
        Self {
            family_prefix: family_prefix.to_string(),
        }
    }

    /// Build the AstDB family name for an object type.
    pub fn db_family(&self, object_type: &str) -> String {
        if self.family_prefix.is_empty() {
            object_type.to_string()
        } else {
            format!("{}/{}", self.family_prefix, object_type)
        }
    }

    /// Serialize a sorcery object to an AstDB value string.
    pub fn serialize(object: &SorceryObject) -> String {
        let mut parts = Vec::new();
        for (key, value) in &object.fields {
            // Escape semicolons in values
            let escaped_value = value.replace(';', "\\;");
            parts.push(format!("{}={}", key, escaped_value));
        }
        parts.join(";")
    }

    /// Deserialize a sorcery object from an AstDB value string.
    pub fn deserialize(object_type: &str, id: &str, data: &str) -> SorceryObject {
        let mut obj = SorceryObject::new(object_type, id);
        for pair in data.split(';') {
            if let Some(eq_pos) = pair.find('=') {
                let key = &pair[..eq_pos];
                let value = pair[eq_pos + 1..].replace("\\;", ";");
                if !key.is_empty() {
                    obj.set_field(key, &value);
                }
            }
        }
        obj
    }
}

impl SorceryWizard for AstDbSorcery {
    fn name(&self) -> &str {
        "astdb"
    }

    fn create(&self, object: &SorceryObject) -> SorceryResult<()> {
        let _family = self.db_family(&object.object_type);
        let _value = Self::serialize(object);
        debug!(
            family = %self.family_prefix,
            id = %object.id,
            "AstDB sorcery create (stub - needs AstDB integration)"
        );
        Err(SorceryError::WizardError(
            "AstDB not connected".to_string(),
        ))
    }

    fn retrieve_id(&self, object_type: &str, id: &str) -> SorceryResult<SorceryObject> {
        debug!(
            family = %self.db_family(object_type),
            id = id,
            "AstDB sorcery retrieve_id (stub)"
        );
        Err(SorceryError::WizardError(
            "AstDB not connected".to_string(),
        ))
    }

    fn retrieve_fields(
        &self,
        _object_type: &str,
        _fields: &[(&str, &str)],
    ) -> SorceryResult<SorceryObject> {
        Err(SorceryError::WizardError(
            "AstDB not connected".to_string(),
        ))
    }

    fn retrieve_multiple(
        &self,
        _object_type: &str,
        _fields: &[(&str, &str)],
    ) -> SorceryResult<Vec<SorceryObject>> {
        Ok(Vec::new())
    }

    fn retrieve_prefix(
        &self,
        _object_type: &str,
        _prefix: &str,
    ) -> SorceryResult<Vec<SorceryObject>> {
        Ok(Vec::new())
    }

    fn update(&self, object: &SorceryObject) -> SorceryResult<()> {
        debug!(
            family = %self.family_prefix,
            id = %object.id,
            "AstDB sorcery update (stub)"
        );
        Err(SorceryError::WizardError(
            "AstDB not connected".to_string(),
        ))
    }

    fn delete(&self, object: &SorceryObject) -> SorceryResult<()> {
        debug!(
            family = %self.family_prefix,
            id = %object.id,
            "AstDB sorcery delete (stub)"
        );
        Err(SorceryError::WizardError(
            "AstDB not connected".to_string(),
        ))
    }
}

// ---------------------------------------------------------------------------
// Sorcery instance (coordinator)
// ---------------------------------------------------------------------------

/// A sorcery instance that coordinates wizards for object types.
///
/// Each object type can have one or more wizards applied in order.
/// On retrieval, wizards are tried in priority order until one returns data.
pub struct SorceryInstance {
    /// Name of this sorcery instance.
    pub name: String,
    /// Wizards applied to object types. Key is object type, value is ordered wizard list.
    wizards: RwLock<HashMap<String, Vec<Arc<dyn SorceryWizard>>>>,
}

impl SorceryInstance {
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            wizards: RwLock::new(HashMap::new()),
        }
    }

    /// Apply a wizard to an object type.
    pub fn apply_wizard(
        &self,
        object_type: &str,
        wizard: Arc<dyn SorceryWizard>,
    ) {
        self.wizards
            .write()
            .entry(object_type.to_string())
            .or_default()
            .push(wizard);
    }

    /// Get wizards for an object type.
    fn get_wizards(&self, object_type: &str) -> Vec<Arc<dyn SorceryWizard>> {
        self.wizards
            .read()
            .get(object_type)
            .cloned()
            .unwrap_or_default()
    }

    /// Create an object using the first writable wizard.
    pub fn create(&self, object: &SorceryObject) -> SorceryResult<()> {
        for wizard in self.get_wizards(&object.object_type) {
            match wizard.create(object) {
                Ok(()) => return Ok(()),
                Err(SorceryError::Other(msg)) if msg.contains("read-only") => continue,
                Err(e) => {
                    warn!(wizard = wizard.name(), error = %e, "Sorcery create failed");
                    continue;
                }
            }
        }
        Err(SorceryError::WizardNotFound(format!(
            "No writable wizard for type '{}'",
            object.object_type
        )))
    }

    /// Retrieve an object by ID from the first wizard that has it.
    pub fn retrieve_id(&self, object_type: &str, id: &str) -> SorceryResult<SorceryObject> {
        for wizard in self.get_wizards(object_type) {
            match wizard.retrieve_id(object_type, id) {
                Ok(obj) => return Ok(obj),
                Err(_) => continue,
            }
        }
        Err(SorceryError::NotFound(
            object_type.to_string(),
            id.to_string(),
        ))
    }

    /// Retrieve an object by field criteria.
    pub fn retrieve_fields(
        &self,
        object_type: &str,
        fields: &[(&str, &str)],
    ) -> SorceryResult<SorceryObject> {
        for wizard in self.get_wizards(object_type) {
            match wizard.retrieve_fields(object_type, fields) {
                Ok(obj) => return Ok(obj),
                Err(_) => continue,
            }
        }
        Err(SorceryError::NotFound(
            object_type.to_string(),
            format!("{:?}", fields),
        ))
    }

    /// Retrieve multiple objects, collecting from all wizards.
    pub fn retrieve_multiple(
        &self,
        object_type: &str,
        fields: &[(&str, &str)],
    ) -> Vec<SorceryObject> {
        let mut results = Vec::new();
        for wizard in self.get_wizards(object_type) {
            if let Ok(mut objs) = wizard.retrieve_multiple(object_type, fields) {
                results.append(&mut objs);
            }
        }
        results
    }

    /// Update an object using the first writable wizard.
    pub fn update(&self, object: &SorceryObject) -> SorceryResult<()> {
        for wizard in self.get_wizards(&object.object_type) {
            match wizard.update(object) {
                Ok(()) => return Ok(()),
                Err(SorceryError::Other(msg)) if msg.contains("read-only") => continue,
                Err(_) => continue,
            }
        }
        Err(SorceryError::WizardNotFound(format!(
            "No writable wizard for type '{}'",
            object.object_type
        )))
    }

    /// Delete an object using the first writable wizard.
    pub fn delete(&self, object: &SorceryObject) -> SorceryResult<()> {
        for wizard in self.get_wizards(&object.object_type) {
            match wizard.delete(object) {
                Ok(()) => return Ok(()),
                Err(SorceryError::Other(msg)) if msg.contains("read-only") => continue,
                Err(_) => continue,
            }
        }
        Err(SorceryError::WizardNotFound(format!(
            "No writable wizard for type '{}'",
            object.object_type
        )))
    }
}

impl fmt::Debug for SorceryInstance {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SorceryInstance")
            .field("name", &self.name)
            .field("types", &self.wizards.read().len())
            .finish()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sorcery_object() {
        let mut obj = SorceryObject::new("endpoint", "alice");
        obj.set_field("context", "default");
        obj.set_field("transport", "udp");

        assert_eq!(obj.id, "alice");
        assert_eq!(obj.get_field("context"), Some("default"));
        assert!(obj.matches(&[("context", "default")]));
        assert!(!obj.matches(&[("context", "wrong")]));
    }

    #[test]
    fn test_memory_sorcery_crud() {
        let wizard = MemorySorcery::new();

        let mut obj = SorceryObject::new("endpoint", "alice");
        obj.set_field("context", "default");

        // Create
        wizard.create(&obj).unwrap();
        assert_eq!(wizard.count(), 1);

        // Retrieve by ID
        let retrieved = wizard.retrieve_id("endpoint", "alice").unwrap();
        assert_eq!(retrieved.get_field("context"), Some("default"));

        // Update
        obj.set_field("context", "internal");
        wizard.update(&obj).unwrap();
        let updated = wizard.retrieve_id("endpoint", "alice").unwrap();
        assert_eq!(updated.get_field("context"), Some("internal"));

        // Delete
        wizard.delete(&obj).unwrap();
        assert!(wizard.retrieve_id("endpoint", "alice").is_err());
    }

    #[test]
    fn test_memory_sorcery_retrieve_fields() {
        let wizard = MemorySorcery::new();

        let mut obj1 = SorceryObject::new("endpoint", "alice");
        obj1.set_field("context", "default");
        wizard.create(&obj1).unwrap();

        let mut obj2 = SorceryObject::new("endpoint", "bob");
        obj2.set_field("context", "internal");
        wizard.create(&obj2).unwrap();

        let result = wizard
            .retrieve_fields("endpoint", &[("context", "internal")])
            .unwrap();
        assert_eq!(result.id, "bob");
    }

    #[test]
    fn test_memory_sorcery_retrieve_multiple() {
        let wizard = MemorySorcery::new();

        let mut obj1 = SorceryObject::new("aor", "alice");
        obj1.set_field("max_contacts", "5");
        wizard.create(&obj1).unwrap();

        let mut obj2 = SorceryObject::new("aor", "bob");
        obj2.set_field("max_contacts", "5");
        wizard.create(&obj2).unwrap();

        let results = wizard
            .retrieve_multiple("aor", &[("max_contacts", "5")])
            .unwrap();
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn test_memory_sorcery_duplicate() {
        let wizard = MemorySorcery::new();
        let obj = SorceryObject::new("endpoint", "alice");
        wizard.create(&obj).unwrap();
        assert!(matches!(
            wizard.create(&obj),
            Err(SorceryError::AlreadyExists(..))
        ));
    }

    #[test]
    fn test_config_sorcery_read_only() {
        let wizard = ConfigSorcery::new("pjsip.conf");
        let obj = SorceryObject::new("endpoint", "alice");
        assert!(wizard.create(&obj).is_err());
        assert!(wizard.update(&obj).is_err());
        assert!(wizard.delete(&obj).is_err());
    }

    #[test]
    fn test_config_sorcery_load() {
        let wizard = ConfigSorcery::new("pjsip.conf");
        wizard.load_from_sections(
            "endpoint",
            &[
                (
                    "alice".to_string(),
                    vec![("context".to_string(), "default".to_string())],
                ),
                (
                    "bob".to_string(),
                    vec![("context".to_string(), "internal".to_string())],
                ),
            ],
        );

        let obj = wizard.retrieve_id("endpoint", "alice").unwrap();
        assert_eq!(obj.get_field("context"), Some("default"));

        let all = wizard.retrieve_multiple("endpoint", &[]).unwrap();
        assert_eq!(all.len(), 2);
    }

    #[test]
    fn test_astdb_sorcery_serialize_deserialize() {
        let mut obj = SorceryObject::new("mailbox", "100@default");
        obj.set_field("msgs_new", "3");
        obj.set_field("msgs_old", "5");

        let serialized = AstDbSorcery::serialize(&obj);
        assert!(serialized.contains("msgs_new=3"));

        let deserialized = AstDbSorcery::deserialize("mailbox", "100@default", &serialized);
        assert_eq!(deserialized.get_field("msgs_new"), Some("3"));
        assert_eq!(deserialized.get_field("msgs_old"), Some("5"));
    }

    #[test]
    fn test_astdb_sorcery_db_family() {
        let wizard = AstDbSorcery::new("sorcery");
        assert_eq!(wizard.db_family("endpoint"), "sorcery/endpoint");

        let wizard2 = AstDbSorcery::new("");
        assert_eq!(wizard2.db_family("endpoint"), "endpoint");
    }

    #[test]
    fn test_sorcery_instance() {
        let instance = SorceryInstance::new("pjsip");
        let memory = Arc::new(MemorySorcery::new());
        instance.apply_wizard("endpoint", memory.clone());

        let mut obj = SorceryObject::new("endpoint", "alice");
        obj.set_field("context", "default");

        instance.create(&obj).unwrap();
        let retrieved = instance.retrieve_id("endpoint", "alice").unwrap();
        assert_eq!(retrieved.get_field("context"), Some("default"));
    }
}
