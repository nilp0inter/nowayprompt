//! Exact scale representation and checked logical→physical arithmetic.
//!
//! Wayland expresses scales as exact rationals: core `wl_output.scale`
//! carries a positive integer, and `wp_fractional_scale_v1.preferred_scale`
//! carries a numerator over the fixed denominator 120. Storing only `f32`
//! was rejected (see design): protocol values are exact and physical
//! dimensions are integer allocation keys, so this type keeps everything
//! in integers and converts to `f32` only at the tiny-skia/cosmic-text
//! drawing boundary.

use crate::frontend::FrontendError;

/// Fixed denominator of the fractional-scale protocol.
pub const FRACTIONAL_DENOMINATOR: u32 = 120;

/// The protocol mode that produced a scale value, used for change
/// detection: a transition between `Integer` and `Fractional` (or a
/// change of the numerator within the same mode) must trigger a rerender.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScaleMode {
    Integer,
    Fractional,
}

/// An exact scale factor.
///
/// - [`Scale::Integer`]` (N)` is the core `wl_output.scale` value; the
///   denominator is 1.
/// - [`Scale::Fractional`]` (P)` is the `preferred_scale` numerator; the
///   denominator is [`FRACTIONAL_DENOMINATOR`] (120).
///
/// `N` and `P` are stored as `u32`; a physical dimension is
/// `ceil(logical * numerator / denominator)` computed with checked
/// arithmetic.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scale {
    /// Core integer buffer scale `N > 0`.
    Integer(u32),
    /// Fractional-scale numerator `P` over [`FRACTIONAL_DENOMINATOR`].
    Fractional(u32),
}

impl Scale {
    /// The 1× default used until an output or preferred scale is known.
    pub const ONE: Scale = Scale::Integer(1);

    /// The protocol mode, for change detection.
    pub fn mode(&self) -> ScaleMode {
        match self {
            Scale::Integer(_) => ScaleMode::Integer,
            Scale::Fractional(_) => ScaleMode::Fractional,
        }
    }

    /// The numerator (N for integer mode, P for fractional mode).
    pub fn numerator(&self) -> u32 {
        match self {
            Scale::Integer(n) => *n,
            Scale::Fractional(p) => *p,
        }
    }

    /// The denominator (1 for integer mode, 120 for fractional mode).
    pub fn denominator(&self) -> u32 {
        match self {
            Scale::Integer(_) => 1,
            Scale::Fractional(_) => FRACTIONAL_DENOMINATOR,
        }
    }

    /// Convert to `f32` for the tiny-skia/cosmic-text drawing boundary.
    pub fn as_f32(&self) -> f32 {
        match self {
            Scale::Integer(n) => *n as f32,
            Scale::Fractional(p) => *p as f32 / FRACTIONAL_DENOMINATOR as f32,
        }
    }

    /// Compute a physical extent: `ceil(logical * numerator / denominator)`,
    /// with checked arithmetic. Overflow or an unrepresentable result is a
    /// [`FrontendError::Init`]; the caller surfaces it before any buffer
    /// allocation or protocol request.
    pub fn physical_dim(&self, logical: u32) -> Result<u32, FrontendError> {
        let num = self.numerator();
        let den = self.denominator();
        // num and den are small constants, but the multiply is the
        // dangerous step.
        let scaled = logical
            .checked_mul(num)
            .ok_or_else(|| FrontendError::Init("physical dimension overflow".into()))?;
        // ceil(scaled / den). `den` is a small positive constant.
        let phys = scaled
            .checked_add(den - 1)
            .ok_or_else(|| FrontendError::Init("physical dimension overflow".into()))?
            / den;
        if phys == 0 {
            return Err(FrontendError::Init("zero physical dimension".into()));
        }
        Ok(phys)
    }

    /// Compute the physical buffer `(width, height)` from logical
    /// dimensions, with checked arithmetic.
    pub fn physical_size(
        &self,
        logical_width: u32,
        logical_height: u32,
    ) -> Result<(u32, u32), FrontendError> {
        Ok((
            self.physical_dim(logical_width)?,
            self.physical_dim(logical_height)?,
        ))
    }
}

