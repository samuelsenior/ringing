//! Representation of a [`Composition`] generated by Monument.

use std::{collections::HashMap, hash::Hash, sync::Arc};

use bellframe::{Block, Row, RowBuf};

use crate::{
    atw::{AtwBitmap, AtwTable, PlaceBellRange},
    group::PartHead,
    parameters::{
        CallDisplayStyle, CallIdx, MethodId, MethodIdx, MethodVec, MusicTypeId, Parameters,
    },
    query::Query,
    utils::{
        counts::Counts,
        lengths::{PerPartLength, TotalLength},
    },
};

#[allow(unused_imports)] // Used by doc comments
use crate::{
    parameters::{Method, MusicType},
    Search,
};

/// A [`Composition`] generated by Monument.
#[derive(Debug, Clone)]
pub struct Composition {
    /// Which index in the generation order this composition appeared, starting from 0.
    /// I.e. the first composition will have `generation_index = 0`, second will have `1`, etc.
    pub(crate) generation_number: usize,

    pub(crate) path: Vec<PathElem>,

    pub(crate) length: TotalLength,
    pub(crate) part_head: PartHead,
    /// The total score generated by this composition, accumulated from music, calls, coursing
    /// patterns, etc.
    pub(crate) total_score: f32,
    /// The number of rows generated of each method
    pub(crate) method_counts: Counts,
    pub(crate) atw_bitmap: AtwBitmap,
    /// The number of counts generated of each [`MusicType`]
    pub(crate) music_counts: HashMap<MusicTypeId, usize>,
    pub(crate) contiguous_duffer_lengths: Vec<PerPartLength>,
    pub(crate) total_duffer: TotalLength,

    /// The [`Query`] which generated this [`Composition`]
    // TODO: Remove this dependency, and make everything else calculated on the fly
    pub(crate) query: Arc<Query>,
    // TODO: Store expanded atw data, independently of the [`AtwTable`]?
    pub(crate) atw_table: Arc<AtwTable>,
}

impl Composition {
    /// The number of [`Row`]s in this composition.
    pub fn length(&self) -> usize {
        self.length.as_usize()
    }

    /// Generate a human-friendly [`String`] summarising the calling of this composition.  For
    /// example, [this composition](https://complib.org/composition/87419) would have a
    /// `call_string` of `D[B]BL[W]N[M]SE[sH]NCYW[sH]`.
    pub fn call_string(&self) -> String {
        let needs_brackets = self.query.is_spliced()
            || self.query.call_display_style == CallDisplayStyle::Positional;
        let is_snap_start = self.path[0].start_sub_lead_idx > 0;
        let is_snap_finish = self.path.last().unwrap().end_sub_lead_idx(&self.query) > 0;
        let part_head = self.part_head();

        let mut path_iter = self.path.iter().peekable();

        let mut s = String::new();
        if self.query.call_display_style == CallDisplayStyle::Positional {
            s.push('#');
        }
        s.push_str(if is_snap_start { "<" } else { "" });
        while let Some(path_elem) = path_iter.next() {
            // Method text
            if self.query.is_spliced()
                || self.query.call_display_style == CallDisplayStyle::Positional
            {
                // Add one shorthand for every lead *covered* (not number of lead heads reached)
                //
                // TODO: Deal with half-lead spliced
                let method = &self.query.methods[path_elem.method];
                let num_leads_covered = num_leads_covered(
                    method.lead_len(),
                    path_elem.start_sub_lead_idx,
                    path_elem.length,
                );
                for _ in 0..num_leads_covered {
                    s.push_str(&method.shorthand());
                }
            }
            // Call text
            if let Some(call_idx) = path_elem.call_to_end {
                let call = &self.query.calls[call_idx];

                s.push_str(if needs_brackets { "[" } else { "" });
                // Call position
                match self.query.call_display_style {
                    CallDisplayStyle::CallingPositions(calling_bell) => {
                        let row_after_call = path_iter
                            .peek()
                            .map_or(part_head, |path_elem| &path_elem.start_row);
                        let place_of_calling_bell = row_after_call.place_of(calling_bell).unwrap();
                        let calling_position = &call.calling_positions[place_of_calling_bell];
                        s.push_str(call.short_symbol());
                        s.push_str(calling_position);
                    }
                    // TODO: Compute actual counts for positional calls
                    CallDisplayStyle::Positional => s.push_str(&call.symbol),
                }
                s.push_str(if needs_brackets { "]" } else { "" });
            }
        }
        s.push_str(if is_snap_finish { ">" } else { "" });

        s
    }

