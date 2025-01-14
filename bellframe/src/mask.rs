use std::{
    fmt::{Debug, Display, Formatter},
    ops::Mul,
};

use crate::{
    music::{self, Pattern},
    Bell, Row, RowBuf, Stage,
};
use itertools::Itertools;

/// A mask which fixes the location of some [`Bell`]s.  Unfilled positions are usually denoted by
/// `'x'` (`X` is not a valid [`Bell`] name).
///
/// This can also be thought of as a music [`Pattern`] with no `*`s.
#[derive(Clone, PartialOrd, Ord, PartialEq, Eq, Hash)]
pub struct Mask {
    bells: Vec<Option<Bell>>,
}

impl Mask {
    pub fn parse(s: &str) -> Self {
        Self {
            bells: s
                .chars()
                .filter_map(|c| match c {
                    'x' | 'X' | '.' => Some(None),
                    // Return `Some(Some(Bell))` if `other_char` is a bell name, otherwise `None`
                    // to ignore random chars
                    other_char => Bell::from_name(other_char).map(Some),
                })
                .collect_vec(),
        }

        // TODO: Check validity
    }

    pub fn parse_with_stage(s: &str, stage: Stage) -> Result<Self, ParseError> {
        let pattern = Pattern::parse(s, stage).map_err(ParseError::Pattern)?;
        Self::from_pattern(&pattern).map_err(|MultipleStars| ParseError::MultipleStars)
    }

    /// Convert a [`Pattern`] into a `Mask` of the same [`Stage`], expanding at most one `*` if it
    /// exists.
    pub fn from_pattern(pattern: &Pattern) -> Result<Self, MultipleStars> {
        let stage = pattern.stage();

        // Validate the pattern
        let num_elems = pattern.elems().len();
        let num_stars = pattern
            .elems()
            .iter()
            .filter(|elem| **elem == music::Elem::Star)
            .count();
        let star_length = match num_stars {
            0 => {
                assert_eq!(num_elems, stage.num_bells());
                0
            }
            1 => stage.num_bells() - (num_elems - num_stars),
            _ => return Err(MultipleStars),
        };
        // Construct mask
        let mut bells = Vec::new();
        for &elem in pattern.elems() {
            match elem {
                // Explicit bells are preserved, provided they fit into the stage
                music::Elem::Bell(b) => bells.push(Some(b)),
                // 'x's are always preserved
                music::Elem::X => bells.push(None),
                // If the star exists, replace it with `star_length` 'x's
                music::Elem::Star => bells.extend(std::iter::repeat(None).take(star_length)),
            }
        }
        Ok(Self { bells })
    }

    /// Creates a `Mask` that fully specifies a given [`Row`]
    pub fn full_row(row: &Row) -> Self {
        Self {
            bells: row.bell_iter().map(Some).collect_vec(),
        }
    }

    /// Creates a `Mask` that matches any [`Row`] of a given [`Stage`] (i.e. a mask where no
    /// [`Bell`] is fixed, written as `xxxx...`).
    pub fn empty(stage: Stage) -> Self {
        Self {
            bells: std::iter::repeat(None)
                .take(stage.num_bells())
                .collect_vec(),
        }
    }

    /// Creates a `Mask` that matches any [`Row`] of a given [`Stage`] (i.e. a mask where no
    /// [`Bell`] is fixed, written as `xxxx...`).
    ///
    /// Exactly equivalent to `Mask::empty`
    #[inline]
    pub fn any(stage: Stage) -> Self {
        Self::empty(stage)
    }

    /// Creates a `Mask` that fixes the given [`Bell`]s into their corresponding 'home' place.
    ///
    /// # Panics
    ///
    /// Panics if any of the [`Bell`] are outside the [`Stage`] of this [`Mask`].
    pub fn with_fixed_bells(stage: Stage, fixed_bells: impl IntoIterator<Item = Bell>) -> Self {
        let mut new_mask = Self::empty(stage);
        for b in fixed_bells {
            // SAFETY: bells are only ever fixed to their own locations, so can't be fixed in two
            // different locations
            unsafe { new_mask.fix_unchecked(b) };
        }
        new_mask
    }

    /// Directly create a [`Mask`] from a sequence of ([`Option`]al) [`Bell`]s
    #[inline]
    pub fn from_bells(bells: impl IntoIterator<Item = Option<Bell>>) -> Self {
        Self::from_vec(bells.into_iter().collect_vec())
    }

