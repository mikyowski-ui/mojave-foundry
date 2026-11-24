//! Deserializing over an existing value: a key the input gives replaces
//! that field (or, for a nested table, is overlaid into it); a key it
//! leaves out keeps the current value; an unknown key is an error. This
//! is what lets a rebalance file change a few keys over a per-game
//! default, which `#[serde(default)]` alone cannot do (its defaults are
//! one per type, not one per game).

use serde::Deserialize;
use serde::de::{DeserializeSeed, Deserializer, MapAccess, Visitor};
use std::collections::BTreeMap;
use std::fmt;
use std::marker::PhantomData;

/// A value that can take a deserialized overlay.
pub(crate) trait Overlay {
    fn overlay<'de, D: Deserializer<'de>>(&mut self, deserializer: D) -> Result<(), D::Error>;
}

/// The seed that overlays into `&mut T`.
pub(crate) struct Seed<'a, T>(pub(crate) &'a mut T);

impl<'de, T: Overlay> DeserializeSeed<'de> for Seed<'_, T> {
    type Value = ();
    fn deserialize<D: Deserializer<'de>>(self, deserializer: D) -> Result<(), D::Error> {
        self.0.overlay(deserializer)
    }
}

/// Leaves: the whole value is replaced.
macro_rules! overlay_leaf {
    ($($ty:ty),* $(,)?) => {$(
        impl $crate::overlay::Overlay for $ty {
            fn overlay<'de, D: serde::de::Deserializer<'de>>(
                &mut self,
                deserializer: D,
            ) -> Result<(), D::Error> {
                *self = <$ty as serde::Deserialize>::deserialize(deserializer)?;
                Ok(())
            }
        }
    )*};
}
pub(crate) use overlay_leaf;

overlay_leaf!(i32, f32, bool, String, Option<f32>, [i32; 2], [f32; 2]);

/// Structs: each named field is overlaid; others are refused.
macro_rules! overlay_struct {
    ($ty:ty { $($field:ident),* $(,)? }) => {
        impl $crate::overlay::Overlay for $ty {
            fn overlay<'de, D: serde::de::Deserializer<'de>>(
                &mut self,
                deserializer: D,
            ) -> Result<(), D::Error> {
                const FIELDS: &[&str] = &[$(stringify!($field)),*];
                struct V<'a>(&'a mut $ty);
                impl<'de> serde::de::Visitor<'de> for V<'_> {
                    type Value = ();
                    fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                        write!(f, "a table of {}", stringify!($ty))
                    }
                    fn visit_map<A: serde::de::MapAccess<'de>>(
                        self,
                        mut map: A,
                    ) -> Result<(), A::Error> {
                        while let Some(key) = map.next_key::<String>()? {
                            match key.as_str() {
                                $(stringify!($field) => {
                                    map.next_value_seed($crate::overlay::Seed(&mut self.0.$field))?
                                })*
                                other => {
                                    return Err(<A::Error as serde::de::Error>::unknown_field(
                                        other, FIELDS,
                                    ));
                                }
                            }
                        }
                        Ok(())
                    }
                }
                deserializer.deserialize_map(V(self))
            }
        }
    };
}
pub(crate) use overlay_struct;

/// Maps: an entry the input names is overlaid when it exists, inserted
/// whole when it does not; other entries are kept.
impl<K, V> Overlay for BTreeMap<K, V>
where
    K: Ord + for<'de> Deserialize<'de>,
    V: Overlay + for<'de> Deserialize<'de>,
{
    fn overlay<'de, D: Deserializer<'de>>(&mut self, deserializer: D) -> Result<(), D::Error> {
        struct MapVisitor<'a, K, V>(&'a mut BTreeMap<K, V>, PhantomData<(K, V)>);
        impl<'de, K, V> Visitor<'de> for MapVisitor<'_, K, V>
        where
            K: Ord + for<'k> Deserialize<'k>,
            V: Overlay + for<'v> Deserialize<'v>,
        {
            type Value = ();
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("a table")
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<(), A::Error> {
                while let Some(key) = map.next_key::<K>()? {
                    match self.0.get_mut(&key) {
                        Some(value) => map.next_value_seed(Seed(value))?,
                        None => {
                            let value = map.next_value::<V>()?;
                            self.0.insert(key, value);
                        }
                    }
                }
                Ok(())
            }
        }
        deserializer.deserialize_map(MapVisitor(self, PhantomData))
    }
}

/// Lists are leaves.
impl<T: for<'de> Deserialize<'de>> Overlay for Vec<T> {
    fn overlay<'de, D: Deserializer<'de>>(&mut self, deserializer: D) -> Result<(), D::Error> {
        *self = Vec::<T>::deserialize(deserializer)?;
        Ok(())
    }
}
