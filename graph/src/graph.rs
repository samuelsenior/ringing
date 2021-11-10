//! A mutable graph of nodes.  Compositions are represented as paths through this node graph.

use std::{
    cmp::{Ordering, Reverse},
    collections::{BinaryHeap, HashMap},
};

use bellframe::{IncompatibleStages, RowBuf};
use itertools::Itertools;
use log::log;
use monument_utils::FrontierItem;

use crate::{
    falseness::FalsenessTable,
    layout::{End, Layout, LinkIdx, NodeId, Segment, StandardNodeId, StartIdx},
    music::{Breakdown, MusicType, Score},
    optimise::Pass,
    row_counts::RowCounts,
    Data,
};

/// The number of rows required to get from a point in the graph to a start/end.
type Distance = usize;

/// Measure which determines which part head has been reached.  Each node link is given a
/// `Rotation` which, when summed modulo [`Graph::num_parts`], will determine which part head has
/// been reached (and therefore whether the composition is valid).
pub type Rotation = u16;

/// A 'prototype' node graph that is (relatively) inefficient to traverse but easy to modify.  This
/// is usually used to build and optimise the node graph before being converted into an efficient
/// graph representation for use in tree search.
#[derive(Debug, Clone)]
pub struct Graph {
    // NOTE: References between nodes don't have to be valid (i.e. they can point to a [`Node`]
    // that isn't actually in the graph - in this case they will be ignored or discarded during the
    // optimisation process).
    nodes: HashMap<NodeId, Node>,
    /// **Invariant**: If `start_nodes` points to a node, it **must** be a start node (i.e. not
    /// have any predecessors, and have `start_label` set)
    start_nodes: Vec<(NodeId, StartIdx)>,
    /// **Invariant**: If `start_nodes` points to a node, it **must** be a end node (i.e. not have
    /// any successors, and have `end_nodes` set)
    end_nodes: Vec<(NodeId, End)>,
    /// The number of different parts
    num_parts: usize,
}

/// A `Node` in a node [`Graph`].  This is an indivisible chunk of ringing which cannot be split up
/// by calls or splices.
#[derive(Debug, Clone)]
pub struct Node {
    /// If this `Node` is a 'start' (i.e. it can be the first node in a composition), then this is
    /// `Some(label)` where `label` should be appended to the front of the human-friendly
    /// composition string.
    is_start: bool,
    /// If this `Node` is an 'end' (i.e. adding it will complete a composition), then this is
    /// `Some(label)` where `label` should be appended to the human-friendly composition string.
    end: Option<End>,
    /// The string that should be added when this node is generated
    label: String,

    successors: Vec<Link>,
    predecessors: Vec<Link>,

    /// The nodes which share rows with `self`, including `self` (because all nodes are false
    /// against themselves).  Optimisation passes probably shouldn't mess with falseness.
    false_nodes: Vec<StandardNodeId>,

    /// The number of rows in this node.  Optimisation passes can't change this
    length: usize,
    /// The number of rows of each method generated by this node
    method_counts: RowCounts,
    /// The music generated by this node in the composition.  Optimisation passes can't change this
    music: Breakdown,

    /* MUTABLE STATE FOR OPTIMISATION PASSES */
    /// Does this node need to be included in every composition in this search?
    pub required: bool,
    /// A lower bound on the number of rows required to go from any rounds to the first row of
    /// `self`
    pub lb_distance_from_rounds: Distance,
    /// A lower bound on the number of rows required to go from the first row **after** `self` to
    /// rounds.
    pub lb_distance_to_rounds: Distance,
}

#[derive(Debug, Clone, Eq, PartialEq, Hash)]
pub struct Link {
    pub id: NodeId,
    /// Indexes into `Layout::links`
    pub source_idx: LinkIdx,
    pub rotation: Rotation,
}

impl Link {
    pub fn new(id: NodeId, source_idx: LinkIdx, rotation: Rotation) -> Self {
        Self {
            id,
            source_idx,
            rotation,
        }
    }
}

// ------------------------------------------------------------------------------------------

impl Graph {
    //! Optimisation

    /// Repeatedly apply a sequence of [`Pass`]es until the graph stops getting smaller, or 20
    /// iterations are made.  Use [`Graph::optimise_with_iter_limit`] to set a custom iteration limit.
    pub fn optimise(&mut self, passes: &mut [Pass], data: &Data) {
        self.optimise_with_iter_limit(passes, data, 20);
    }

