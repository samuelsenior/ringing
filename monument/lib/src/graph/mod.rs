//! Creation and manipulation of composition graphs.
//!
//! This implements routines for creating and optimising such graphs, in preparation for performing
//! tree search.

mod build;
mod optimise;

use std::{
    collections::HashMap,
    fmt::{Debug, Display, Formatter},
    ops::Deref,
    sync::Arc,
};

use bellframe::Row;
use datasize::DataSize;

use crate::{
    atw::AtwBitmap,
    group::PhRotation,
    parameters::{CallIdx, MethodIdx},
    utils::{
        counts::Counts,
        lengths::{PerPartLength, TotalLength},
        MusicBreakdown,
    },
};

/// A 'prototype' chunk graph that is (relatively) inefficient to traverse but easy to modify.  This
/// is usually used to build and optimise the chunk graph before being converted into an efficient
/// graph representation for use in tree search.
#[derive(Debug, Clone)]
pub struct Graph {
    // NOTE: References between chunks don't have to be valid (i.e. they can point to a [`Chunk`]
    // that isn't actually in the graph - in this case they will be ignored or discarded during the
    // optimisation process).
    pub(crate) chunks: HashMap<ChunkId, Chunk>,
    pub(crate) links: LinkSet,

    /// Lookup table for the [`Link`]s which can start a composition, along with the [`ChunkId`]s
    /// that they lead to
    ///
    /// **Invariant**: for each `(link_id, chunk_id)` in `self.starts`:
    /// - `self.links[link_id].from == LinkSide::StartOrEnd`
    /// - `self.links[link_id].to == LinkSide::Chunk(chunk_id)`
    pub(crate) starts: Vec<(LinkId, ChunkId)>,
    /// Lookup table for the [`Link`]s which can end a composition, along with the [`ChunkId`]s
    /// which lead to them
    ///
    /// **Invariant**: for each `(link_id, chunk_id)` in `self.ends`:
    /// - `self.links[link_id].from == LinkSide::Chunk(chunk_id)`
    /// - `self.links[link_id].to == LinkSide::StartOrEnd`
    pub(crate) ends: Vec<(LinkId, ChunkId)>,
}

/// A `Chunk` in a chunk [`Graph`].  This is an indivisible chunk of ringing which cannot be split
/// up by calls or splices.
#[derive(Debug, Clone)]
pub(crate) struct Chunk {
    pub(crate) predecessors: Vec<LinkId>,
    pub(crate) successors: Vec<LinkId>,

    /// The chunks which share rows with `self`, including `self` (because all chunks are false
    /// against themselves).  Optimisation passes probably shouldn't mess with falseness.
    pub(crate) false_chunks: Vec<ChunkId>,

    /// The number of rows in the range covered by this chunk (i.e. its length in one part of the
    /// composition)
    pub(crate) per_part_length: PerPartLength,
    /// The number of rows that this this chunk adds to the composition (its total length across all
    /// parts).  Optimisation passes can't change this
    pub(crate) total_length: TotalLength,
    /// The number of rows of each method generated by this chunk
    pub(crate) method_counts: Counts,
    /// The music generated by this chunk in the composition.  Optimisation passes can't change this
    pub(crate) music: MusicBreakdown,
    /// An [`AtwBitmap`] storing which sections of methods have been rung in this chunk
    pub(crate) atw_bitmap: AtwBitmap,

    /// Does this chunk need to be included in every composition in this search?
    pub(crate) required: bool,

    /// A lower bound on the number of rows required to go from any rounds to the first row of
    /// `self`
    pub(crate) lb_distance_from_rounds: TotalLength,
    /// A lower bound on the number of rows required to go from the first row **after** `self` to
    /// rounds.
    pub(crate) lb_distance_to_rounds: TotalLength,

    /// True if `self` is considered a 'duffer'
    pub(crate) duffer: bool,
    /// A lower bound on the number of [`Row`]s required to go from any non-duffer course to the
    /// first row of `self`.
    pub(crate) lb_distance_from_non_duffer: PerPartLength,
    /// A lower bound on the number of rows required to go from the first row **after** `self` to
    /// the closest non-duffer course.
    pub(crate) lb_distance_to_non_duffer: PerPartLength,
}

/// A link between two [`Chunk`]s in a [`Graph`]
#[derive(Debug, Clone, Eq, PartialEq, Hash)]
pub(crate) struct Link {
    pub from: LinkSide<ChunkId>,
    pub to: LinkSide<ChunkId>,
    /// Indexes into [`crate::Query::calls`]
    pub call: Option<CallIdx>,
    pub ph_rotation: PhRotation,
    // TODO: Remove this and compute it on the fly for `LinkView`?
    pub ph_rotation_back: PhRotation,
}

impl Link {
    pub fn is_start(&self) -> bool {
        self.from.is_start_or_end()
    }

    pub fn is_end(&self) -> bool {
        self.to.is_start_or_end()
    }
}

