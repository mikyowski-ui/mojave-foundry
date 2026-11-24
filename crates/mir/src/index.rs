//! The plugins of an extract, held in memory, and an index of every record
//! in them by form.
//!
//! A record is indexed as a header and a borrowed payload; nothing is
//! decompressed or decoded until someone asks. When two plugins define the
//! same form, the later one in the list wins, as it does in the game.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use fnv_esm::strings::decode_zstring;
use fnv_esm::{
    CellChildKind, Context, Event, FormId, Plugin, Record, RecordRef, Sig, SubrecordRef, TopItem,
    Walker,
};
use sha2::{Digest, Sha256};

use crate::error::MirError;
use crate::paths::FormKey;

/// Lowercase hex SHA-256 of `bytes`: the content hash of
/// `docs/contracts.md`, section 2.
pub fn content_hash(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(64);
    for byte in digest {
        out.push(char::from_digit(u32::from(byte >> 4), 16).unwrap_or('0'));
        out.push(char::from_digit(u32::from(byte & 0xF), 16).unwrap_or('0'));
    }
    out
}

/// One plugin file, read whole.
pub(crate) struct PluginFile {
    /// File name, lowercase.
    pub name: String,
    pub bytes: Vec<u8>,
    pub sha256: String,
}

/// Finds a file in `dir` by name, exactly or ignoring ASCII case.
fn find_file(dir: &Path, name: &str) -> PathBuf {
    let exact = dir.join(name);
    if exact.is_file() {
        return exact;
    }
    let mut matches: Vec<PathBuf> = fs::read_dir(dir)
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter(|e| {
            e.file_name()
                .to_str()
                .is_some_and(|n| n.eq_ignore_ascii_case(name))
        })
        .map(|e| e.path())
        .collect();
    matches.sort();
    matches.into_iter().next().unwrap_or(exact)
}

/// Reads the plugins of the list, in list order.
pub(crate) fn load_plugins(data_dir: &Path, names: &[String]) -> Result<Vec<PluginFile>, MirError> {
    let mut files = Vec::with_capacity(names.len());
    for name in names {
        let path = find_file(data_dir, name);
        let bytes = fs::read(&path).map_err(|e| MirError::io(&path, e))?;
        let sha256 = content_hash(&bytes);
        files.push(PluginFile {
            name: name.to_lowercase(),
            bytes,
            sha256,
        });
    }
    Ok(files)
}

/// A form's identity inside one [`Index`]: the index of the owning
/// plugin's name in the sorted name table, and the local id. Orders like
/// [`FormKey`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct Key(u64);

impl Key {
    fn new(name: u32, local_id: u32) -> Self {
        Self((u64::from(name) << 32) | u64::from(local_id & 0x00FF_FFFF))
    }

    fn name(self) -> usize {
        (self.0 >> 32) as usize
    }

    fn local_id(self) -> u32 {
        self.0 as u32
    }
}

/// One record of the index.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Entry<'a> {
    pub key: Key,
    /// Index of the plugin the record was read from, in list order.
    pub plugin: u16,
    pub record: RecordRef<'a>,
    /// The worldspace whose children group encloses the record.
    pub world: Option<Key>,
    /// The cell whose children group encloses the record.
    pub cell: Option<Key>,
    /// The topic whose children group encloses the record.
    pub topic: Option<Key>,
    /// Which of a cell's children groups the record is in.
    pub child: Option<CellChildKind>,
    /// Whether an exterior cell block encloses the record. A `CELL` inside
    /// a worldspace but outside every block is the persistent cell.
    pub in_block: bool,
}

impl Entry<'_> {
    pub fn sig(&self) -> Sig {
        self.record.sig()
    }
}

/// A record that could not be used.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BadRecord {
    /// The plugin's file name, lowercase.
    pub plugin: String,
    /// Byte offset of the record (or group) header in the plugin.
    pub offset: u64,
    pub sig: String,
    pub form: Option<FormKey>,
    pub error: String,
}

/// The fields every record may have, read without a typed decoder.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct CommonScan {
    pub editor_id: Option<String>,
    pub name: Option<String>,
    pub model: Option<String>,
    pub script: Option<FormId>,
    pub bounds: Option<[i16; 6]>,
}

/// Reads the first `EDID`, `FULL`, `MODL`, `SCRI` and `OBND` of a record.
pub(crate) fn scan_common(subrecords: &[SubrecordRef<'_>]) -> CommonScan {
    let mut out = CommonScan::default();
    for sub in subrecords {
        match &sub.sig.0 {
            b"EDID" if out.editor_id.is_none() => out.editor_id = Some(decode_zstring(sub.data)),
            b"FULL" if out.name.is_none() => out.name = Some(decode_zstring(sub.data)),
            b"MODL" if out.model.is_none() => out.model = Some(decode_zstring(sub.data)),
            b"SCRI" if out.script.is_none() => {
                if let Ok(bytes) = <[u8; 4]>::try_from(sub.data) {
                    out.script = Some(FormId(u32::from_le_bytes(bytes)));
                }
            }
            b"OBND" if out.bounds.is_none() && sub.data.len() == 12 => {
                let mut bounds = [0i16; 6];
                for (value, pair) in bounds.iter_mut().zip(sub.data.as_chunks::<2>().0) {
                    *value = i16::from_le_bytes([pair[0], pair[1]]);
                }
                out.bounds = Some(bounds);
            }
            _ => {}
        }
    }
    out
}

/// What the extractor needs to know about a base form.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Summary {
    pub sig: String,
    pub editor_id: Option<String>,
    pub model: Option<String>,
}