    /// Repeatedly apply a sequence of [`Pass`]es until the graph either becomes static, or `limit`
    /// many iterations are performed.
    pub fn optimise_with_iter_limit(&mut self, passes: &mut [Pass], data: &Data, limit: usize) {
        let mut last_size = Size::from(&*self);

        for _ in 0..limit {
            self.run_passes(passes, data);

            let new_size = Size::from(&*self);
            // Stop optimising if the optimisations don't make the graph strictly smaller.  If
            // they make some parts smaller but others larger, then keep optimising.
            match new_size.partial_cmp(&last_size) {
                Some(Ordering::Equal | Ordering::Greater) => return,
                Some(Ordering::Less) | None => {}
            }
            last_size = new_size;
        }
    }

    /// Run a sequence of [`Pass`]es on `self`
    pub fn run_passes(&mut self, passes: &mut [Pass], data: &Data) {
        for p in &mut *passes {
            p.run(self, data);
        }
    }

    /// For each start node in `self`, creates a copy of `self` with _only_ that start node.  This
    /// partitions the set of generated compositions across these `Graph`s, but allows for better
    /// optimisations because more is known about each `Graph`.
    pub fn split_by_start_node(&self) -> Vec<Graph> {
        self.start_nodes
            .iter()
            .cloned()
            .map(|start_id| {
                let mut new_self = self.clone();
                new_self.start_nodes = vec![start_id];
                new_self
            })
            .collect_vec()
    }

    pub fn num_parts(&self) -> usize {
        self.num_parts
    }
}

// ------------------------------------------------------------------------------------------

impl Graph {
    //! Helpers for optimisation passes

    /// Removes all nodes for whom `pred` returns `false`
    pub fn retain_nodes(&mut self, pred: impl FnMut(&NodeId, &mut Node) -> bool) {
        self.nodes.retain(pred);
    }

    /// Remove elements from [`Self::start_nodes`] for which a predicate returns `false`.
    pub fn retain_start_nodes(&mut self, pred: impl FnMut(&(NodeId, StartIdx)) -> bool) {
        self.start_nodes.retain(pred);
    }

    /// Remove elements from [`Self::end_nodes`] for which a predicate returns `false`.
    pub fn retain_end_nodes(&mut self, pred: impl FnMut(&(NodeId, End)) -> bool) {
        self.end_nodes.retain(pred);
    }
}

impl Node {
    //! Helpers for optimisation passes

    /// A lower bound on the length of a composition which passes through this node.
    pub fn min_comp_length(&self) -> usize {
        self.lb_distance_from_rounds + self.length + self.lb_distance_to_rounds
    }
}

/// A measure of the `Size` of a [`Graph`].  Used to detect when further optimisations aren't
/// useful.
#[derive(Debug, PartialEq, Clone, Copy)]
struct Size {
    num_nodes: usize,
    num_links: usize,
    num_starts: usize,
    num_ends: usize,
}

impl From<&Graph> for Size {
    fn from(g: &Graph) -> Self {
        Self {
            num_nodes: g.nodes.len(),
            // This assumes that every successor link also corresponds to a predecessor link
            num_links: g.nodes().map(|(_id, node)| node.successors.len()).sum(),
            num_starts: g.start_nodes.len(),
            num_ends: g.end_nodes.len(),
        }
    }
}

impl PartialOrd for Size {
    // TODO: Make this into a macro?
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        let cmp_nodes = self.num_nodes.cmp(&other.num_nodes);
        let cmp_links = self.num_links.cmp(&other.num_links);
        let cmp_starts = self.num_starts.cmp(&other.num_starts);
        let cmp_ends = self.num_ends.cmp(&other.num_ends);

        let all_comparisons = [cmp_nodes, cmp_links, cmp_starts, cmp_ends];

        let are_any_less = all_comparisons
            .iter()
            .any(|cmp| matches!(cmp, Ordering::Less));
        let are_any_greater = all_comparisons
            .iter()
            .any(|cmp| matches!(cmp, Ordering::Greater));

