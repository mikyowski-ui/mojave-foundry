//! Checks for the "no non-finite float in JSON" rule (`docs/contracts.md`,
//! section 2, "Numbers").

/// Whether every value is finite.
pub(crate) fn all(values: &[f32]) -> bool {
    values.iter().all(|v| v.is_finite())
}

/// Whether every component of every point is finite.
pub(crate) fn points(values: &[[f32; 3]]) -> bool {
    values.iter().all(|p| all(p))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn non_finite_values_are_caught() {
        assert!(all(&[0.0, -1.5, 1e30]));
        assert!(!all(&[0.0, f32::NAN]));
        assert!(!points(&[[0.0, 0.0, f32::INFINITY]]));
    }
}