/// Pure effective-scale decision: the protocol-level policy that selects
/// the surface's [`Scale`] from fractional availability, the latest
/// `preferred_scale` numerator, and the positive integer scales of the
/// entered outputs.
///
/// A fractional preferred scale `P` takes precedence when fractional
/// scaling is enabled (both the fractional-scale object and a viewport
/// exist). Otherwise the highest positive integer scale among entered
/// outputs is used, defaulting to 1× when none has reported a positive
/// scale.
///
/// Extracted from `Surface::effective_scale` so the decision logic is
/// testable without a live Wayland connection. `entered_scales` is the
/// iterator of integer scales (one per entered output; 0 until reported).
pub fn compute_effective_scale(
    fractional_enabled: bool,
    fractional_preferred: Option<u32>,
    entered_scales: impl Iterator<Item = u32>,
) -> Scale {
    if fractional_enabled {
        if let Some(p) = fractional_preferred {
            return Scale::Fractional(p);
        }
    }
    let integer = entered_scales.filter(|&s| s > 0).max().unwrap_or(1);
    Scale::Integer(integer)
}

#[cfg(test)]
mod tests {
    use super::{Scale, ScaleMode};

    #[test]
    fn integer_mode_and_value() {
        assert_eq!(Scale::Integer(2).mode(), ScaleMode::Integer);
        assert_eq!(Scale::Integer(2).numerator(), 2);
        assert_eq!(Scale::Integer(2).denominator(), 1);
        assert_eq!(Scale::Integer(2).as_f32(), 2.0);
    }

    #[test]
    fn fractional_mode_and_value() {
        assert_eq!(Scale::Fractional(180).mode(), ScaleMode::Fractional);
        assert_eq!(Scale::Fractional(180).numerator(), 180);
        assert_eq!(Scale::Fractional(180).denominator(), 120);
        assert_eq!(Scale::Fractional(180).as_f32(), 1.5);
    }

    #[test]
    fn integer_physical_dim_is_exact_product() {
        // 200 logical * 2 = 400.
        assert_eq!(Scale::Integer(2).physical_dim(200).unwrap(), 400);
        assert_eq!(Scale::ONE.physical_dim(200).unwrap(), 200);
    }

    #[test]
    fn fractional_physical_dim_rounds_up() {
        // 200 * 180 / 120 = 300.
        assert_eq!(Scale::Fractional(180).physical_dim(200).unwrap(), 300);
        // 101 * 150 / 120 = ceil(126.25) = 127.
        assert_eq!(Scale::Fractional(150).physical_dim(101).unwrap(), 127);
        // 100 * 120 / 120 = 100.
        assert_eq!(Scale::Fractional(120).physical_dim(100).unwrap(), 100);
    }

    #[test]
    fn physical_size_rounds_each_extent() {
        let (w, h) = Scale::Fractional(150).physical_size(101, 50).unwrap();
        // 101 * 150 / 120 = ceil(126.25) = 127; 50 * 150 / 120 = ceil(62.5) = 63.
        assert_eq!((w, h), (127, 63));
    }

    #[test]
    fn overflow_rejected() {
        assert!(Scale::Integer(2).physical_dim(u32::MAX).is_err());
        // A large fractional numerator over a small logical extent.
        assert!(Scale::Fractional(u32::MAX).physical_dim(u32::MAX).is_err());
    }

    #[test]
    fn zero_logical_rejected() {
        assert!(Scale::Integer(1).physical_dim(0).is_err());
        assert!(Scale::Fractional(120).physical_dim(0).is_err());
    }

    // --- Effective-scale decision logic (Task 5.1) ---
    // Pure tests of `compute_effective_scale` covering scale rounding,
    // overflow rejection, integer fallback, fractional precedence,
    // multi-output selection, enter/leave, and output removal — all
    // without a live Wayland connection.

    use super::compute_effective_scale;

    #[test]
    fn effective_integer_scale_exact() {
        // Exact integer N: no rounding needed.
        assert_eq!(
            compute_effective_scale(false, None, [1].into_iter()),
            Scale::Integer(1)
        );
        assert_eq!(
            compute_effective_scale(false, None, [2].into_iter()),
            Scale::Integer(2)
        );
    }

    #[test]
    fn effective_fractional_ceil_semantics() {
        // Fractional P/120 with non-integral result takes the exact
        // numerator; the physical-dimension ceil is exercised separately
        // (see fractional_physical_dim_rounds_up above, which verifies
        // 101 * 150/120 → ceil(126.25) = 127).
        assert_eq!(
            compute_effective_scale(true, Some(150), [1].into_iter()),
            Scale::Fractional(150)
        );
        // 180/120 = 1.5×.
        assert_eq!(
            compute_effective_scale(true, Some(180), [1, 2].into_iter()),
            Scale::Fractional(180)
        );
    }