        match (are_any_less, are_any_greater) {
            (true, false) => Some(Ordering::Less), // If nothing got larger, then the size is smaller
            (false, true) => Some(Ordering::Greater), // If nothing got smaller, then the size is larger
            (false, false) => Some(Ordering::Equal),  // No < or > means all components are equal
            (true, true) => None, // If some are smaller & some are greater then these are incomparable
        }
    }
}

// ------------------------------------------------------------------------------------------

impl Graph {
    //! Getters & Iterators

    // Getters

    pub fn get_node<'graph>(&'graph self, id: &NodeId) -> Option<&'graph Node> {
        self.nodes.get(id)
    }

    pub fn get_node_mut<'graph>(&'graph mut self, id: &NodeId) -> Option<&'graph mut Node> {
        self.nodes.get_mut(id)
    }

    pub fn start_nodes(&self) -> &[(NodeId, StartIdx)] {
        &self.start_nodes
    }

    pub fn end_nodes(&self) -> &[(NodeId, End)] {
        &self.end_nodes
    }

    pub fn node_map(&self) -> &HashMap<NodeId, Node> {
        &self.nodes
    }

    pub fn get_start(&self, idx: usize) -> Option<(&Node, StartIdx)> {
        let (start_node_id, start_idx) = self.start_nodes.get(idx)?;
        let start_node = self.nodes.get(start_node_id)?;
        assert!(start_node.is_start);
        Some((start_node, *start_idx))
    }

    // Iterators

    /// An [`Iterator`] over the [`NodeId`] of every [`Node`] in this `Graph`
    pub fn ids(&self) -> impl Iterator<Item = &NodeId> {
        self.nodes.keys()
    }

    /// An [`Iterator`] over every [`Node`] in this `Graph` (including its [`NodeId`])
    pub fn nodes(&self) -> impl Iterator<Item = (&NodeId, &Node)> {
        self.nodes.iter()
    }

    /// An [`Iterator`] over every [`Node`] in this `Graph`, without its [`NodeId`].
    pub fn just_nodes(&self) -> impl Iterator<Item = &Node> {
        self.nodes.values()
    }

    /// A mutable [`Iterator`] over the [`NodeId`] of every [`Node`] in this `Graph`
    pub fn nodes_mut(&mut self) -> impl Iterator<Item = (&NodeId, &mut Node)> {
        self.nodes.iter_mut()
    }
}

impl Node {
    //! Getters & Iterators

    pub fn length(&self) -> usize {
        self.length
    }

    pub fn method_counts(&self) -> &RowCounts {
        &self.method_counts
    }

    pub fn score(&self) -> Score {
        self.music.total
    }

    pub fn label(&self) -> &str {
        self.label.as_str()
    }

    // STARTS/ENDS //

    pub fn is_start(&self) -> bool {
        self.is_start
    }

    pub fn end(&self) -> Option<End> {
        self.end
    }

    pub fn is_end(&self) -> bool {
        self.end.is_some()
    }

    // CROSS-NODE REFERENCES //

    pub fn successors(&self) -> &[Link] {
        self.successors.as_slice()
    }

    pub fn successors_mut(&mut self) -> &mut Vec<Link> {
        &mut self.successors
    }

    pub fn predecessors(&self) -> &[Link] {
        self.predecessors.as_slice()
    }

    pub fn predecessors_mut(&mut self) -> &mut Vec<Link> {
        &mut self.predecessors
    }

    pub fn false_nodes(&self) -> &[StandardNodeId] {
        self.false_nodes.as_slice()
    }

    pub fn false_nodes_mut(&mut self) -> &mut Vec<StandardNodeId> {
        &mut self.false_nodes
    }
}

////////////////////////////////
// LAYOUT -> GRAPH CONVERSION //
////////////////////////////////

