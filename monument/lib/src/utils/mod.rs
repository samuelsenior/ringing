use std::{
    cmp::Ordering,
    ops::{Add, AddAssign},
};

use bellframe::{Row, RowBuf, Stage, Stroke};
use itertools::Itertools;

use crate::parameters::MusicType;

use self::counts::Counts;

pub(crate) mod counts;
pub(crate) mod lengths;

pub use lengths::{PerPartLength, TotalLength};

/// A container type which sorts its contents according to some given distance metric
#[derive(Debug, Clone)]
pub(crate) struct FrontierItem<Item, Dist> {
    pub item: Item,
    pub distance: Dist,
}

impl<Item, Dist> FrontierItem<Item, Dist> {
    pub fn new(item: Item, distance: Dist) -> Self {
        Self { item, distance }
    }
}

impl<Item, Dist: PartialEq> PartialEq for FrontierItem<Item, Dist> {
    fn eq(&self, other: &Self) -> bool {
        self.distance == other.distance
    }
}

impl<Item, Dist: Eq> Eq for FrontierItem<Item, Dist> {}

impl<Item, Dist: Ord> PartialOrd for FrontierItem<Item, Dist> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl<Item, Dist: Ord> Ord for FrontierItem<Item, Dist> {
    fn cmp(&self, other: &Self) -> Ordering {
        self.distance.cmp(&other.distance)
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum Boundary {
    Start,
    End,
}

/// Integer division, but rounding up instead of down
pub(crate) fn div_rounding_up(lhs: usize, rhs: usize) -> usize {
    (lhs + rhs - 1) / rhs
}

/// A breakdown of the music generated by a composition
#[derive(Debug, Clone)]
pub(crate) struct MusicBreakdown {
    pub score: f32,
    /// The number of occurrences of each [`MusicType`] specified in the current
    /// [`Query`](crate::Query)
    pub counts: Counts,
}

impl MusicBreakdown {
    /// Creates the `Score` of 0 (i.e. the `Score` generated by no rows).
    pub fn zero(num_music_types: usize) -> Self {
        Self {
            score: 0.0,
            counts: Counts::zeros(num_music_types),
        }
    }

    /// Returns the `Score` generated by a sequence of [`Row`]s, (pre-)transposed by some course
    /// head.
    pub fn from_rows<'r>(
        rows: impl IntoIterator<Item = &'r Row>,
        pre_transposition: &Row,
        music_types: &[MusicType],
        start_stroke: Stroke,
    ) -> Self {
        let mut temp_row = RowBuf::rounds(Stage::ONE);
        let mut occurences = vec![0; music_types.len()];
        // For every (transposed) row ...
        for (idx, r) in rows.into_iter().enumerate() {
            pre_transposition.mul_into(r, &mut temp_row).unwrap();
            // ... for every music type ...
            for (num_instances, ty) in occurences.iter_mut().zip_eq(music_types) {
                if ty.strokes.contains(start_stroke.offset(idx)) {
                    // ... count the number of instances of that type of music
                    for pattern in &ty.patterns {
                        // Unwrap is safe because `pattern` must have the same `Stage` as the rest
                        // of the rows
                        if pattern.matches(&temp_row).unwrap() {
                            *num_instances += 1;
                        }
                    }
                }
            }
        }

        Self {
            score: occurences
                .iter()
                .zip_eq(music_types)
                .map(|(&num_instances, ty)| ty.weight * num_instances as f32)
                .sum(),
            counts: occurences.into(),
        }
    }

    /// # Panics
    ///
    /// Panics if the number of [`MusicType`]s in `rhs` is different to that of `self`.
    pub fn saturating_sub(&self, rhs: &Self) -> Self {
        MusicBreakdown {
            score: self.score - rhs.score,
            counts: self.counts.saturating_sub(&rhs.counts),
        }
    }

    /// # Panics
    ///
    /// Panics if the number of [`MusicType`]s in `rhs` is different to that of `self`.
    pub fn saturating_sub_assign(&mut self, rhs: &Self) {
        self.score -= rhs.score;
        self.counts.saturating_sub_assign(&rhs.counts);
    }
}

impl Add for &MusicBreakdown {
    type Output = MusicBreakdown;

    /// Combines two [`Score`]s to create one [`Score`] representing both `self` and `rhs`.
    ///
    /// # Panics
    ///
    /// Panics if the number of [`MusicType`]s in `rhs` is different to that of `self`.
    fn add(self, rhs: &MusicBreakdown) -> Self::Output {
        MusicBreakdown {
            score: self.score + rhs.score,
            counts: &self.counts + &rhs.counts,
        }
    }
}

impl AddAssign<&MusicBreakdown> for MusicBreakdown {
    /// Combines the scores from another [`Score`] into `self` (so that `self` now represents the
    /// score generated by `self` and the RHS).
    ///
    /// # Panics
    ///
    /// Panics if the number of [`MusicType`]s in `rhs` is different to that of `self`.
    fn add_assign(&mut self, rhs: &MusicBreakdown) {
        self.score += rhs.score;
        self.counts += &rhs.counts;
    }
}
