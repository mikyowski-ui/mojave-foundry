//! What names can mean outside one script: every editor id of the plugin
//! list, the variables of every script, and which script each quest and
//! placed reference runs. The build fills it from the MIR; tests fill it
//! by hand.

use std::collections::{BTreeMap, HashMap};

use crate::ir::{FormKey, VarDecl};

/// A form an editor id names, with its record signature.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct NamedForm {
    /// The form.
    pub form: FormKey,
    /// Its signature: `QUST`, `GLOB`, `REFR`, `ACHR`, `NPC_`, ...
    pub sig: String,
}

impl NamedForm {
    /// A placed reference: an object, an NPC or a creature reference.
    pub fn is_placed(&self) -> bool {
        is_placed_sig(&self.sig)
    }
}

/// Whether a signature is a placed reference's.
pub fn is_placed_sig(sig: &str) -> bool {
    matches!(sig, "REFR" | "ACHR" | "ACRE" | "PGRE" | "PMIS" | "PBEA")
}

/// The names of the plugin list.
#[derive(Debug, Clone, Default)]
pub struct NameTable {
    forms: HashMap<String, Vec<NamedForm>>,
    sigs: HashMap<FormKey, String>,
    script_vars: BTreeMap<FormKey, Vec<VarDecl>>,
    quest_scripts: BTreeMap<FormKey, FormKey>,
    ref_bases: BTreeMap<FormKey, FormKey>,
    base_scripts: BTreeMap<FormKey, FormKey>,
    unique_refs: BTreeMap<FormKey, FormKey>,
    form_lists: BTreeMap<FormKey, Vec<FormKey>>,
    plugins: Vec<String>,
}

impl NameTable {
    /// An empty table.
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a form under its editor id.
    pub fn add_form(&mut self, editor_id: &str, form: FormKey, sig: &str) {
        let entry = self
            .forms
            .entry(editor_id.to_ascii_lowercase())
            .or_default();
        let named = NamedForm {
            form: form.clone(),
            sig: sig.to_owned(),
        };
        if let Err(at) = entry.binary_search(&named) {
            entry.insert(at, named);
        }
        self.sigs.insert(form, sig.to_owned());
    }

    /// Records a script's variables.
    pub fn set_script_vars(&mut self, script: FormKey, vars: Vec<VarDecl>) {
        self.script_vars.insert(script, vars);
    }

    /// Records a quest's script.
    pub fn set_quest_script(&mut self, quest: FormKey, script: FormKey) {
        self.quest_scripts.insert(quest, script);
    }

    /// Records a placed reference's base.
    pub fn set_ref_base(&mut self, reference: FormKey, base: FormKey) {
        self.ref_bases.insert(reference, base);
    }

    /// Records a base's script.
    pub fn set_base_script(&mut self, base: FormKey, script: FormKey) {
        self.base_scripts.insert(base, script);
    }

    /// Records the only placed reference of an actor base.
    pub fn set_unique_ref(&mut self, base: FormKey, reference: FormKey) {
        self.unique_refs.insert(base, reference);
    }

    /// The only placed reference of an actor base, when it has exactly
    /// one.
    pub fn unique_ref(&self, base: &FormKey) -> Option<&FormKey> {
        self.unique_refs.get(base)
    }

    /// Records the load order, for raw form ids written in a script
    /// (`00000F`): the high byte indexes it.
    pub fn set_plugins(&mut self, plugins: Vec<String>) {
        self.plugins = plugins;
    }

    /// The form a raw form id written in a script names: eight hex digits,
    /// the high byte a load index.
    pub fn raw_form(&self, text: &str) -> Option<NamedForm> {
        if text.len() != 8 || !text.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        let raw = u32::from_str_radix(text, 16).ok()?;
        let plugin = self.plugins.get((raw >> 24) as usize)?;
        let form = FormKey::new(plugin, raw);
        let sig = self.sig(&form).unwrap_or("").to_owned();
        Some(NamedForm { form, sig })
    }

    /// The forms an editor id names (case does not matter), sorted.
    pub fn forms(&self, editor_id: &str) -> &[NamedForm] {
        let key = editor_id.to_ascii_lowercase();
        self.forms.get(&key).map_or(&[], Vec::as_slice)
    }

    /// A form's signature, when it has an editor id.
    pub fn sig(&self, form: &FormKey) -> Option<&str> {
        self.sigs.get(form).map(String::as_str)
    }

    /// A script's variables.
    pub fn script_vars(&self, script: &FormKey) -> Option<&[VarDecl]> {
        self.script_vars.get(script).map(Vec::as_slice)
    }

    /// A quest's script.
    pub fn quest_script(&self, quest: &FormKey) -> Option<&FormKey> {
        self.quest_scripts.get(quest)
    }

    /// A quest's script variables.
    pub fn quest_vars(&self, quest: &FormKey) -> Option<&[VarDecl]> {
        self.quest_script(quest).and_then(|s| self.script_vars(s))
    }

    /// The script a placed reference runs (its base's).
    pub fn ref_script(&self, reference: &FormKey) -> Option<&FormKey> {
        self.ref_bases
            .get(reference)
            .and_then(|base| self.base_scripts.get(base))
    }

    /// A placed reference's base.
    pub fn ref_base(&self, reference: &FormKey) -> Option<&FormKey> {
        self.ref_bases.get(reference)
    }

    /// A base's script.
    pub fn base_script(&self, base: &FormKey) -> Option<&FormKey> {
        self.base_scripts.get(base)
    }

    /// Records a form list's members (`FLST` `LNAM`), in order.
    pub fn set_form_list(&mut self, list: FormKey, members: Vec<FormKey>) {
        self.form_lists.insert(list, members);
    }

    /// A form list's direct members, in order; `None` when the form is not
    /// a known list.
    pub fn form_list(&self, list: &FormKey) -> Option<&[FormKey]> {
        self.form_lists.get(list).map(Vec::as_slice)
    }
}