struct PluginCtx {
    name: String,
    /// Master index → name table index; the plugin itself is last.
    masters: Vec<u32>,
}

/// Every record of the plugin list, by form.
pub(crate) struct Index<'a> {
    /// Every plugin name met, as a plugin or as a master: lowercase,
    /// sorted.
    names: Vec<String>,
    plugins: Vec<PluginCtx>,
    /// Sorted by key, one entry per form.
    entries: Vec<Entry<'a>>,
    /// Framing errors met while indexing.
    pub walk_errors: Vec<BadRecord>,
    /// Records whose own form id names a master index past the plugin's
    /// master list. The engine reads such an id as the plugin's own, and
    /// so does the index; they are reported as notes, not errors.
    pub remapped: Vec<BadRecord>,
}

impl<'a> Index<'a> {
    /// Indexes the plugins.
    ///
    /// # Errors
    ///
    /// [`MirError::Plugin`] when a plugin's header does not parse. Framing
    /// errors further in are collected in [`Index::walk_errors`]: they cost
    /// the rest of the top-level group they are in, nothing more.
    pub fn build(files: &'a [PluginFile]) -> Result<Self, MirError> {
        let mut parsed: Vec<Plugin<'a>> = Vec::with_capacity(files.len());
        let mut names = BTreeSet::new();
        for file in files {
            let plugin = Plugin::parse(&file.bytes).map_err(|source| MirError::Plugin {
                plugin: file.name.clone(),
                source,
            })?;
            names.insert(file.name.clone());
            names.extend(plugin.header().masters.iter().map(|m| m.to_lowercase()));
            parsed.push(plugin);
        }
        let names: Vec<String> = names.into_iter().collect();
        let name_index = |name: &str| names.binary_search_by(|n| n.as_str().cmp(name)).ok();

        let mut plugins = Vec::with_capacity(files.len());
        for (file, plugin) in files.iter().zip(&parsed) {
            let masters = plugin
                .master_list(&file.name)
                .masters()
                .iter()
                .map(String::as_str)
                .chain([file.name.as_str()])
                .filter_map(name_index)
                .map(|i| i as u32)
                .collect();
            plugins.push(PluginCtx {
                name: file.name.clone(),
                masters,
            });
        }

        let mut index = Self {
            names,
            plugins,
            entries: Vec::new(),
            walk_errors: Vec::new(),
            remapped: Vec::new(),
        };
        for (number, plugin) in parsed.iter().enumerate() {
            let number = u16::try_from(number).unwrap_or(u16::MAX);
            for item in plugin.top_level() {
                match item {
                    Ok(TopItem::Group(group)) => {
                        let mut walker = group.walk();
                        index.walk_group(number, &mut walker, group.header.offset);
                    }
                    Ok(TopItem::Record(record)) => {
                        index.add(number, record, &Context::default());
                    }
                    Err(error) => index.walk_error(number, 0, &error),
                }
            }
        }

        // The last plugin of the list wins a form.
        index
            .entries
            .sort_by_key(|e| (e.key, std::cmp::Reverse(e.plugin)));
        index.entries.dedup_by_key(|e| e.key);
        Ok(index)
    }

    fn walk_group(&mut self, plugin: u16, walker: &mut Walker<'a>, group_offset: usize) {
        loop {
            match walker.next_event() {
                Ok(Some(Event::Record(record))) => {
                    self.add(plugin, record, walker.context());
                }
                Ok(Some(_)) => {}
                Ok(None) => break,
                Err(error) => {
                    self.walk_error(plugin, group_offset, &error);
                    break;
                }
            }
        }
    }

    fn walk_error(&mut self, plugin: u16, fallback_offset: usize, error: &fnv_esm::EsmError) {
        self.walk_errors.push(BadRecord {
            plugin: self.plugin_name(plugin).to_owned(),
            offset: error.offset().unwrap_or(fallback_offset) as u64,
            sig: error.sig().map(|s| s.to_string()).unwrap_or_default(),
            form: None,
            error: format!("the rest of the enclosing top-level group is lost: {error}"),
        });
    }