    /// The [`Row`] reached at the end of the first part.  If this is a 1-part, then this will be
    /// [`rounds`](Row::is_rounds).
    pub fn part_head(&self) -> &Row {
        self.query.part_head_group.get_row(self.part_head)
    }

    /// Return a [`Block`] containing the [`Row`]s in this composition.  Each [`Row`] is annotated
    /// with a `(method index, index within a lead)` pair.  For example, splicing a lead of Bastow
    /// into Cambridge Major would create a [`Block`] which starts like:
    ///
    /// ```text
    /// Block {
    ///     12345678: (<ID of Bastow>, 0),
    ///     21436587: (<ID of Bastow>, 1),
    ///     21345678: (<ID of Bastow>, 2),
    ///     12436587: (<ID of Bastow>, 3),
    ///     14263857: (<ID of Cambridge>, 0),
    ///     41628375: (<ID of Cambridge>, 1),
    ///     14682735: (<ID of Cambridge>, 2),
    ///     41867253: (<ID of Cambridge>, 3),
    ///     48162735: (<ID of Cambridge>, 4),
    ///        ...
    /// }
    /// ```
    pub fn rows(&self) -> Block<(MethodId, usize)> {
        // Generate plain courses for each method
        let plain_courses = self
            .query
            .methods
            .iter()
            .map(|m| m.plain_course().map_annots(|a| (m.id, a.sub_lead_idx)))
            .collect::<MethodVec<_>>();

        // Generate the first part
        let mut first_part = Block::with_leftover_row(self.query.start_row.clone());
        for elem in &self.path {
            assert_eq!(first_part.leftover_row(), elem.start_row.as_row());
            let plain_course = &plain_courses[elem.method];
            // Add this elem to the first part
            let start_idx = elem.start_sub_lead_idx;
            let end_idx = start_idx + elem.length.as_usize();
            if end_idx > plain_course.len() {
                // `elem` wraps over the course head, so copy it in two pieces
                first_part
                    .extend_range(plain_course, start_idx..)
                    .expect("All path elems should have the same stage");
                first_part
                    .extend_range(plain_course, ..end_idx - plain_course.len())
                    .expect("All path elems should have the same stage");
            } else {
                // `elem` doesn't wrap over the course head, so copy it in one piece
                first_part
                    .extend_range(plain_course, start_idx..end_idx)
                    .expect("All path elems should have the same stage");
            }
            // If this PathElem ends in a call, then change the `leftover_row` to suit
            if let Some(call_idx) = elem.call_to_end {
                let last_non_leftover_row = first_part.rows().next_back().unwrap();
                let new_leftover_row = last_non_leftover_row
                    * self.query.calls[call_idx].place_notation.transposition();
                first_part
                    .leftover_row_mut()
                    .copy_from(&new_leftover_row)
                    .unwrap();
            }
        }

        // Generate the other parts from the first
        let part_len = first_part.len();
        let mut comp = first_part;
        for _ in 0..self.query.num_parts() - 1 {
            comp.extend_from_within(..part_len);
        }
        assert_eq!(comp.len(), self.length());
        assert_eq!(comp.leftover_row(), self.query.end_row.as_row());
        comp
    }

    /// Returns a factor in `0.0..=1.0` where 0.0 means nothing was rung and 1.0 means everything
    /// was rung (i.e. the composition is atw)
    pub fn atw_factor(&self) -> f32 {
        self.atw_table.atw_factor(&self.atw_bitmap)
    }

    pub fn place_bell_ranges_rung(&self) -> Vec<PlaceBellRange> {
        self.atw_table.place_bells_rung(&self.atw_bitmap)
    }

    /// The total score generated by this composition from all the different weights (music, calls,
    /// changes of method, handbell coursing, etc.).
    pub fn total_score(&self) -> f32 {
        self.total_score
    }

    /// The average score generated by each [`Row`] in this composition.  This is equal to
    /// `self.total_score() / self.length() as f32`.
    pub fn average_score(&self) -> f32 {
        self.total_score() / self.length() as f32
    }