impl Graph {
    /// Generate a graph of all nodes which are reachable within a given length constraint.
    pub fn from_layout(layout: &Layout, music_types: &[MusicType], max_length: usize) -> Self {
        log::info!("Building `Graph`");

        // The set of reachable nodes and whether or not they are a start node (each mapping to a
        // distance from rounds)
        let mut expanded_nodes: HashMap<NodeId, (Segment, Distance)> = HashMap::new();

        let mut end_nodes = Vec::new();

        // Unexplored nodes, ordered by distance from rounds (i.e. the minimum number of rows required
        // to reach them from rounds)
        let mut frontier: BinaryHeap<Reverse<FrontierItem<NodeId>>> = BinaryHeap::new();

        /* Run Dijkstra's algorithm using comp length as edge weights */

        // Populate the frontier with all the possible start nodes, each with distance 0
        let start_nodes = layout
            .starts
            .iter()
            .enumerate()
            .map(|(idx, start)| {
                let id = NodeId::standard(start.course_head.to_arc(), start.row_idx, true);
                (id, StartIdx::new(idx))
            })
            .collect_vec();
        frontier.extend(
            start_nodes
                .iter()
                .cloned()
                .map(|(id, _)| FrontierItem::new(id))
                .map(Reverse),
        );

        while let Some(Reverse(FrontierItem {
            item: node_id,
            distance,
        })) = frontier.pop()
        {
            // Don't expand nodes multiple times (Dijkstra's algorithm makes sure that the first time
            // it is expanded will be have the shortest distance)
            if expanded_nodes.get(&node_id).is_some() {
                continue;
            }
            // If the node hasn't been expanded yet, then add its reachable nodes to the frontier
            let segment = layout
                .get_segment(&node_id)
                .expect("Infinite segment found");

            // If the shortest composition including this node is longer the length limit, then don't
            // include it in the node graph
            let new_dist = distance + segment.length;
            if new_dist > max_length {
                continue;
            }
            if let Some(end) = segment.end {
                end_nodes.push((node_id.clone(), end));
            }
            // Expand the node by adding its successors to the frontier
            for (_link_idx, id_after_link) in &segment.links {
                // Add the new node to the frontier
                frontier.push(Reverse(FrontierItem {
                    item: id_after_link.to_owned(),
                    distance: new_dist,
                }));
            }
            // Mark this node as expanded
            expanded_nodes.insert(node_id, (segment, distance));
        }

        // Once Dijkstra's finishes, `expanded_nodes` contains every node reachable from rounds
        // within the length of the composition.  However, we're still not done because we have to
        // build a graph over these IDs (which requires computing falseness, music, connections,
        // etc.).
        let rounds = RowBuf::rounds(layout.stage);
        let mut nodes: HashMap<NodeId, Node> = expanded_nodes
            .iter()
            .map(|(node_id, (segment, distance))| {
                assert_eq!(node_id, &segment.node_id);

                let music = Breakdown::from_rows(
                    segment.untransposed_rows(layout),
                    node_id.course_head().unwrap_or(&rounds),
                    music_types,
                );

                let new_node = Node {
                    length: segment.length,
                    method_counts: segment.method_counts.clone(),
                    music,

                    is_start: node_id.is_start(),
                    end: segment.end,
                    label: segment.label.clone(),

                    required: false,
                    lb_distance_from_rounds: *distance,
                    // Distances to rounds are computed later.  However, the distance is an lower
                    // bound, so we can set it to 0 without breaking any invariants.
                    lb_distance_to_rounds: 0,

                    successors: segment
                        .links
                        .iter()
                        .map(|(idx, id)| Link::new(id.clone(), *idx, 0)) // No part heads to rotate
                        .collect_vec(),

                    // These are populated in separate passes once all the `Node`s have been created
                    false_nodes: Vec::new(),
                    predecessors: Vec::new(),
                };
                (node_id.clone(), new_node)
            })
            .collect();

        // We need to clone the `NodeId`s, because otherwise they would borrow from `nodes` whilst
        // the loop is modifying the contents (i.e. breaking reference aliasing)
        let node_ids_and_lengths = nodes
            .iter()
            .map(|(id, node)| (id.to_owned(), node.length))
            .collect_vec();

        // Compute falseness between the nodes
        log::info!(
            "Graph has {:?} nodes, with {:?} starts and {:?} ends.",
            nodes.len(),
            start_nodes.len(),
            end_nodes.len()
        );
        log::debug!("Building falseness table");
        let table = FalsenessTable::from_layout(layout, &node_ids_and_lengths);
        log::trace!("Table: {:#?}", table);
        log::debug!("Setting falseness links");
        for (id, node) in nodes.iter_mut() {
            node.false_nodes = node_ids_and_lengths
                .iter()
                .filter(|(id2, length2)| table.are_false(id, node.length, id2, *length2))
                .map(|(id2, _)| id2.to_owned())
                .filter_map(|id| id.into_std_id())
                .collect_vec();
        }

        // Add predecessor references (every node is a predecessor to all of its successors)
        log::debug!("Setting predecessor links");
        for (id, _dist) in expanded_nodes {
            for succ_link in nodes.get(&id).unwrap().successors.clone() {
                assert_eq!(succ_link.rotation, 0);
                if let Some(node) = nodes.get_mut(&succ_link.id) {
                    node.predecessors.push(Link {
                        id: id.clone(),
                        ..succ_link
                    });
                }
            }
        }

        Self {
            nodes,
            start_nodes,
            end_nodes,
            num_parts: 1,
        }
    }
}

