//! Monument's search routines, along with the code for interacting with in-progress [`Search`]es.

mod best_first;
mod graph;
mod path;
mod prefix;

use std::{
    convert::TryInto,
    ops::RangeInclusive,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};

use bellframe::Stage;
use sysinfo::SystemExt;

use crate::{
    atw::AtwTable,
    parameters::{MethodId, MusicTypeId, Parameters},
    prove_length::{prove_lengths, RefinedRanges},
    query::Query,
    utils::lengths::{PerPartLength, TotalLength},
    Composition,
};

/// Handle to a search being run by Monument.
///
/// This is used if you want to keep control over searches as they are running, for example
/// [to abort them](Self::signal_abort) or receive [`Update`]s on their [`Progress`].  If you just
/// want to run a (hopefully quick) search, use [`Parameters::run`] or
/// [`Parameters::run_with_config`].  Both of those will deal with handling the [`Search`] for
/// you.
// TODO: Rename all instances from `data` to `search`
#[derive(Debug)]
pub struct Search {
    /* Data */
    query: Arc<Query>,
    config: Config,
    graph: self::graph::Graph,
    atw_table: Arc<AtwTable>,
    refined_ranges: RefinedRanges,
}

impl Search {
    /// Create a new `Search` which generates [`Composition`]s according to the given [`Query`].
    /// This also verifies that the [`Query`] makes sense; if not, an [`Error`](crate::Error)
    /// describing the problem is returned.
    ///
    /// **The returned `Search` won't start until you explicitly call
    /// [`search.run(...)`](Self::run)**.
    pub fn new(params: Parameters, config: Config) -> crate::Result<Self> {
        let query = Query::new(params);

        // Build and optimise the graph
        let (mut source_graph, atw_table) = crate::graph::Graph::unoptimised(&query, &config)?;
        // Prove which lengths are impossible, and use that to refine the length and method count
        // ranges
        let refined_ranges = prove_lengths(&source_graph, &query)?;
        // Reduce the size of the graph to improve the search speed
        source_graph.optimise(&query, &refined_ranges);
        // Create a fast-to-traverse copy of the graph
        let graph = self::graph::Graph::new(&source_graph, &query);
        drop(source_graph);

        Ok(Search {
            query: Arc::new(query),
            atw_table: Arc::new(atw_table),
            config,
            refined_ranges,
            graph,
        })
    }

    /// Runs the search, **blocking the current thread** until either the search is completed or an
    /// [abort is signalled](Self::signal_abort).
    pub fn run(&self, update_fn: impl FnMut(Update), abort_flag: &AtomicBool) {
        // Make sure that `abort_flag` starts as false (so the search doesn't abort immediately).
        // We want this to be sequentially consistent to make sure that the worker threads don't
        // see the previous value (which could be 'true').
        abort_flag.store(false, Ordering::SeqCst);
        best_first::search(self, update_fn, abort_flag);
    }
}

impl Search {
    /// Gets the range of counts required of the given [`MethodId`].
    pub fn method_count_range(&self, id: MethodId) -> RangeInclusive<usize> {
        let idx = self.query.get_method_by_id(id);
        let range = &self.refined_ranges.method_counts[idx];
        range.start().as_usize()..=range.end().as_usize()
    }

    pub fn methods(&self) -> impl Iterator<Item = (&crate::parameters::Method, String)> {
        self.query.methods.iter().map(|m| (&m.inner, m.shorthand()))
    }

    pub fn music_type_ids(&self) -> impl Iterator<Item = MusicTypeId> + '_ {
        self.query.music_types.iter().map(|ty| ty.id)
    }

    pub fn max_music_count(&self, id: MusicTypeId) -> usize {
        self.query
            .get_music_type_by_id(id)
            .max_count()
            .unwrap_or(usize::MAX)
    }

    pub fn parameters(&self) -> &Parameters {
        &self.query.parameters
    }

    pub fn max_consecutive_duffer(&self) -> Option<usize> {
        self.query
            .max_contiguous_duffer
            .map(PerPartLength::as_usize)
    }

    pub fn max_total_duffer(&self) -> Option<usize> {
        self.query.max_total_duffer.map(TotalLength::as_usize)
    }

    pub fn num_parts(&self) -> usize {
        self.query.num_parts()
    }

    /// Does this `Query` generate [`Composition`](crate::Composition)s with more than one part?
    pub fn is_multipart(&self) -> bool {
        self.query.is_multipart()
    }

    /// Gets the [`effective_stage`](bellframe::Row::effective_stage) of the part heads used in
    /// this `Query`.  The short form of every possible part head will be exactly this length.
    pub fn effective_part_head_stage(&self) -> Stage {
        self.query.part_head_group.effective_stage()
    }
}