    /// Directly create a [`Mask`] from a [`Vec`] of ([`Option`]al) [`Bell`]s.
    #[inline]
    // TODO: Verify the sequence here
    pub fn from_vec(bells: Vec<Option<Bell>>) -> Self {
        Self { bells }
    }

    pub fn contains(&self, bell: Bell) -> bool {
        self.bells.contains(&Some(bell))
    }

    /// Returns an [`Iterator`] over the [`Bell`]s (or gaps) in `self`.
    pub fn bells(&self) -> impl DoubleEndedIterator<Item = Option<Bell>> + Clone + '_ {
        self.bells.iter().copied()
    }

    /// Modifies `self` so that a [`Bell`] is fixed in its home position.  Returns `Err(())` if
    /// that [`Bell`] already appears in this `Mask`.
    #[inline]
    pub fn fix(&mut self, b: Bell) -> Result<(), BellAlreadySet> {
        self.set_bell(b, b.index())
    }

    /// Modifies `self` so that a [`Bell`] is fixed in its home position, without checking if that
    /// [`Bell`] is already fixed in a different location.
    ///
    /// # Panics
    ///
    /// Panics if the [`Bell`] is outside the [`Stage`] of this [`Mask`].
    ///
    /// # Safety
    ///
    /// This function is safe if `b` is not already fixed in `self`, or is already fixed to its
    /// home position.
    #[inline]
    pub unsafe fn fix_unchecked(&mut self, b: Bell) {
        self.set_bell_unchecked(b, b.index())
    }

    /// Returns an [`Iterator`] over the indices of locations where this `Mask` contains an `x`
    pub fn unspecified_places(&self) -> impl Iterator<Item = usize> + '_ {
        self.bells
            .iter()
            .enumerate()
            .filter(|(_, b)| b.is_none())
            .map(|(i, _)| i)
    }

    /// Returns the [`Stage`] of [`Row`] that this `Mask` matches
    #[inline(always)]
    pub fn stage(&self) -> Stage {
        Stage::new(self.bells.len() as u8)
    }

    /// Tests whether or not a [`Row`] satisfies this `Mask`.
    pub fn matches(&self, row: &Row) -> bool {
        // Rows can't match masks of different stages
        if self.stage() != row.stage() {
            return false;
        }

        for (&expected_bell, real_bell) in self.bells.iter().zip_eq(row.bell_iter()) {
            if let Some(b) = expected_bell {
                if b != real_bell {
                    // If the mask specifically requested a different bell in this location, then
                    // the row doesn't match
                    return false;
                }
            }
        }
        true
    }

    /// Returns the place of a [`Bell`] within this `Mask`.  If that [`Bell`] isn't found (either
    /// because it's outside the [`Stage`] or because the `Mask` doesn't specify a location) this
    /// returns `None`.
    pub fn place_of(&self, bell: Bell) -> Option<usize> {
        for (i, b) in self.bells.iter().enumerate() {
            if *b == Some(bell) {
                return Some(i);
            }
        }
        None
    }

    /// Updates this `Mask` so that a given [`Bell`] is required at a given place.
    pub fn set_bell(&mut self, bell: Bell, place: usize) -> Result<(), BellAlreadySet> {
        let existing_bell_place = self
            .bells
            .iter()
            .position(|maybe_bell| maybe_bell == &Some(bell));
        match existing_bell_place {
            Some(p) if p == place => Ok(()), // Bell is already fixed here, so nothing to do
            Some(_) => Err(BellAlreadySet(bell)), // Adding the bell would fix it twice
            None => {
                // SAFETY: because this match arm only executes if `bell` isn't fixed in `self`
                unsafe { self.set_bell_unchecked(bell, place) };
                Ok(())
            }
        }
    }

    /// Updates this `Mask` so that a given [`Bell`] is required at a given place.
    ///
    /// # Safety
    ///
    /// This function is safe if `b` is not already fixed in `self`, or is already fixed at the
    /// given `place`.
    pub unsafe fn set_bell_unchecked(&mut self, bell: Bell, place: usize) {
        self.bells[place] = Some(bell);
    }

    pub fn is_empty(&self) -> bool {
        self.bells.iter().all(Option::is_none)
    }

    /// If this mask matches exactly one [`Row`], then return that [`Row`] (otherwise `None`).
    pub fn as_row(&self) -> Option<RowBuf> {
        if self.bells.iter().all(Option::is_some) {
            // SAFETY: The invariants of `self.bells` a superset of those of `Row`s, so a complete
            // `Mask` satisfies all the invariants of `Row` and `RowBuf`.
            Some(unsafe { RowBuf::from_bell_iter_unchecked(self.bells.iter().map(|b| b.unwrap())) })
        } else {
            None
        }
    }

    /// Returns `true` if the set of [`Row`]s satisfying `self` is a subset of those satisfying
    /// `other`.  This implies that `self` is 'stricter' than `other`; for example, `xx3456` is a
    /// subset of `xxxx56`.
    pub fn is_subset_of(&self, other: &Mask) -> bool {
        // Two rows which are of different stages can't have a superset/subset relation
        if self.stage() != other.stage() {
            return false;
        }

        // Now check that every bell required by `other` is also required by `self`
        for (b1, b2) in self.bells.iter().zip_eq(&other.bells) {
            match (*b1, *b2) {
                // If `other` specifies a bell, then `self` must agree
                (None, Some(_)) => return false,
                (Some(b_self), Some(b_other)) => {
                    if b_self != b_other {
                        return false;
                    }
                }
                // If `other` doesn't require a specific bell, then it doesn't matter what's in
                // `self`
                (_, None) => {}
            }
        }

        // If none of the bells caused a disagreement, then `self` is a subset of `other`
        true
    }

    /// Returns `true` if the set of [`Row`]s satisfying `self` is a **strict** subset of those
    /// satisfying `other`.
    pub fn is_strict_subset_of(&self, other: &Mask) -> bool {
        self != other && self.is_subset_of(other)
    }

    /// Check if there exist any [`Row`]s which can satisfy both `Mask`s (i.e. the two `Mask`s are
    /// 'compatible').  `a.is_compatible_with(b)` equivalent to (but faster than)
    /// `a.combine(b).is_some()`.
    pub fn is_compatible_with(&self, other: &Mask) -> bool {
        // Masks of different stages are always incompatible
        if self.stage() != other.stage() {
            return false;
        }

        // Now iterate over `other`'s bells and make sure that, for each specified bell
        // 1. `self` doesn't require a different bell to be in that place
        // 2. `self` doesn't require that bell to be in a different place
        for (i, (&maybe_bell_other, &maybe_bell_self)) in
            other.bells.iter().zip_eq(&self.bells).enumerate()
        {
            if let Some(b_other) = maybe_bell_other {
                // Check that `self` doesn't requires a different bell in this place
                if !maybe_bell_self.map_or(true, |b_self| b_self == b_other) {
                    return false;
                }
                // Check that `self` doesn't require this bell in a different place
                if !self
                    .bells
                    .iter()
                    .position(|&b| b == Some(b_other))
                    .map_or(true, |idx_self| i == idx_self)
                {
                    return false;
                }
            }
        }

        // If no disagreement was found, the masks are compatible
        true
    }

    /// Creates a new `Mask` which matches precisely the [`Row`]s matched by both `self` _and_
    /// `other`.  If `self` and `other` aren't [compatible](Self::is_compatible_with), then such a
    /// `Mask` cannot exist and `None` is returned.
    pub fn intersect(&self, other: &Mask) -> Option<Mask> {
        if !self.is_compatible_with(other) {
            return None;
        }

        Some(Self {
            bells: self
                .bells
                .iter()
                .zip_eq(&other.bells)
                .map(|maybe_bells| match maybe_bells {
                    (Some(b1), Some(b2)) => {
                        assert_eq!(b1, b2);
                        Some(*b1)
                    }
                    (Some(b1), None) => Some(*b1),
                    (None, maybe_bell) => *maybe_bell,
                })
                .collect_vec(),
        })
    }
}

