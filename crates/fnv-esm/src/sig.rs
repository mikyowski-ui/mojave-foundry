//! Four-byte signatures of records, groups and subrecords.

use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// A four-byte record, group or subrecord signature, such as `REFR`.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Sig(pub [u8; 4]);

impl Sig {
    /// Builds a signature from a four-byte literal: `Sig::new(b"REFR")`.
    pub const fn new(bytes: &[u8; 4]) -> Self {
        Self(*bytes)
    }

    /// The four stored bytes.
    pub const fn as_bytes(&self) -> &[u8; 4] {
        &self.0
    }
}

impl From<&[u8; 4]> for Sig {
    fn from(bytes: &[u8; 4]) -> Self {
        Self(*bytes)
    }
}

/// Printable ASCII is shown as is; any other byte as `\xNN`.
impl fmt::Display for Sig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for &b in &self.0 {
            if b.is_ascii_graphic() || b == b' ' {
                write!(f, "{}", b as char)?;
            } else {
                write!(f, "\\x{b:02X}")?;
            }
        }
        Ok(())
    }
}

impl fmt::Debug for Sig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Sig({self})")
    }
}

/// Serialized as a four-character string, each byte as the code point of the
/// same value, so every signature has exactly one string form.
impl Serialize for Sig {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let text: String = self.0.iter().map(|&b| char::from(b)).collect();
        serializer.serialize_str(&text)
    }
}

impl<'de> Deserialize<'de> for Sig {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        let mut bytes = [0u8; 4];
        let mut count = 0usize;
        for c in text.chars() {
            let code = u32::from(c);
            let byte = u8::try_from(code)
                .map_err(|_| serde::de::Error::custom("signature character above U+00FF"))?;
            if count >= 4 {
                return Err(serde::de::Error::custom("signature longer than 4 bytes"));
            }
            bytes[count] = byte;
            count += 1;
        }
        if count != 4 {
            return Err(serde::de::Error::custom("signature shorter than 4 bytes"));
        }
        Ok(Self(bytes))
    }
}

/// Signatures this crate names.
pub mod sigs {
    use super::Sig;

    macro_rules! sig_consts {
        ($($name:ident = $bytes:literal;)*) => {
            $(
                #[doc = concat!("The `", stringify!($name), "` signature.")]
                pub const $name: Sig = Sig::new($bytes);
            )*
        };
    }

    sig_consts! {
        TES4 = b"TES4";
        GRUP = b"GRUP";
        XXXX = b"XXXX";
        WRLD = b"WRLD";
        CELL = b"CELL";
        LAND = b"LAND";
        NAVM = b"NAVM";
        REFR = b"REFR";
        ACHR = b"ACHR";
        ACRE = b"ACRE";
        STAT = b"STAT";
        DOOR = b"DOOR";
        CONT = b"CONT";
        MISC = b"MISC";
        WEAP = b"WEAP";
        ARMO = b"ARMO";
        ALCH = b"ALCH";
        AMMO = b"AMMO";
        NPC_ = b"NPC_";
        CREA = b"CREA";
        QUST = b"QUST";
        DIAL = b"DIAL";
        INFO = b"INFO";
        SCPT = b"SCPT";
        LTEX = b"LTEX";
        TXST = b"TXST";
        RACE = b"RACE";
        HDPT = b"HDPT";
        HAIR = b"HAIR";
        EYES = b"EYES";
        ARMA = b"ARMA";
        FLST = b"FLST";
        LVLN = b"LVLN";
        LVLC = b"LVLC";
        LVLI = b"LVLI";
        TERM = b"TERM";
        NOTE = b"NOTE";
        PACK = b"PACK";
    }
}