/// Update message from an in-progress [`Search`].
#[derive(Debug)]
pub enum Update {
    /// A new composition has been found
    Comp(Composition),
    /// A thread is sending a status update
    Progress(Progress),
    /// The search has completed
    Complete,
}

/// How much of a [`Search`] has been completed so far.
#[derive(Debug, Clone, Copy)]
pub struct Progress {
    /// How many times the core composing loop has been run so far.
    pub iter_count: usize,
    /// How many [`Composition`]s have been generated so far.
    pub num_comps: usize,

    /// The current length of the queue of [`Composition`] prefixes waiting to be expanded.
    pub queue_len: usize,
    /// The average length of [`Composition`] prefixes in the queue.
    pub avg_length: f32,
    /// The length of the longest [`Composition`] prefix in the queue.
    pub max_length: usize,

    /// `true` if the search routine is currently shortening the prefix queue to save memory.
    pub truncating_queue: bool,
    /// `true` if the search routine is in the process of aborting
    pub aborting: bool,
}

impl Progress {
    /// [`Progress`] made by a [`Search`] which hasn't started yet.
    pub const START: Self = Self {
        iter_count: 0,
        num_comps: 0,

        queue_len: 0,
        avg_length: 0.0,
        max_length: 0,

        truncating_queue: false,
        aborting: false,
    };
}

/// Configuration options for a [`Search`].
///
/// `Config` *won't* change which compositions are generated, unlike the parameters set by
/// [`Search`]'s builder API.
#[derive(Debug, Clone)]
pub struct Config {
    /* General */
    /// Number of threads used to generate compositions.  If `None`, this uses the number of
    /// **physical** CPU cores (i.e. ignoring hyper-threading).
    pub thread_limit: Option<usize>,

    /* Graph Generation */
    /// The maximum number of chunks in the composition graph.  If a search would produce a graph
    /// bigger than this, it is aborted.  If there was no limit, it would be very easy to cause an
    /// out-of-memory crash by requesting a hugely open query such as split-tenors Maximus.
    pub graph_size_limit: usize,

    /* Search */
    /// The maximum number of bytes of heap memory which the search routine is allowed to use.
    /// Defaults to 80% of available memory.
    pub mem_limit: usize,
    /// If `true`, the data structures used by searches will be leaked using [`std::mem::forget`].
    /// This massively improves the termination speed (because the search creates tons of small
    /// allocations which we now don't need to explicitly free) but only makes sense for the CLI,
    /// where the process will do exactly one search run before terminating (thus returning the memory
    /// to the OS anyway).
    pub leak_search_memory: bool,
}

impl Default for Config {
    fn default() -> Self {
        // Use as a memory limit either 80% of available memory or 5GB if we can't access
        // availability
        let ideal_mem_limit = if sysinfo::System::IS_SUPPORTED {
            (sysinfo::System::new_all().available_memory() as f32 * 0.8) as u64
        } else {
            5_000_000_000u64
        };
        // However, always use 500MB less than the memory that's accessible by the system (i.e. if
        // we're running in 32-bit environments like WASM, we can't fill available memory so we
        // just default to `2*32 - 500MB ~= 3.5GB`)
        let pointer_size_limit = (usize::MAX as u64).saturating_sub(500_000_000);
        let mem_limit: usize = ideal_mem_limit
            .min(pointer_size_limit)
            .try_into()
            .expect("Memory limit should fit into `usize`");

        Self {
            thread_limit: None,

            graph_size_limit: 100_000,

            mem_limit,
            leak_search_memory: false,
        }
    }
}