////////////
// ERRORS //
////////////

/// Error returned by [`Mask::fix`]
#[derive(Debug, Clone, Copy)]
pub struct BellAlreadySet(pub Bell);

/// Error returned by [`Mask::from_pattern`]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MultipleStars;

impl Display for MultipleStars {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "`Mask`s can't have more than one `*`")
    }
}

impl std::error::Error for MultipleStars {}

/// The different ways that [`Mask::parse_with_stage`] can fail
#[derive(Debug, Clone)]
pub enum ParseError {
    Pattern(crate::music::PatternError),
    MultipleStars,
}

impl Display for ParseError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            ParseError::MultipleStars => write!(
                f,
                "too many `*`s.  Masks can only have one region with `x` or `*`."
            ),
            ParseError::Pattern(pattern_error) => pattern_error.write_message(f, "mask"),
        }
    }
}

impl std::error::Error for ParseError {}

/* ===== FORMATTING ===== */

impl Debug for Mask {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "Mask({})", self)
    }
}

impl Display for Mask {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        for maybe_bell in &self.bells {
            match maybe_bell {
                Some(b) => write!(f, "{}", b)?,
                None => write!(f, "x")?,
            }
        }
        Ok(())
    }
}

/* ===== ARITHMETIC ===== */

impl Mul<&Row> for &Mask {
    type Output = Mask;

