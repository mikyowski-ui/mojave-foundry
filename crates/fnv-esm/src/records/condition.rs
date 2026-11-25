//! Conditions (`CTDA`).

use serde::{Deserialize, Serialize};

use crate::bytes::{Rd, Wr};
use crate::formid::FormId;

/// How a condition compares the function result with its value. Stored in
/// the top three bits of the first `CTDA` byte.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum Comparison {
    /// `==`
    #[default]
    Equal,
    /// `!=`
    NotEqual,
    /// `>`
    Greater,
    /// `>=`
    GreaterOrEqual,
    /// `<`
    Less,
    /// `<=`
    LessOrEqual,
    /// The undocumented codes 6 and 7, kept as stored.
    Reserved(u8),
}

impl Comparison {
    fn from_code(code: u8) -> Self {
        match code {
            0 => Self::Equal,
            1 => Self::NotEqual,
            2 => Self::Greater,
            3 => Self::GreaterOrEqual,
            4 => Self::Less,
            5 => Self::LessOrEqual,
            other => Self::Reserved(other),
        }
    }

    fn code(self) -> u8 {
        match self {
            Self::Equal => 0,
            Self::NotEqual => 1,
            Self::Greater => 2,
            Self::GreaterOrEqual => 3,
            Self::Less => 4,
            Self::LessOrEqual => 5,
            Self::Reserved(code) => code & 0x07,
        }
    }
}

/// What a condition compares against.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum ConditionValue {
    /// A literal.
    Float(f32),
    /// The value of a global variable.
    Global(FormId),
}

impl Default for ConditionValue {
    fn default() -> Self {
        Self::Float(0.0)
    }
}

/// One condition.
///
/// The function is reported as its index; this crate has no name table.
/// Whether a parameter is a form id, an integer or an enum depends on the
/// function, so both are given as the stored 32 bits.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct Condition {
    /// The comparison operator.
    pub comparison: Comparison,
    /// The low five bits of the first byte, as stored. See the `FLAG_*`
    /// constants. [`Condition::FLAG_USE_GLOBAL`] always agrees with
    /// [`Condition::value`] in a decoded condition; when encoding, the bit
    /// is taken from `value`.
    pub flags: u8,
    /// The comparison value.
    pub value: ConditionValue,
    /// The function index, the stored 32-bit field. Every documented index
    /// fits in 16 bits, and the upper 16 are zero in every condition of a
    /// retail master, so reading the field as 32 or as 16 bits gives the
    /// same index; see [`Condition::function_index`].
    pub function: u32,
    /// The first parameter, as stored.
    pub param1: u32,
    /// The second parameter, as stored.
    pub param2: u32,
    /// What the function runs on: 0 subject, 1 target, 2 reference,
    /// 3 combat target, 4 linked reference. 0 when the stored condition is
    /// the short 20-byte form.
    pub run_on: u32,
    /// The reference for run-on 2; null when the stored form has none.
    pub reference: FormId,
}

/// The condition functions whose parameters can name a placed reference,
/// with the parameter slots (`[first, second]`) that do.
///
/// Found by resolving the two parameters of every condition of both
/// masters against the placed references (`REFR`, `ACHR`, `ACRE`): only
/// these functions ever have one. The names are the public function names
/// where they are settled (`GetDistance`, `GetInSameCell`, `GetDetected`,
/// `GetScriptVariable`, `GetIsReference`, `IsCombatTarget`); 162 and 464
/// (New Vegas) and 230 (Fallout 3, second parameter) are listed by what the
/// data shows. `GetDead` and `GetDisabled` do not appear: they run on a
/// reference, which the condition's `run_on` and `reference` fields give.
pub const REFERENCE_PARAMETER_FUNCTIONS: &[(u16, [bool; 2])] = &[
    (1, [true, false]),
    (32, [true, false]),
    (45, [true, false]),
    (53, [true, false]),
    (136, [true, false]),
    (162, [true, false]),
    (230, [false, true]),
    (464, [true, false]),
    (515, [true, false]),
];

/// Which parameters of a condition function can name a placed reference;
/// see [`REFERENCE_PARAMETER_FUNCTIONS`].
pub fn reference_parameter_slots(function: u16) -> [bool; 2] {
    REFERENCE_PARAMETER_FUNCTIONS
        .iter()
        .find(|(index, _)| *index == function)
        .map_or([false, false], |&(_, slots)| slots)
}

impl Condition {
    /// The parameters that can name a placed reference, as form ids (as
    /// stored; a value of 0 is `None`): see
    /// [`REFERENCE_PARAMETER_FUNCTIONS`].
    pub fn reference_params(&self) -> [Option<FormId>; 2] {
        let slots = reference_parameter_slots(self.function_index());
        let param = |slot: bool, value: u32| (slot && value != 0).then_some(FormId(value));
        [param(slots[0], self.param1), param(slots[1], self.param2)]
    }

    /// This condition is OR-ed with the next one instead of AND-ed.
    pub const FLAG_OR: u8 = 0x01;
    /// Run on the target (the older form of [`Condition::run_on`]).
    pub const FLAG_RUN_ON_TARGET: u8 = 0x02;
    /// The comparison value is a global variable.
    pub const FLAG_USE_GLOBAL: u8 = 0x04;

