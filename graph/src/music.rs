use std::ops::{Add, AddAssign, Sub, SubAssign};

use bellframe::{music::Regex, Row, RowBuf, Stage};
use itertools::Itertools;
use monument_utils::OptRange;
use ordered_float::OrderedFloat;

pub type Score = OrderedFloat<f32>;

/// A class of music that Monument should care about
#[derive(Debug, Clone)]
pub struct MusicType {
    regexes: Vec<Regex>,
    weight: Score,
    pub(crate) count_range: OptRange,
}

impl MusicType {
    pub fn new(regexes: Vec<Regex>, weight: f32, count_range: OptRange) -> Self {
        Self {
            regexes,
            weight: OrderedFloat(weight),
            count_range,
        }
    }

    /// Compute the score of a sequence of [`Row`]s
    pub fn score<'r>(&self, rows: impl IntoIterator<Item = &'r Row>) -> Score {
        let mut num_matches = 0usize;
        for row in rows.into_iter() {
            for regex in &self.regexes {
                if regex.matches(row) {
                    num_matches += 1;
                }
            }
        }
        // Give each match a score of `self.weight`
        Score::from(num_matches as f32) * self.weight
    }
}

/// A breakdown of the music generated by a composition
#[derive(Debug, Clone)]
pub struct Breakdown {
    pub score: Score,
    /// The number of occurrences of each [`MusicType`] (the list of music types is stored in the
    /// [`Engine`] singleton).
    pub counts: Vec<usize>,
}

impl Breakdown {
    /// Creates the `Score` of 0 (i.e. the `Score` generated by no rows).
    pub fn zero(num_music_types: usize) -> Self {
        Self {
            score: Score::from(0.0),
            counts: vec![0; num_music_types],
        }
    }

    /// Returns the `Score` generated by a sequence of [`Row`]s, (pre-)transposed by some course head.
    pub fn from_rows<'r>(
        rows: impl IntoIterator<Item = &'r Row>,
        course_head: &Row,
        music_types: &[MusicType],
    ) -> Self {
        let mut temp_row = RowBuf::rounds(Stage::ONE);
        let mut occurences = vec![0; music_types.len()];
        // For every (transposed) row ...
        for r in rows {
            course_head.mul_into(r, &mut temp_row).unwrap();
            // ... for every music type ...
            for (num_instances, ty) in occurences.iter_mut().zip_eq(music_types) {
                // ... count the number of instances of that type of music
                for regex in &ty.regexes {
                    if regex.matches(&temp_row) {
                        *num_instances += 1;
                    }
                }
            }
        }

        Self {
            score: occurences
                .iter()
                .zip_eq(music_types)
                .map(|(&num_instances, ty)| Score::from(num_instances as f32) * ty.weight)
                .sum(),
            counts: occurences,
        }
    }

    /// # Panics
    ///
    /// Panics if the number of [`MusicType`]s in `rhs` is different to that of `self`.
    pub fn saturating_sub(&self, rhs: &Self) -> Self {
        Breakdown {
            score: self.score - rhs.score,
            counts: self
                .counts
                .iter()
                .zip_eq(rhs.counts.iter())
                .map(|(a, b)| a.saturating_sub(*b))
                .collect_vec(),
        }
    }

    /// # Panics
    ///
    /// Panics if the number of [`MusicType`]s in `rhs` is different to that of `self`.
    pub fn saturating_sub_assign(&mut self, rhs: &Self) {
        self.score -= rhs.score;
        for (a, b) in self.counts.iter_mut().zip_eq(rhs.counts.iter()) {
            *a = a.saturating_sub(*b);
        }
    }
}

impl Add for &Breakdown {
    type Output = Breakdown;

    /// Combines two [`Score`]s to create one [`Score`] representing both `self` and `rhs`.
    ///
    /// # Panics
    ///
    /// Panics if the number of [`MusicType`]s in `rhs` is different to that of `self`.
    fn add(self, rhs: &Breakdown) -> Self::Output {
        Breakdown {
            score: self.score + rhs.score,
            counts: self
                .counts
                .iter()
                .zip_eq(rhs.counts.iter())
                .map(|(a, b)| a + b)
                .collect_vec(),
        }
    }
}

impl AddAssign<&Breakdown> for Breakdown {
    /// Combines the scores from another [`Score`] into `self` (so that `self` now represents the
    /// score generated by `self` and the RHS).
    ///
    /// # Panics
    ///
    /// Panics if the number of [`MusicType`]s in `rhs` is different to that of `self`.
    fn add_assign(&mut self, rhs: &Breakdown) {
        self.score += rhs.score;
        for (a, b) in self.counts.iter_mut().zip_eq(rhs.counts.iter()) {
            *a += *b;
        }
    }
}

impl Sub for &Breakdown {
    type Output = Breakdown;

    /// Combines two [`Score`]s to create one [`Score`] representing both `self` and `rhs`.
    ///
    /// # Panics
    ///
    /// Panics if the number of [`MusicType`]s in `rhs` is different to that of `self`.
    fn sub(self, rhs: &Breakdown) -> Self::Output {
        Breakdown {
            score: self.score - rhs.score,
            counts: self
                .counts
                .iter()
                .zip_eq(rhs.counts.iter())
                .map(|(a, b)| a - b)
                .collect_vec(),
        }
    }
}

impl SubAssign<&Breakdown> for Breakdown {
    /// Combines the scores from another [`Score`] into `self` (so that `self` now represents the
    /// score generated by `self` and the RHS).
    ///
    /// # Panics
    ///
    /// Panics if the number of [`MusicType`]s in `rhs` is different to that of `self`.
    fn sub_assign(&mut self, rhs: &Breakdown) {
        self.score -= rhs.score;
        for (a, b) in self.counts.iter_mut().zip_eq(rhs.counts.iter()) {
            *a -= *b;
        }
    }
}