///////////////////////////
// MULTI-PART CONVERSION //
///////////////////////////

impl Graph {
    pub fn to_multipart(&self, data: &Data) -> Result<Self, IncompatibleStages> {
        assert_eq!(self.num_parts, 1); // Giving an already multipart graph more parts is impossible

        if data.part_head.is_rounds() {
            return Ok(self.clone()); // If the part head is rounds (=> a 1-part), no work is required
        }

        let part_heads = data.part_head.closure_from_rounds();
        // Assign each NodeId in `self` to an equivalence class
        let class_by_id = self.class_by_id(&part_heads)?;

        // Invert `class_by_id` into a set of equivalence classes - i.e. we're mapping a
        // `HashMap<SourceId, (EquivId, Rot)>` into a `HashMap<EquivId, Vec<(SourceId, Rot)>>`.
        // Each value in this table is an equivalence class of nodes, and corresponds to one node
        // in the new graph.
        let mut equiv_classes: HashMap<NodeId, Vec<Equiv>> = HashMap::new();
        for (source_id, equiv) in &class_by_id {
            equiv_classes
                .entry(equiv.id.clone())
                .or_insert_with(Vec::new)
                .push(Equiv {
                    id: source_id.clone(),
                    ..equiv.clone()
                });
        }

        // TODO: Remove any equivalence classes which are false against themselves in a different
        // part

        Ok(self.build_multipart(&part_heads, &class_by_id, &equiv_classes, data))
    }

    /// For each [`NodeId`] in the source graph, determine which equivalence class Id and rotation
    /// to which it corresponds.  Any source [`NodeId`]s which aren't keys in this [`HashMap`] will
    /// not be included in the multi-part graph.
    fn class_by_id(
        &self,
        part_heads: &[RowBuf],
    ) -> Result<HashMap<NodeId, Equiv>, IncompatibleStages> {
        // Maps each **source** `NodeId` to (`NodeId` in multi-part graph, rotation).  Explicitly
        // mapping to `None` means that a `NodeId` is known to be in the source graph but should be
        // removed in the multi-part case (e.g. snap-finish nodes in graphs where there are no
        // start finishes).
        let mut class_by_id: HashMap<NodeId, Option<Equiv>> = HashMap::new();

        // In order for multi-parts to work, the set of end locations must be equal to the start
        // locations.  Therefore, wherever there's a start node, we create an equivalent class of
        // end nodes (where the 0-rotation `NodeId` corresponds to a 0-length end).
        for (id, _idx) in &self.start_nodes {
            for (rot, ph) in part_heads.iter().enumerate() {
                let mut new_id = id.pre_multiply(ph).unwrap();

                let mut tag = EquivTag::Std;
                if rot != 0 {
                    new_id.set_start(false);
                    tag = EquivTag::PartHead;
                }
                class_by_id.insert(new_id, Some(Equiv::new(tag, id.clone(), rot as Rotation)));
            }
        }
        for (id, end) in &self.end_nodes {
            if *end != End::ZeroLength {
                // Any non-zero length end node has been truncated at rounds, so its entire
                // equivalence class should be removed from the graph.  TODO: Handle cases where we
                // actually want these nodes (e.g. far calls Bristol)
                for ph in part_heads {
                    class_by_id.insert(id.pre_multiply(ph)?, None);
                }
            }
        }
        // Map any source 0-length end node to a 0-rotation 0-length end node (because rounds
        // becomes the 0th part head)
        #[allow(clippy::unnecessary_cast)]
        class_by_id.insert(
            NodeId::ZeroLengthEnd,
            Some(Equiv::std(NodeId::ZeroLengthEnd, 0 as Rotation)),
        );

        // Now that starts/ends have been handled as special cases, we can group all the nodes into
        // their equivalence classes
        for id in self.ids() {
            if class_by_id.contains_key(id) {
                continue; // Don't add equiv classes for nodes which have already been assigned one
            }
            // Add this node's equivalence class, arbitrarily defining itself to have rotation 0
            for (rot, ph) in part_heads.iter().enumerate() {
                class_by_id.insert(
                    id.pre_multiply(ph)?,
                    Some(Equiv::std(id.clone(), rot as Rotation)),
                );
            }
        }
        // Filter the map to only include entries which map to `Some((id, rot))`
        Ok(class_by_id
            .into_iter()
            .filter_map(|(k, maybe_v)| maybe_v.map(|v| (k, v)))
            .collect())
    }