    #[test]
    fn effective_integer_fallback_default_one() {
        // No entered outputs → default 1×.
        assert_eq!(
            compute_effective_scale(false, None, [].into_iter()),
            Scale::Integer(1)
        );
        // All entered outputs report 0 (not yet reported) → default 1×.
        assert_eq!(
            compute_effective_scale(false, None, [0, 0].into_iter()),
            Scale::Integer(1)
        );
    }

    #[test]
    fn effective_integer_fallback_highest() {
        // Highest positive integer scale among entered outputs.
        assert_eq!(
            compute_effective_scale(false, None, [1, 2].into_iter()),
            Scale::Integer(2)
        );
        assert_eq!(
            compute_effective_scale(false, None, [3, 1, 2].into_iter()),
            Scale::Integer(3)
        );
        // Scale 0 entries are ignored.
        assert_eq!(
            compute_effective_scale(false, None, [0, 2, 0].into_iter()),
            Scale::Integer(2)
        );
    }

    #[test]
    fn effective_fractional_precedence_over_integer() {
        // preferred_scale(180) beats entered-output integer scales.
        assert_eq!(
            compute_effective_scale(true, Some(180), [1, 2].into_iter()),
            Scale::Fractional(180)
        );
        // 150/120 = 1.25× beats a 2× integer output.
        assert_eq!(
            compute_effective_scale(true, Some(150), [2].into_iter()),
            Scale::Fractional(150)
        );
    }

    #[test]
    fn effective_fractional_ignored_when_not_enabled() {
        // When fractional scaling is unavailable (no viewporter/fractional),
        // a preferred_scale event is ignored → integer fallback.
        assert_eq!(
            compute_effective_scale(false, Some(180), [1, 2].into_iter()),
            Scale::Integer(2)
        );
        // No preferred scale and no fractional → integer fallback too.
        assert_eq!(
            compute_effective_scale(false, None, [2].into_iter()),
            Scale::Integer(2)
        );
    }

    #[test]
    fn effective_fractional_no_preferred_falls_back_to_integer() {
        // Fractional enabled but no preferred_scale received yet →
        // highest entered-output integer scale.
        assert_eq!(
            compute_effective_scale(true, None, [1, 2].into_iter()),
            Scale::Integer(2)
        );
        assert_eq!(
            compute_effective_scale(true, None, [].into_iter()),
            Scale::Integer(1)
        );
    }

    #[test]
    fn effective_scale_enter_leave() {
        // Enter: only the 2× output entered → effective 2.
        assert_eq!(
            compute_effective_scale(false, None, [2].into_iter()),
            Scale::Integer(2)
        );
        // Leave the only 2× output, remaining on 1× → effective 1.
        assert_eq!(
            compute_effective_scale(false, None, [1].into_iter()),
            Scale::Integer(1)
        );
    }

    #[test]
    fn effective_scale_multi_output_then_removal() {
        // Entered outputs with scales 1 and 2 → effective 2.
        assert_eq!(
            compute_effective_scale(false, None, [1, 2].into_iter()),
            Scale::Integer(2)
        );
        // global_remove of the 2× output → only 1× remains → effective 1.
        assert_eq!(
            compute_effective_scale(false, None, [1].into_iter()),
            Scale::Integer(1)
        );
    }

    #[test]
    fn physical_dim_overflow_rejected() {
        // physical_dim overflow (multiplication).
        assert!(Scale::Integer(2).physical_dim(u32::MAX).is_err());
        // physical_dim overflow (ceiling add).
        assert!(Scale::Fractional(120).physical_dim(u32::MAX).is_err());
        // physical_size overflow on either extent.
        assert!(Scale::Integer(2).physical_size(1, u32::MAX).is_err());
    }

    #[test]
    fn physical_dim_exact_boundary_no_overflow() {
        // A dimension that just fits should succeed.
        // 2^31 - 1 is the max i32; physical_dim should reject values
        // exceeding u32::MAX arithmetic but accept values that fit.
        assert_eq!(Scale::Integer(1).physical_dim(u32::MAX).unwrap(), u32::MAX);
        // physical_size: both extents at the boundary.
        assert_eq!(
            Scale::ONE.physical_size(u32::MAX, 1).unwrap(),
            (u32::MAX, 1)
        );
    }
}
