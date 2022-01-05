use std::collections::HashMap;

use bellframe::{AnnotBlock, Mask, Row, RowBuf};
use index_vec::IndexVec;
use itertools::Itertools;

use super::{
    utils::{SNAP_FINISH_LABEL, SNAP_START_LABEL},
    Result,
};
use crate::layout::{BlockIdx, BlockVec, Layout, Link, LinkVec, RowIdx, StartOrEnd};

/// Prefix inserted at the front of every leadwise composition to allow it to be parsed as such
const LEADWISE_PREFIX: &str = "#";

/// Creates a `Layout` where every course is exactly one lead long.
pub fn leadwise(
    methods: &[(bellframe::Method, String)],
    calls: &[super::Call],
    start_indices: Option<&[usize]>,
    end_indices: Option<&[usize]>,
) -> Result<Layout> {
    super::utils::check_duplicate_shorthand(methods)?;

    let stage = methods
        .iter()
        .map(|(m, _shorthand)| m.stage())
        .max()
        .expect("Can't compute stage of 0 methods");
    let blocks = methods
        .iter()
        .map(|(method, shorthand)| {
            method
                .first_lead()
                .clone_map_annots_with_index(|i, _| (i == 0).then(|| shorthand.clone()))
        })
        .collect::<BlockVec<_>>();

    // Compute the course head mask, which should only consist of fixed bells (e.g. `1xxxxxxx`
    // for Plain Bob lead-head Major methods).
    let calls_per_method = methods
        .iter()
        .map(|_| calls.iter().collect_vec())
        .collect_vec();
    let fixed_bells = super::utils::fixed_bells(methods, &calls_per_method, stage);
    let lead_head_mask = Mask::fix_bells(stage, fixed_bells);

    let blks = blocks.as_raw_slice();
    Ok(Layout {
        starts: start_or_ends(
            start_indices,
            LEADWISE_PREFIX,
            SNAP_START_LABEL,
            &lead_head_mask,
            blks,
        ),
        ends: start_or_ends(end_indices, "", SNAP_FINISH_LABEL, &lead_head_mask, blks),
        links: links(methods, calls, &lead_head_mask),
        blocks,
        stage,
    })
}

fn start_or_ends<I: index_vec::Idx>(
    allowed_indices: Option<&[usize]>,
    label_prefix: &str,
    snap_label: &str,
    lead_head_mask: &Mask,
    blocks: &[AnnotBlock<Option<String>>],
) -> IndexVec<I, StartOrEnd> {
    let mut locs = IndexVec::new();
    for (meth_idx, first_lead) in blocks.iter().enumerate() {
        // Closure to construct a `StartOrEnd` at a given row
        let block_idx = BlockIdx::new(meth_idx);
        let new_start_or_end = |(row_idx, row): (usize, &Row)| {
            let mut label = label_prefix.to_owned();
            if row_idx != 0 {
                label.push_str(snap_label);
            }
            StartOrEnd {
                course_head: !row,
                row_idx: RowIdx::new(block_idx, row_idx),
                label,
            }
        };

        match allowed_indices {
            // If the user has specified required indices then we add exactly them, panicking
            // if they are longer than the lead length.
            Some(idxs) => locs.extend(idxs.iter().map(|&idx| {
                assert!(idx < first_lead.len());
                new_start_or_end((idx, &first_lead.row_vec()[idx]))
            })),
            // If no indices are specified, then we allow any index which satisfies the
            // lead_head_mask (i.e. any lead index where the fixed bells are at their home
            // positions).
            None => locs.extend(
                first_lead
                    .rows()
                    .enumerate()
                    .filter(|(_, r)| lead_head_mask.matches(r))
                    .map(new_start_or_end),
            ),
        }
    }
    locs
}

fn links(
    methods: &[(bellframe::Method, String)],
    calls: &[super::Call],
    lead_head_mask: &Mask,
) -> LinkVec<Link> {
    // Maps each lead label to where calls of that label can **end**
    let mut call_starts: HashMap<&str, Vec<CallStart>> = HashMap::new();
    let mut call_ends: HashMap<&str, Vec<CallEnd>> = HashMap::new();
    for (block_idx, (method, _)) in methods.iter().enumerate() {
        let lead = method.first_lead();
        for (row_idx_after, annot_row_after) in lead.annot_rows().enumerate() {
            if let Some(label) = annot_row_after.annot() {
                let row_idx_before = (row_idx_after + lead.len() - 1) % lead.len();
                let row_before = lead.get_row(row_idx_before).unwrap();
                let row_after_plain = lead.get_row(row_idx_before + 1).unwrap();

                call_starts
                    .entry(label)
                    .or_insert_with(Vec::new)
                    .push(CallStart {
                        row_idx: RowIdx::new(block_idx.into(), row_idx_before),
                        row_before: row_before.to_owned(),
                        row_after_plain: row_after_plain.to_owned(),
                    });
                call_ends
                    .entry(label)
                    .or_insert_with(Vec::new)
                    .push(CallEnd {
                        row_idx: RowIdx::new(block_idx.into(), row_idx_after),
                        inv_row: !annot_row_after.row(),
                    });
            }
        }
    }

    // Place calls between every `call_start` and every `call_end` of that lead label
    let mut links = Vec::new();
    for call in calls {
        let label = call.lead_location.as_str();
        let starts = &call_starts[label];
        let ends = &call_ends[label];

        for start in starts {
            let mut row_after_call = start.row_before.clone();
            call.place_not.permute(&mut row_after_call).unwrap();
            for end in ends {
                // Call
                links.push(Link {
                    from: start.row_idx,
                    to: end.row_idx,

                    ch_mask: lead_head_mask.clone(),
                    ch_transposition: &row_after_call * &end.inv_row,

                    debug_name: call.debug_symbol.clone(),
                    display_name: format!("[{}]", call.debug_symbol),
                    weight: call.weight,
                });
                // Plain
                links.push(plain_link(
                    start.row_idx,
                    end.row_idx,
                    lead_head_mask,
                    &start.row_after_plain * &end.inv_row,
                ));
            }
        }
    }

    // Always add plain links at the end of every lead.  In nearly all cases, these will already
    // exist and be deduplicated, but if there are no calls at the end of each lead (e.g. in
    // link-cyclic or Stedman) then these will have to be generated separately.
    for (method_idx_from, (method_from, _)) in methods.iter().enumerate() {
        for (method_idx_to, _) in methods.iter().enumerate() {
            links.push(plain_link(
                RowIdx {
                    block: method_idx_from.into(),
                    // - 1 to refer to the lead **end** not the lead **head**
                    row: method_from.lead_len() - 1,
                },
                RowIdx {
                    block: method_idx_to.into(),
                    row: 0,
                },
                lead_head_mask,
                method_from.lead_head().to_owned(),
            ));
        }
    }

    // Deduplicate links and return
    super::utils::dedup_links(&mut links);
    links.into()
}

fn plain_link(from: RowIdx, to: RowIdx, ch_mask: &Mask, ch_transposition: RowBuf) -> Link {
    Link {
        from,
        to,

        ch_mask: ch_mask.clone(),
        ch_transposition,

        debug_name: "p".to_owned(),
        display_name: String::new(),
        weight: 0.0,
    }
}

/// A position at which a call could start
#[derive(Debug, Clone, Eq, PartialEq)]
struct CallStart {
    row_idx: RowIdx,
    row_before: RowBuf,
    row_after_plain: RowBuf,
}

/// A position at which a call could end
#[derive(Debug, Clone, Eq, PartialEq)]
struct CallEnd {
    row_idx: RowIdx,
    inv_row: RowBuf,
}