    fn add(&mut self, plugin: u16, record: RecordRef<'a>, context: &Context) {
        let key = match self.resolve_any(plugin, record.form_id()) {
            Some(key) => Some(key),
            None => self.own_key(plugin, record.form_id()).inspect(|&key| {
                let masters = self
                    .plugins
                    .get(usize::from(plugin))
                    .map_or(0, |p| p.masters.len().saturating_sub(1));
                self.remapped.push(BadRecord {
                    plugin: self.plugin_name(plugin).to_owned(),
                    offset: record.offset as u64,
                    sig: record.sig().to_string(),
                    form: Some(self.form_key(key)),
                    error: format!(
                        "form id {} names master index {}, past the plugin's {masters} masters; \
                         it is read as the plugin's own form, as the engine reads it",
                        record.form_id(),
                        record.form_id().master_index()
                    ),
                });
            }),
        };
        let Some(key) = key else {
            self.walk_errors.push(BadRecord {
                plugin: self.plugin_name(plugin).to_owned(),
                offset: record.offset as u64,
                sig: record.sig().to_string(),
                form: None,
                error: format!(
                    "form id {} names a master the plugin does not list",
                    record.form_id()
                ),
            });
            return;
        };
        let resolve = |form: Option<FormId>| form.and_then(|f| self.resolve(plugin, f));
        let entry = Entry {
            key,
            plugin,
            record,
            world: resolve(context.worldspace()),
            cell: resolve(context.cell()),
            topic: resolve(context.topic()),
            child: context.cell_child_kind(),
            in_block: context.exterior_block().is_some(),
        };
        self.entries.push(entry);
    }

    /// The file name of a plugin of the list, lowercase.
    pub fn plugin_name(&self, plugin: u16) -> &str {
        self.plugins
            .get(usize::from(plugin))
            .map_or("", |p| p.name.as_str())
    }

    /// Every record, sorted by form.
    pub fn entries(&self) -> &[Entry<'a>] {
        &self.entries
    }

    /// The key of a form id taken as the plugin's own, whatever its master
    /// index says.
    fn own_key(&self, plugin: u16, form: FormId) -> Option<Key> {
        let ctx = self.plugins.get(usize::from(plugin))?;
        let name = *ctx.masters.last()?;
        Some(Key::new(name, form.local_id()))
    }

    fn resolve_any(&self, plugin: u16, form: FormId) -> Option<Key> {
        let ctx = self.plugins.get(usize::from(plugin))?;
        let name = *ctx.masters.get(usize::from(form.master_index()))?;
        Some(Key::new(name, form.local_id()))
    }

    /// Resolves a form id stored in a record of `plugin`. The null id and
    /// an id that names a master the plugin does not list give `None`.
    pub fn resolve(&self, plugin: u16, form: FormId) -> Option<Key> {
        if form.is_null() {
            return None;
        }
        self.resolve_any(plugin, form)
    }

    /// The public identity of a key.
    pub fn form_key(&self, key: Key) -> FormKey {
        FormKey {
            plugin: self.names.get(key.name()).cloned().unwrap_or_default(),
            local_id: key.local_id(),
        }
    }

    /// The key of a public form identity, when the plugin is known.
    pub fn key(&self, form: &FormKey) -> Option<Key> {
        let name = self
            .names
            .binary_search_by(|n| n.as_str().cmp(form.plugin.as_str()))
            .ok()?;
        Some(Key::new(u32::try_from(name).ok()?, form.local_id))
    }

    /// Resolves and formats in one step.
    pub fn form(&self, plugin: u16, form: FormId) -> Option<FormKey> {
        self.resolve(plugin, form).map(|key| self.form_key(key))
    }

    /// The record of a form, if a plugin of the list defines it.
    pub fn get(&self, key: Key) -> Option<&Entry<'a>> {
        self.entries
            .binary_search_by_key(&key, |e| e.key)
            .ok()
            .map(|i| &self.entries[i])
    }

    /// Decodes a record through the typed decoders.
    pub fn decode(&self, entry: &Entry<'a>) -> Result<Record, BadRecord> {
        entry
            .record
            .decode()
            .map_err(|error| self.bad(entry, error.to_string()))
    }

    /// A [`BadRecord`] for an entry.
    pub fn bad(&self, entry: &Entry<'a>, error: String) -> BadRecord {
        BadRecord {
            plugin: self.plugin_name(entry.plugin).to_owned(),
            offset: entry.record.offset as u64,
            sig: entry.sig().to_string(),
            form: Some(self.form_key(entry.key)),
            error,
        }
    }

    /// Signature, editor id and model of a form, read without a typed
    /// decoder so that it works for every record type. `None` when the
    /// form is not in the plugin list; a record that does not read gives
    /// its signature alone.
    pub fn summary(&self, key: Key) -> Option<Summary> {
        let entry = self.get(key)?;
        let mut summary = Summary {
            sig: entry.sig().to_string(),
            ..Summary::default()
        };
        if let Ok(data) = entry.record.data()
            && let Ok(subrecords) = data.collect()
        {
            let common = scan_common(&subrecords);
            summary.editor_id = common.editor_id;
            summary.model = common.model;
        }
        Some(summary)
    }
}