    /// Score generated by just the [`MusicType`]s (not including calls, changes of methods,
    /// etc.).
    pub fn music_score(&self) -> f32 {
        self.music_counts
            .iter()
            .map(|(id, count)| self.query.get_music_type_by_id(*id).weight * *count as f32)
            .sum::<f32>()
    }

    /// A slice containing the number of [`Row`]s generated for each [`Method`] used in the
    /// [`Search`].  These are stored in the same order as the [`Method`]s.
    pub fn method_counts(&self) -> &[usize] {
        self.method_counts.as_slice()
    }

    /// The number of *instances* of each [`MusicType`] in the [`Search`].
    pub fn music_counts(&self) -> &HashMap<MusicTypeId, usize> {
        &self.music_counts
    }

    /// Return an [`Iterator`] over the number of [`Row`]s in each transition between
    /// [`non_duffer_courses`](Parameters::non_duffer_courses).
    pub fn contiguous_duffer_lengths(&self) -> impl DoubleEndedIterator<Item = usize> + '_ {
        self.contiguous_duffer_lengths.iter().map(|l| l.as_usize())
    }

    /// Returns the total number of [`Row`]s of duffer courses (i.e. those not in
    /// [`Parameters::non_duffer_courses`]) present in this composition.
    pub fn total_duffer(&self) -> usize {
        self.total_duffer.as_usize()
    }

    pub fn generation_number(&self) -> usize {
        self.generation_number
    }
}

/// A piece of a [`Composition`]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct PathElem {
    pub start_row: RowBuf,
    pub method: MethodIdx,
    pub start_sub_lead_idx: usize,
    pub length: PerPartLength,
    pub call_to_end: Option<CallIdx>,
}

impl PathElem {
    pub fn ends_with_plain(&self) -> bool {
        self.call_to_end.is_none()
    }

    pub(crate) fn end_sub_lead_idx(&self, query: &Query) -> usize {
        query.methods[self.method].add_sub_lead_idx(self.start_sub_lead_idx, self.length)
    }
}

/// A way to display a [`Composition`] by pairing it with a [`Query`]
#[derive(Debug, Clone, Copy)]
struct DisplayComposition<'a>(pub &'a Composition, pub &'a Parameters);

impl std::fmt::Display for DisplayComposition<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let DisplayComposition(comp, query) = self;

        write!(f, "len: {}, ", comp.length)?;
        // Method counts for spliced
        if query.is_spliced() {
            write!(f, "ms: {:>3?}, ", comp.method_counts.as_slice())?;
        }
        // Part heads if multi-part with >2 parts (2-part compositions only have one possible part
        // head)
        if query.num_parts() > 2 {
            write!(f, "PH: {}, ", comp.part_head())?;
        }
        write!(
            f,
            "music: {:>6.2?}, avg score: {:.6}, str: {}",
            comp.music_score(),
            comp.average_score(),
            comp.call_string()
        )
    }
}

///////////
// UTILS //
///////////

/// Return the number of leads covered by some [`Chunk`]
fn num_leads_covered(lead_len: usize, start_sub_lead_idx: usize, length: PerPartLength) -> usize {
    assert_ne!(length, PerPartLength::ZERO); // 0-length chunks shouldn't exist
    let dist_to_end_of_first_lead = lead_len - start_sub_lead_idx;
    let rows_after_end_of_first_lead = length.as_usize().saturating_sub(dist_to_end_of_first_lead);
    // `+ 1` for the first lead
    crate::utils::div_rounding_up(rows_after_end_of_first_lead, lead_len) + 1
}

#[cfg(test)]
mod tests {
    use crate::utils::lengths::PerPartLength;

    #[test]
    fn num_leads_covered() {
        assert_eq!(super::num_leads_covered(32, 0, PerPartLength::new(32)), 1);
        assert_eq!(super::num_leads_covered(32, 2, PerPartLength::new(32)), 2);
        assert_eq!(super::num_leads_covered(32, 2, PerPartLength::new(30)), 1);
        assert_eq!(super::num_leads_covered(32, 0, PerPartLength::new(2)), 1);
        assert_eq!(super::num_leads_covered(32, 16, PerPartLength::new(24)), 2);
    }
}