    /// A condition comparing function `function` against a literal.
    pub fn float(function: u32, comparison: Comparison, value: f32) -> Self {
        Self {
            comparison,
            value: ConditionValue::Float(value),
            function,
            ..Self::default()
        }
    }

    /// A condition comparing function `function` against a global.
    pub fn global(function: u32, comparison: Comparison, global: impl Into<FormId>) -> Self {
        Self {
            comparison,
            flags: Self::FLAG_USE_GLOBAL,
            value: ConditionValue::Global(global.into()),
            function,
            ..Self::default()
        }
    }

    /// Sets both parameters.
    #[must_use]
    pub fn with_params(mut self, param1: u32, param2: u32) -> Self {
        self.param1 = param1;
        self.param2 = param2;
        self
    }

    /// Sets the OR flag.
    #[must_use]
    pub fn or(mut self) -> Self {
        self.flags |= Self::FLAG_OR;
        self
    }

    /// The low 16 bits of [`Condition::function`].
    pub fn function_index(&self) -> u16 {
        (self.function & 0xFFFF) as u16
    }

    /// Whether this condition is OR-ed with the next.
    pub fn is_or(&self) -> bool {
        self.flags & Self::FLAG_OR != 0
    }

    /// Reads a `CTDA` payload of 28 bytes, or of the shorter 24-byte (no
    /// reference) and 20-byte (no run-on either) forms.
    pub(crate) fn parse(data: &[u8]) -> Option<Self> {
        if !matches!(data.len(), 20 | 24 | 28) {
            return None;
        }
        let mut rd = Rd::new(data);
        let kind = rd.u8()?;
        rd.skip(3)?;
        let flags = kind & 0x1F;
        let raw_value = rd.arr::<4>()?;
        let value = if flags & Self::FLAG_USE_GLOBAL != 0 {
            ConditionValue::Global(FormId(u32::from_le_bytes(raw_value)))
        } else {
            ConditionValue::Float(f32::from_le_bytes(raw_value))
        };
        Some(Self {
            comparison: Comparison::from_code(kind >> 5),
            flags,
            value,
            function: rd.u32()?,
            param1: rd.u32()?,
            param2: rd.u32()?,
            run_on: rd.u32().unwrap_or(0),
            reference: rd.form().unwrap_or(FormId::NULL),
        })
    }

    /// The 28-byte `CTDA` payload.
    pub(crate) fn bytes(&self) -> Vec<u8> {
        let (global_bit, raw_value) = match self.value {
            ConditionValue::Float(v) => (0, v.to_le_bytes()),
            ConditionValue::Global(form) => (Self::FLAG_USE_GLOBAL, form.0.to_le_bytes()),
        };
        let flags = (self.flags & 0x1F & !Self::FLAG_USE_GLOBAL) | global_bit;
        Wr::default()
            .u8((self.comparison.code() << 5) | flags)
            .zeros(3)
            .bytes(&raw_value)
            .u32(self.function)
            .u32(self.param1)
            .u32(self.param2)
            .u32(self.run_on)
            .form(self.reference)
            .done()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_byte_splits_into_comparison_and_flags() {
        let mut data = vec![0u8; 28];
        // 0b011_00101: greater-or-equal, OR + use-global.
        data[0] = 0x65;
        data[4..8].copy_from_slice(&0x0100_0ABCu32.to_le_bytes());
        data[8..12].copy_from_slice(&72u32.to_le_bytes());
        data[12..16].copy_from_slice(&7u32.to_le_bytes());
        data[16..20].copy_from_slice(&9u32.to_le_bytes());
        data[20..24].copy_from_slice(&2u32.to_le_bytes());
        data[24..28].copy_from_slice(&0x0000_1234u32.to_le_bytes());
        let c = Condition::parse(&data).expect("28-byte condition");
        assert_eq!(c.comparison, Comparison::GreaterOrEqual);
        assert_eq!(c.flags, 0x05);
        assert!(c.is_or());
        assert_eq!(c.value, ConditionValue::Global(FormId(0x0100_0ABC)));
        assert_eq!((c.function, c.function_index()), (72, 72));
        assert_eq!((c.param1, c.param2, c.run_on), (7, 9, 2));
        assert_eq!(c.reference, FormId(0x1234));
        assert_eq!(c.bytes(), data);
    }

    #[test]
    fn short_forms_default_the_missing_tail() {
        let mut data = vec![0u8; 20];
        data[0] = 0x80; // less-than, no flags
        data[4..8].copy_from_slice(&2.5f32.to_le_bytes());
        data[8..12].copy_from_slice(&14u32.to_le_bytes());
        let c = Condition::parse(&data).expect("20-byte condition");
        assert_eq!(c.comparison, Comparison::Less);
        assert_eq!(c.value, ConditionValue::Float(2.5));
        assert_eq!((c.run_on, c.reference), (0, FormId::NULL));
        assert_eq!(Condition::parse(&c.bytes()), Some(c));
        assert_eq!(Condition::parse(&data[..19]), None);
        assert_eq!(Condition::parse(&[0u8; 32]), None);
    }

    #[test]
    fn reserved_comparison_codes_survive() {
        let mut data = vec![0u8; 28];
        data[0] = 0xE0;
        let c = Condition::parse(&data).expect("condition");
        assert_eq!(c.comparison, Comparison::Reserved(7));
        assert_eq!(c.bytes()[0], 0xE0);
    }
}