    /// Construct a new [`Graph`] with one [`Node`] per equivalence class modulo the part heads
    fn build_multipart(
        &self,
        part_heads: &[RowBuf],
        class_by_id: &HashMap<NodeId, Equiv>,
        equiv_classes: &HashMap<NodeId, Vec<Equiv>>,
        data: &Data,
    ) -> Self {
        // Check that all non-end equivalence classes are the same size
        for (id, nodes) in equiv_classes {
            if id.is_standard() {
                assert_eq!(
                    nodes.len(),
                    part_heads.len(),
                    "Node {:?} has invalid equiv class size ({:?})",
                    id,
                    nodes
                );
            }
        }

        let nodes: HashMap<NodeId, Node> = equiv_classes
            .iter()
            .map(|(equiv_id, source_ids)| self.equiv_node(equiv_id, source_ids, class_by_id, data))
            .collect();

        let start_nodes = self
            .start_nodes
            .iter()
            .filter_map(|(id, start_idx)| {
                let equiv = class_by_id.get(id)?;
                assert_eq!(equiv.rot, 0);
                Some((equiv.id.clone(), *start_idx))
            })
            .collect_vec();
        let end_nodes = nodes
            .iter()
            .filter_map(|(id, node)| node.end().map(|end| (id.clone(), end)))
            .collect_vec();

        Graph {
            nodes,
            start_nodes,
            end_nodes,
            num_parts: part_heads.len(),
        }
    }

    /// Given an equivalence class of [`Node`]s, create a [`Node`] to represent it.
    fn equiv_node(
        &self,
        equiv_id: &NodeId,
        source_ids: &[Equiv],
        class_by_id: &HashMap<NodeId, Equiv>,
        data: &Data,
    ) -> (NodeId, Node) {
        let source_nodes = source_ids
            .iter()
            .filter_map(|equiv| self.get_node(&equiv.id).map(|node| (node, equiv)))
            .collect_vec();
        let &(zero_rot_node, zero_rot_equiv) = source_nodes
            .iter()
            .find(|(_, equiv)| equiv.rot == 0)
            .expect("Each equiv class should have a node with rotation 0");

        let is_std = equiv_id.is_standard();
        assert_eq!(is_std, zero_rot_equiv.id.is_standard());

        // Check all properties that should be equal for any equivalent nodes
        assert!(all_eq(&source_nodes, |(n, equiv)| n.is_start
            || equiv.tag == EquivTag::PartHead));
        if is_std {
            // These things aren't necessarily equal for nodes which become 0-length ends
            assert!(all_eq(&source_nodes, |(n, _)| n.length));
            assert!(all_eq(&source_nodes, |(n, _)| &n.label));
            assert!(all_eq(&source_nodes, |(n, _)| &n.method_counts));
        }

        // Construct new node
        let equiv_node = Node {
            is_start: zero_rot_node.is_start,
            // TODO: Handle these for cases like far calls in Bristol?
            end: zero_rot_node.end,
            label: zero_rot_node.label.clone(),

            successors: combine_links(zero_rot_node, LinkDirection::Succ, class_by_id),
            predecessors: combine_links(zero_rot_node, LinkDirection::Pred, class_by_id),
            false_nodes: combine_falseness(zero_rot_node, class_by_id),

            length: zero_rot_node.length * source_nodes.len(),
            method_counts: &zero_rot_node.method_counts * source_nodes.len(),
            music: if is_std {
                sum_music(&source_nodes, data)
            } else {
                Breakdown::zero(data.music_types.len())
            },

            required: source_nodes.iter().any(|(n, _)| n.required),
            lb_distance_from_rounds: merge_dists(&source_nodes, |n| n.lb_distance_from_rounds),
            lb_distance_to_rounds: if is_std {
                merge_dists(&source_nodes, |n| n.lb_distance_to_rounds)
            } else {
                0
            },
        };
        (equiv_id.clone(), equiv_node)
    }
}

