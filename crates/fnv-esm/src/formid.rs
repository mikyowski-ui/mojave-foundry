//! Form ids and their resolution against a plugin's master list.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::error::EsmError;

/// A form id as stored in a plugin.
///
/// The top byte is an index into the plugin's master list; the low 24 bits
/// are the id local to the plugin that owns the form. An index equal to the
/// number of masters means the plugin itself.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Serialize, Deserialize)]
#[serde(transparent)]
pub struct FormId(pub u32);

impl FormId {
    /// The null form id, used by the format for "no form".
    pub const NULL: FormId = FormId(0);

    /// The top byte: an index into the plugin's master list.
    pub const fn master_index(self) -> u8 {
        (self.0 >> 24) as u8
    }

    /// The low 24 bits: the id local to the owning plugin.
    pub const fn local_id(self) -> u32 {
        self.0 & 0x00FF_FFFF
    }

    /// Whether this is the null form id.
    pub const fn is_null(self) -> bool {
        self.0 == 0
    }
}

impl From<u32> for FormId {
    fn from(value: u32) -> Self {
        Self(value)
    }
}

impl fmt::Display for FormId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:08X}", self.0)
    }
}

impl fmt::Debug for FormId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "FormId({:08X})", self.0)
    }
}

/// A form id resolved to the plugin that owns the form.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ResolvedFormId<'a> {
    /// File name of the owning plugin, lowercase.
    pub plugin: &'a str,
    /// The 24-bit id local to that plugin.
    pub local_id: u32,
}

/// Shown as `plugin:XXXXXX`, the local id as six hex digits.
impl fmt::Display for ResolvedFormId<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{:06X}", self.plugin, self.local_id)
    }
}

/// A plugin's master list plus the plugin's own name, for resolving form
/// ids.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MasterList {
    /// Masters in file order, then the plugin itself; all lowercase.
    names: Vec<String>,
}

impl MasterList {
    /// Builds the list from the plugin's own file name and its masters in
    /// file order. Names are lowercased.
    pub fn new<S: AsRef<str>>(plugin_name: &str, masters: &[S]) -> Self {
        let mut names: Vec<String> = masters.iter().map(|m| m.as_ref().to_lowercase()).collect();
        names.push(plugin_name.to_lowercase());
        Self { names }
    }

    /// Number of masters, not counting the plugin itself.
    pub fn master_count(&self) -> usize {
        self.names.len() - 1
    }

    /// The masters in file order, lowercase.
    pub fn masters(&self) -> &[String] {
        &self.names[..self.names.len() - 1]
    }

    /// The plugin's own name, lowercase.
    pub fn plugin_name(&self) -> &str {
        &self.names[self.names.len() - 1]
    }

    /// Resolves a form id to its owning plugin and local id.
    ///
    /// The null form id resolves like any other (to local id 0 of the first
    /// master, or of the plugin when it has no masters); test
    /// [`FormId::is_null`] first where "no form" matters.
    ///
    /// # Errors
    ///
    /// [`EsmError::MasterIndexOutOfRange`] when the top byte is greater than
    /// the number of masters.
    pub fn resolve(&self, form_id: FormId) -> Result<ResolvedFormId<'_>, EsmError> {
        let index = form_id.master_index();
        match self.names.get(usize::from(index)) {
            Some(name) => Ok(ResolvedFormId {
                plugin: name,
                local_id: form_id.local_id(),
            }),
            None => Err(EsmError::MasterIndexOutOfRange {
                form_id,
                index,
                master_count: self.master_count(),
            }),
        }
    }
}