/// What a `Link` points to.  This is either a [`StartOrEnd`](Self::StartOrEnd), or a specific
/// [`Chunk`](Self::Chunk).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, DataSize)]
pub(crate) enum LinkSide<Id> {
    StartOrEnd,
    Chunk(Id),
}

impl<Id> LinkSide<Id> {
    pub fn is_start_or_end(&self) -> bool {
        matches!(self, Self::StartOrEnd)
    }

    pub fn as_ref(&self) -> LinkSide<&Id> {
        match self {
            Self::StartOrEnd => LinkSide::StartOrEnd,
            Self::Chunk(c) => LinkSide::Chunk(c),
        }
    }
}

// ------------------------------------------------------------------------------------------

impl Chunk {
    /// An [`Iterator`] over only valid predecessor [`Link`]s
    pub(crate) fn pred_links<'g>(
        &'g self,
        graph: &'g Graph,
    ) -> impl Iterator<Item = (LinkId, &'g Link)> {
        self.predecessors
            .iter()
            .filter_map(|&id| graph.links.get(id).map(|l| (id, l)))
    }

    /// An [`Iterator`] over only valid successor [`Link`]s
    pub(crate) fn succ_links<'g>(
        &'g self,
        graph: &'g Graph,
    ) -> impl Iterator<Item = (LinkId, &'g Link)> {
        self.successors
            .iter()
            .filter_map(|&id| graph.links.get(id).map(|l| (id, l)))
    }
}

////////////////////////
// UTILITY DATA TYPES //
////////////////////////

/// The unique identifier of a [`Chunk`] within a given [`Graph`].
#[derive(Clone, Eq, PartialEq, PartialOrd, Ord, Hash)]
pub(crate) struct ChunkId {
    pub lead_head: Arc<Row>, // `Arc` is used to make cloning cheaper
    pub row_idx: RowIdx,
}

impl ChunkId {
    pub fn new(lead_head: Arc<Row>, row_idx: RowIdx) -> Self {
        Self { lead_head, row_idx }
    }
}

impl Deref for ChunkId {
    type Target = RowIdx;

    fn deref(&self) -> &Self::Target {
        &self.row_idx
    }
}

impl Debug for ChunkId {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "ChunkId({})", self)
    }
}

impl Display for ChunkId {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{},{:?}:{}",
            self.lead_head, self.method, self.sub_lead_idx,
        )?;
        Ok(())
    }
}

/// The unique index of a [`Row`] within a lead.
// TODO: Merge this into `ChunkId`?
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct RowIdx {
    pub method: MethodIdx,
    pub sub_lead_idx: usize,
}

impl RowIdx {
    pub fn new(method_idx: MethodIdx, sub_lead_idx: usize) -> Self {
        Self {
            method: method_idx,
            sub_lead_idx,
        }
    }
}

//////////////
// LINK SET //
//////////////

pub(crate) use link_set::{LinkId, LinkSet};

mod link_set {
    use std::collections::HashMap;

    use super::Link;

    /// Unique identifier for a [`Link`] within a [`Graph`]
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub(crate) struct LinkId(usize);

    /// A [`HashMap`] containing a set of [`Link`]s, all addressed by unique [`LinkId`]s
    #[derive(Debug, Clone, Default)]
    pub(crate) struct LinkSet {
        next_id: usize,
        map: HashMap<LinkId, Link>,
    }

    impl LinkSet {
        /// Create a [`LinkSet`] containing no [`Link`]s
        pub fn new() -> Self {
            Self::default()
        }

        /// Add a new [`Link`] to this set, returning its [`LinkId`]
        pub fn add(&mut self, link: Link) -> LinkId {
            let id = self.next_id();
            self.map.insert(id, link);
            id
        }

        pub fn len(&self) -> usize {
            self.map.len()
        }

        pub fn get(&self, id: LinkId) -> Option<&Link> {
            self.map.get(&id)
        }

        pub fn contains(&self, id: LinkId) -> bool {
            self.map.contains_key(&id)
        }

        pub fn iter(&self) -> std::collections::hash_map::Iter<LinkId, Link> {
            self.map.iter()
        }

        #[allow(dead_code)] // Don't want `values` without `keys`
        pub fn keys(&self) -> std::iter::Copied<std::collections::hash_map::Keys<LinkId, Link>> {
            self.map.keys().copied()
        }

        pub fn values(&self) -> std::collections::hash_map::Values<LinkId, Link> {
            self.map.values()
        }

        /// Remove any [`Link`]s from `self` which don't satisfy a given predicate
        pub fn retain(&mut self, mut pred: impl FnMut(LinkId, &mut Link) -> bool) {
            self.map.retain(|id, link| pred(*id, link))
        }

        /// Get a new [`LinkId`], unique from all the others returned from `self`
        fn next_id(&mut self) -> LinkId {
            let id = LinkId(self.next_id);
            self.next_id += 1;
            id
        }
    }

    impl std::ops::Index<LinkId> for LinkSet {
        type Output = Link;

        #[track_caller]
        fn index(&self, index: LinkId) -> &Self::Output {
            &self.map[&index]
        }
    }
}