/// Compute the false nodes for an equivalence node
fn combine_links(
    zero_rot_node: &Node,
    direction: LinkDirection,
    class_by_id: &HashMap<NodeId, Equiv>,
) -> Vec<Link> {
    use LinkDirection::*;

    let zero_rot_links = match direction {
        Pred => &zero_rot_node.predecessors,
        Succ => &zero_rot_node.successors,
    };

    // Generate an equivalent link for each new link
    let mut new_links = zero_rot_links
        .iter()
        .filter_map(|link| {
            let equiv = class_by_id.get(&link.id)?;
            let id = match (equiv.tag, direction) {
                (EquivTag::PartHead, Pred) => return None,
                (EquivTag::PartHead, Succ) => NodeId::ZeroLengthEnd,
                (EquivTag::Std, _) => equiv.id.clone(),
            };
            Some(Link {
                id,
                source_idx: link.source_idx,
                rotation: equiv.rot,
            })
        })
        .collect_vec();
    // Remove any predecessor links to 0-length end nodes
    if direction == Pred {
        new_links.retain(|link| link.id.is_standard());
    }
    new_links
}

/// Compute the false nodes for an equivalence node
fn combine_falseness(
    zero_rot_node: &Node,
    class_by_id: &HashMap<NodeId, Equiv>,
) -> Vec<StandardNodeId> {
    // For every false node ...
    zero_rot_node
        .false_nodes
        .iter()
        // ... look up the corresponding equivalence node ...
        .filter_map(|id| class_by_id.get(&NodeId::Standard(id.clone())))
        // ... keeping only the equivalence class's ID ...
        .map(|equiv| &equiv.id)
        // ... and without any 0-length nodes
        .filter_map(|id| id.std_id())
        .cloned()
        .collect_vec()
}

/// Sum the music scores of a list of source nodes
fn sum_music(source_nodes: &[(&Node, &Equiv)], data: &Data) -> Breakdown {
    let mut total = Breakdown::zero(data.music_types.len());
    for (node, _) in source_nodes {
        total += &node.music;
    }
    total
}

/// Extract and merge distances taken from the source nodes
fn merge_dists(source_nodes: &[(&Node, &Equiv)], f: impl Fn(&Node) -> Distance) -> Distance {
    source_nodes
        .iter()
        .map(|(node, _)| f(node))
        .min()
        .unwrap_or(0)
}

/// Returns `true` if `f(t)` is equal for all `t` in some [`Iterator`]
fn all_eq<T, E: Eq + Clone>(i: impl IntoIterator<Item = T>, f: impl FnMut(T) -> E) -> bool {
    i.into_iter().map(f).tuple_windows().all(|(a, b)| a == b)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LinkDirection {
    Succ,
    Pred,
}

#[derive(Debug, Clone)]
struct Equiv {
    tag: EquivTag,
    id: NodeId,
    rot: Rotation,
}

impl Equiv {
    fn new(tag: EquivTag, id: NodeId, rot: Rotation) -> Self {
        Self { tag, id, rot }
    }

    fn std(id: NodeId, rot: Rotation) -> Self {
        Self {
            tag: EquivTag::Std,
            id,
            rot,
        }
    }
}

/// The relation between a [`Node`] in the source graph and a [`Node`] in the multi-part graph (or
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EquivTag {
    /// The [`Node`] is equivalent to a standard node or a 0-length end node.  It gets converted to
    /// this [`NodeId`] in all positions.
    Std,
    /// The [`Node`] is equivalent to the start of a non-rounds part.  This means that it:
    /// - is ignored when computing predecessors
    /// - is treated as equivalent to the corresponding start when computing falseness
    /// - becomes a 0-length end when computing successors
    PartHead,
}
