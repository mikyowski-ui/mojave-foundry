//! Raw form ids to form keys.

use crate::ir::FormKey;

/// Resolves the raw 32-bit form ids of a record to form keys.
///
/// The high byte of a raw id indexes the owning plugin's master list, and
/// the master list itself is not in the MIR. This resolver assumes that a
/// plugin's masters are exactly the plugins before it in the load order,
/// which holds for a master alone (`Fallout3.esm`, `FalloutNV.esm`) and for
/// the usual master-then-DLC lists; the typed tables of task T1 resolve
/// through the real master lists and replace it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FormResolver {
    plugins: Vec<String>,
}

impl FormResolver {
    /// A resolver for a load order (names lowercase).
    pub fn new(plugins: Vec<String>) -> Self {
        Self {
            plugins: plugins
                .into_iter()
                .map(|p| p.to_ascii_lowercase())
                .collect(),
        }
    }

    /// The load index of a plugin, by name (case does not matter).
    pub fn plugin_index(&self, name: &str) -> Option<usize> {
        let name = name.to_ascii_lowercase();
        self.plugins.iter().position(|p| *p == name)
    }

    /// The form a raw id stored in a record of plugin `owner` (its load
    /// index) names; `None` for the null id or an index past the owner.
    pub fn resolve(&self, owner: usize, raw: u32) -> Option<FormKey> {
        if raw == 0 {
            return None;
        }
        let high = (raw >> 24) as usize;
        let plugin = match high.cmp(&owner) {
            std::cmp::Ordering::Less => self.plugins.get(high)?,
            std::cmp::Ordering::Equal => self.plugins.get(owner)?,
            std::cmp::Ordering::Greater => return None,
        };
        Some(FormKey::new(plugin, raw & 0x00FF_FFFF))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn high_byte_picks_the_master() {
        let r = FormResolver::new(vec!["Fallout3.esm".into(), "Anchorage.esm".into()]);
        assert_eq!(
            r.resolve(0, 0x0000_0abc),
            Some(FormKey::new("fallout3.esm", 0xabc))
        );
        assert_eq!(
            r.resolve(1, 0x0000_0abc),
            Some(FormKey::new("fallout3.esm", 0xabc))
        );
        assert_eq!(
            r.resolve(1, 0x0100_0abc),
            Some(FormKey::new("anchorage.esm", 0xabc))
        );
        assert_eq!(r.resolve(0, 0x0100_0abc), None);
        assert_eq!(r.resolve(0, 0), None);
        assert_eq!(r.plugin_index("ANCHORAGE.ESM"), Some(1));
    }
}