    /// Use a [`Row`] to permute the required [`Bell`]s in a [`Mask`].  Mathematically, if `r` is a
    /// [`Row`] and `m` is a [`Mask`] and `m` matches some [`Row`] `s`, then `m * r` matches `s *
    /// r`.
    ///
    /// # Panics
    ///
    /// Panics if the [`Stage`]s of the [`Row`] and [`Mask`] don't match.
    fn mul(self, rhs: &Row) -> Self::Output {
        assert_eq!(self.stage(), rhs.stage());
        Mask {
            bells: rhs.bell_iter().map(|b| self.bells[b.index()]).collect_vec(),
        }
    }
}

impl Mul<&RowBuf> for &Mask {
    type Output = Mask;

    /// Use a [`RowBuf`] to permute the required [`Bell`]s in a [`Mask`].  Mathematically, if `r`
    /// is a [`RowBuf`] and `m` is a [`Mask`] and `m` matches some [`RowBuf`] `s`, then `m * r`
    /// matches `s * r`.
    ///
    /// # Panics
    ///
    /// Panics if the [`Stage`]s of the [`Row`] and [`Mask`] don't match.
    fn mul(self, rhs: &RowBuf) -> Self::Output {
        self * rhs.as_row()
    }
}

impl Mul<RowBuf> for &Mask {
    type Output = Mask;

    /// Use a [`RowBuf`] to permute the required [`Bell`]s in a [`Mask`].  Mathematically, if `r`
    /// is a [`RowBuf`] and `m` is a [`Mask`] and `m` matches some [`RowBuf`] `s`, then `m * r`
    /// matches `s * r`.
    ///
    /// # Panics
    ///
    /// Panics if the [`Stage`]s of the [`Row`] and [`Mask`] don't match.
    fn mul(self, rhs: RowBuf) -> Self::Output {
        self * rhs.as_row()
    }
}

impl Mul<&Mask> for &Row {
    type Output = Mask;

    /// Use a [`Row`] to transfigure the required [`Bell`]s in a [`Mask`].  Mathematically, if `r`
    /// is a [`Row`] and `m` is a [`Mask`] and `m` matches some [`Row`] `s`, then `r * m` matches
    /// `r * s`.
    ///
    /// # Panics
    ///
    /// Panics if the [`Stage`]s of the [`Row`] and [`Mask`] don't match.
    fn mul(self, rhs: &Mask) -> Self::Output {
        assert_eq!(self.stage(), rhs.stage());
        Mask {
            bells: rhs
                .bells
                .iter()
                .map(|maybe_bell| maybe_bell.map(|b| self[b.index()]))
                .collect_vec(),
        }
    }
}

impl Mul<&Mask> for &RowBuf {
    type Output = Mask;

    /// Use a [`RowBuf`] to transfigure the required [`Bell`]s in a [`Mask`].  Mathematically, if
    /// `r` is a [`RowBuf`] and `m` is a [`Mask`] and `m` matches some [`RowBuf`] `s`, then `r * m`
    /// matches `r * s`.
    ///
    /// # Panics
    ///
    /// Panics if the [`Stage`]s of the [`Row`] and [`Mask`] don't match.
    fn mul(self, rhs: &Mask) -> Self::Output {
        self.as_row() * rhs
    }
}

impl Mul<&Mask> for RowBuf {
    type Output = Mask;

