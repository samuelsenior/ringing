use std::{
    collections::BinaryHeap,
    sync::atomic::{AtomicBool, Ordering},
};

use datasize::DataSize;

use crate::utils::TotalLength;

use super::{path::Paths, prefix::CompPrefix, Progress, Search, Update};

const ITERS_BETWEEN_ABORT_CHECKS: usize = 10_000;
const ITERS_BETWEEN_PROGRESS_UPDATES: usize = 100_000;

/// Searches a [`Graph`](m_gr::Graph) for compositions
pub(crate) fn search(
    search_data: &Search,
    mut update_fn: impl FnMut(Update),
    abort_flag: &AtomicBool,
) {
    // Initialise the frontier to just the start chunks
    let mut paths = Paths::new();
    let mut frontier: BinaryHeap<CompPrefix> = CompPrefix::starts(&search_data.graph, &mut paths);

    // Number of bytes occupied by each `CompPrefix` in the frontier.
    let prefix_size = frontier.peek().unwrap().size();

    let mut iter_count = 0;
    let mut num_comps = 0;

    macro_rules! send_progress_update {
        (truncating_queue = $truncating_queue: expr) => {
            send_progress_update(
                &frontier,
                &mut update_fn,
                iter_count,
                num_comps,
                $truncating_queue,
            );
        };
    }

    // Repeatedly choose the best prefix and expand it (i.e. add each way of extending it to the
    // frontier).  This is best-first search (and can be A* depending on the cost function used).
    while let Some(prefix) = frontier.pop() {
        let maybe_comp = prefix.expand(search_data, &mut paths, &mut frontier);

        // Submit new compositions when they're generated
        if let Some(comp) = maybe_comp {
            update_fn(Update::Comp(comp));
            num_comps += 1;

            if num_comps == search_data.query.num_comps {
                break; // Stop the search once we've got enough comps
            }
        }

        // If we end up using too much memory, half the size of the queue and garbage-collect the
        // paths.
        let mem_usage = frontier.len() * prefix_size + paths.estimate_heap_size();
        if mem_usage >= search_data.config.mem_limit {
            send_progress_update!(truncating_queue = true);
            truncate_queue(frontier.len() / 2, &mut frontier);
            paths.gc(frontier.iter().map(|prefix| prefix.path_head()));
            send_progress_update!(truncating_queue = false);
        }

        iter_count += 1;

        // Check for abort every so often
        if iter_count % ITERS_BETWEEN_ABORT_CHECKS == 0 && abort_flag.load(Ordering::Relaxed) {
            update_fn(Update::Aborting);
            break;
        }

        // Send stats every so often
        if iter_count % ITERS_BETWEEN_PROGRESS_UPDATES == 0 {
            send_progress_update!(truncating_queue = false);
        }
    }

    // Always send a final update before finishing
    send_progress_update!(truncating_queue = false);

    // If we're running the CLI, then `mem::forget` the frontier to avoid tons of drop calls.  We
    // don't care about leaking because the Monument process is about to terminate and the OS will
    // clean up the memory anyway.
    if search_data.config.leak_search_memory {
        std::mem::forget(frontier);
    }
}

fn send_progress_update(
    frontier: &BinaryHeap<CompPrefix>,
    update_fn: &mut impl FnMut(Update),
    iter_count: usize,
    num_comps: usize,
    truncating_queue: bool,
) {
    let mut total_len = 0u64; // NOTE: We have use `u64` here to avoid overflow
    let mut max_length = TotalLength::ZERO;
    frontier.iter().for_each(|n| {
        total_len += n.length().as_usize() as u64;
        max_length = max_length.max(n.length());
    });
    update_fn(Update::Progress(Progress {
        iter_count,
        num_comps,

        queue_len: frontier.len(),
        avg_length: if frontier.is_empty() {
            0.0 // Avoid returning `NaN` if the frontier is empty
        } else {
            total_len as f32 / frontier.len() as f32
        },
        max_length: max_length.as_usize(),

        truncating_queue,
    }));
}

fn truncate_queue<T: Ord>(len: usize, queue: &mut BinaryHeap<T>) {
    let heap = std::mem::take(queue);
    let mut chunks = heap.into_vec();
    chunks.sort_by(|a, b| b.cmp(a)); // Sort highest score first
    if len < chunks.len() {
        chunks.drain(len..);
    }
    *queue = BinaryHeap::from(chunks);
}