    /// Use a [`RowBuf`] to transfigure the required [`Bell`]s in a [`Mask`].  Mathematically, if
    /// `r` is a [`RowBuf`] and `m` is a [`Mask`] and `m` matches some [`RowBuf`] `s`, then `r * m`
    /// matches `r * s`.
    ///
    /// # Panics
    ///
    /// Panics if the [`Stage`]s of the [`Row`] and [`Mask`] don't match.
    fn mul(self, rhs: &Mask) -> Self::Output {
        self.as_row() * rhs
    }
}

/* ===== CONVERSIONS ===== */

impl From<&Row> for Mask {
    fn from(r: &Row) -> Self {
        Self {
            bells: r.bell_iter().map(Some).collect_vec(),
        }
    }
}

impl From<RowBuf> for Mask {
    fn from(r: RowBuf) -> Self {
        Self::from(r.as_row())
    }
}

impl From<Mask> for Pattern {
    fn from(mask: Mask) -> Pattern {
        Pattern::from_elems(
            mask.bells
                .iter()
                .map(|b| b.map_or(music::Elem::X, music::Elem::Bell)),
            mask.stage(),
        )
        // TODO: This will be safe once `Mask` gets stricter invariants
        .unwrap()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_pattern() {
        #[track_caller]
        fn check_ok(pattern: &str, num_bells: u8, mask: &str) {
            let pattern = Pattern::parse(pattern, Stage::new(num_bells)).unwrap();
            assert_eq!(Mask::from_pattern(&pattern), Ok(Mask::parse(mask)));
        }
        #[track_caller]
        fn check_err(pattern: &str, num_bells: u8) {
            let pattern = Pattern::parse(pattern, Stage::new(num_bells)).unwrap();
            assert_eq!(Mask::from_pattern(&pattern), Err(MultipleStars));
        }

        check_ok("xxx3", 4, "xxx3");
        check_ok("*3", 4, "xxx3");
        check_ok("3*", 4, "3xxx");
        check_ok("3*", 6, "3xxxxx");
        check_ok("3*x", 6, "3xxxxx");
        check_ok("3**x", 6, "3xxxxx"); // Doesn't error because stars are normalised
        check_ok("3*xx*", 6, "3xxxxx"); // Doesn't error because stars are normalised
        check_ok("3*4", 6, "3xxxx4");
        check_ok("3*x4", 6, "3xxxx4");
        check_ok("*78", 8, "xxxxxx78");

        check_err("5*3*8", 8);
        check_err("*5*", 8);
    }

    #[test]
    fn matches() {
        #[track_caller]
        fn check(mask: &str, row: &str, exp_match: bool) {
            let is_match = Mask::parse(mask).matches(&RowBuf::parse(row).unwrap());
            match (is_match, exp_match) {
                (true, false) => panic!("'{}' unexpectedly matched '{}'", mask, row),
                (false, true) => panic!("'{}' unexpectedly didn't match '{}'", row, mask),
                _ => {}
            }
        }

        check("1xx45", "12345", true);
        check("x", "1", true);
        check("1", "1", true);
        check("123456", "123456", true);
        check("123456", "123465", false);
        check("123456", "1234567", false);
        check("x1xx56", "123456", false);
        check("x1xx56", "214356", true);
        check("x1xx56", "241356", false);
    }

    #[test]
    fn row_mul_mask() {
        fn check_ok(row: &str, mask: &str, exp_mask_str: &str) {
            let new_mask = RowBuf::parse(row).unwrap().as_row() * &Mask::parse(mask);
            let exp_mask = Mask::parse(exp_mask_str);
            assert_eq!(
                new_mask, exp_mask,
                "{} * {} gave {} (expected {})",
                row, mask, new_mask, exp_mask_str
            );
        }

        check_ok("12345", "1xx45", "1xx45");
        check_ok("32154", "1xx45", "3xx54");
        check_ok("67812345", "xxxx6578", "xxxx3245");
    }

    #[test]
    fn mask_mul_row() {
        fn check_ok(mask: &str, row: &str, exp_mask_str: &str) {
            let new_mask = Mask::parse(mask).mul(&RowBuf::parse(row).unwrap());
            let exp_mask = Mask::parse(exp_mask_str);
            assert_eq!(
                new_mask, exp_mask,
                "{} * {} gave {} (expected {})",
                mask, row, new_mask, exp_mask_str
            );
        }

        check_ok("1xx45", "12345", "1xx45");
        check_ok("1xx45", "32154", "xx154");
        check_ok("xxxx6578", "67812345", "578xxxx6");
    }
}
