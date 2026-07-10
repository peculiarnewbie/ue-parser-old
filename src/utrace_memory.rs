//! Bounded aggregation for Unreal Memory trace events.

use std::collections::{BTreeMap, HashMap};

use crate::utrace::{
    MemoryAllocationDashboard, MemoryAllocationKind, MemoryAllocationSample, MemoryDashboard,
    MemoryInitSummary, MemoryLlmDashboard, MemoryRootHeapSummary, MemoryScopeSummary,
    MemoryTagSummary,
};

const MAX_TAGS: usize = 4_096;
const MAX_SCOPE_TAGS: usize = 4_096;
const MAX_OUTSTANDING_ALLOCS: usize = 262_144;
const MAX_ALLOCATION_SAMPLES: usize = 40;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct MemoryInit {
    pub(crate) version: u8,
    pub(crate) page_size: u64,
    pub(crate) marker_period: u32,
    pub(crate) min_alignment: u8,
    pub(crate) size_shift: u8,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct MemoryTag {
    pub(crate) tag: i32,
    pub(crate) parent: i32,
    pub(crate) display: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct MemoryAllocation {
    pub(crate) address: u64,
    pub(crate) size: u64,
    pub(crate) root_heap: u8,
    pub(crate) callstack_id: u32,
    pub(crate) kind: MemoryAllocationKind,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct MemoryFree {
    pub(crate) address: u64,
    pub(crate) root_heap: u8,
    pub(crate) is_realloc: bool,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct RootHeapTotals {
    alloc_count: u64,
    free_count: u64,
    bytes_allocated: u64,
    bytes_freed: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct OutstandingAllocation {
    size: u64,
    root_heap: u8,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct MemoryProvider {
    init: Option<MemoryInit>,
    tags: BTreeMap<i32, MemoryTag>,
    tag_count: u64,
    tag_overflow: u64,
    scope_counts: BTreeMap<i32, u64>,
    scope_count: u64,
    scope_tag_overflow: u64,
    alloc_count: u64,
    free_count: u64,
    realloc_alloc_count: u64,
    realloc_free_count: u64,
    bytes_allocated: u64,
    bytes_freed: u64,
    unresolved_free: u64,
    outstanding: HashMap<u64, OutstandingAllocation>,
    outstanding_overflow: bool,
    outstanding_dropped: u64,
    root_heaps: BTreeMap<u8, RootHeapTotals>,
    samples: Vec<MemoryAllocationSample>,
}

impl MemoryProvider {
    pub(crate) fn set_init(&mut self, init: MemoryInit) {
        self.init = Some(init);
    }

    pub(crate) fn init(&self) -> Option<MemoryInit> {
        self.init
    }

    pub(crate) fn record_tag(&mut self, tag: MemoryTag) {
        self.tag_count = self.tag_count.saturating_add(1);
        if self.tags.contains_key(&tag.tag) || self.tags.len() < MAX_TAGS {
            self.tags.insert(tag.tag, tag);
        } else {
            self.tag_overflow = self.tag_overflow.saturating_add(1);
        }
    }

    pub(crate) fn record_scope(&mut self, tag: i32) {
        self.scope_count = self.scope_count.saturating_add(1);
        if self.scope_counts.contains_key(&tag) || self.scope_counts.len() < MAX_SCOPE_TAGS {
            *self.scope_counts.entry(tag).or_default() += 1;
        } else {
            self.scope_tag_overflow = self.scope_tag_overflow.saturating_add(1);
        }
    }

    pub(crate) fn record_allocation(&mut self, allocation: MemoryAllocation) {
        self.alloc_count = self.alloc_count.saturating_add(1);
        if allocation.kind == MemoryAllocationKind::ReallocAlloc {
            self.realloc_alloc_count = self.realloc_alloc_count.saturating_add(1);
        }
        self.bytes_allocated = self.bytes_allocated.saturating_add(allocation.size);
        let root = self.root_heaps.entry(allocation.root_heap).or_default();
        root.alloc_count = root.alloc_count.saturating_add(1);
        root.bytes_allocated = root.bytes_allocated.saturating_add(allocation.size);

        if self.samples.len() < MAX_ALLOCATION_SAMPLES {
            self.samples.push(MemoryAllocationSample {
                address: allocation.address,
                size: allocation.size,
                root_heap: allocation.root_heap,
                callstack_id: allocation.callstack_id,
                kind: allocation.kind,
            });
        }

        if self.outstanding_overflow {
            self.outstanding_dropped = self.outstanding_dropped.saturating_add(1);
        } else if self.outstanding.len() < MAX_OUTSTANDING_ALLOCS
            || self.outstanding.contains_key(&allocation.address)
        {
            self.outstanding.insert(
                allocation.address,
                OutstandingAllocation {
                    size: allocation.size,
                    root_heap: allocation.root_heap,
                },
            );
        } else {
            self.outstanding_overflow = true;
            self.outstanding_dropped = self.outstanding_dropped.saturating_add(1);
        }
    }

    pub(crate) fn record_free(&mut self, free: MemoryFree) {
        self.free_count = self.free_count.saturating_add(1);
        if free.is_realloc {
            self.realloc_free_count = self.realloc_free_count.saturating_add(1);
        }

        let Some(allocation) = self.outstanding.remove(&free.address) else {
            self.unresolved_free = self.unresolved_free.saturating_add(1);
            let root = self.root_heaps.entry(free.root_heap).or_default();
            root.free_count = root.free_count.saturating_add(1);
            return;
        };

        self.bytes_freed = self.bytes_freed.saturating_add(allocation.size);
        let root = self.root_heaps.entry(allocation.root_heap).or_default();
        root.free_count = root.free_count.saturating_add(1);
        root.bytes_freed = root.bytes_freed.saturating_add(allocation.size);
    }

    pub(crate) fn dashboard(self) -> MemoryDashboard {
        let mut tags = self
            .tags
            .values()
            .map(|tag| MemoryTagSummary {
                tag: tag.tag,
                parent: tag.parent,
                display: tag.display.clone(),
            })
            .collect::<Vec<_>>();
        tags.sort_by_key(|tag| tag.tag);

        let mut scopes = self
            .scope_counts
            .into_iter()
            .map(|(tag, count)| MemoryScopeSummary {
                tag,
                count,
                display: self.tags.get(&tag).map(|tag| tag.display.clone()),
            })
            .collect::<Vec<_>>();
        scopes.sort_by(|left, right| {
            right
                .count
                .cmp(&left.count)
                .then_with(|| left.tag.cmp(&right.tag))
        });

        let outstanding_allocations = u64::try_from(self.outstanding.len()).unwrap_or(u64::MAX);
        let outstanding_bytes = self.outstanding.values().fold(0_u64, |total, allocation| {
            total.saturating_add(allocation.size)
        });
        let mut by_root_heap = self
            .root_heaps
            .into_iter()
            .map(|(root_heap, totals)| MemoryRootHeapSummary {
                root_heap,
                name: root_heap_name(root_heap).to_owned(),
                alloc_count: totals.alloc_count,
                free_count: totals.free_count,
                bytes_allocated: totals.bytes_allocated,
                bytes_freed: totals.bytes_freed,
                net_bytes: signed_difference(totals.bytes_allocated, totals.bytes_freed),
            })
            .collect::<Vec<_>>();
        by_root_heap.sort_by(|left, right| {
            right
                .bytes_allocated
                .cmp(&left.bytes_allocated)
                .then_with(|| left.root_heap.cmp(&right.root_heap))
        });

        MemoryDashboard {
            init: self.init.map(|init| MemoryInitSummary {
                version: init.version,
                page_size: init.page_size,
                marker_period: init.marker_period,
                min_alignment: init.min_alignment,
                size_shift: init.size_shift,
            }),
            tag_count: self.tag_count,
            tag_overflow: self.tag_overflow,
            tags,
            scope_count: self.scope_count,
            scope_tag_overflow: self.scope_tag_overflow,
            scopes,
            allocs: MemoryAllocationDashboard {
                count: self.alloc_count,
                free_count: self.free_count,
                realloc_alloc_count: self.realloc_alloc_count,
                realloc_free_count: self.realloc_free_count,
                bytes_allocated: self.bytes_allocated,
                bytes_freed: self.bytes_freed,
                net_bytes: signed_difference(self.bytes_allocated, self.bytes_freed),
                unresolved_free: self.unresolved_free,
                outstanding_allocations,
                outstanding_bytes,
                outstanding_overflow: self.outstanding_overflow,
                outstanding_dropped: self.outstanding_dropped,
                by_root_heap,
                samples: self.samples,
            },
            llm: MemoryLlmDashboard::default(),
        }
    }
}

fn root_heap_name(root_heap: u8) -> &'static str {
    match root_heap {
        0 => "System memory",
        1 => "Video memory",
        _ => "Other",
    }
}

fn signed_difference(allocated: u64, freed: u64) -> i64 {
    let difference = i128::from(allocated) - i128::from(freed);
    difference.clamp(i128::from(i64::MIN), i128::from(i64::MAX)) as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summaries_reclaim_a_matched_allocation() {
        let mut provider = MemoryProvider::default();
        provider.set_init(MemoryInit {
            version: 2,
            page_size: 4096,
            marker_period: 4096,
            min_alignment: 8,
            size_shift: 3,
        });
        provider.record_tag(MemoryTag {
            tag: 7,
            parent: 0,
            display: "Streaming".to_owned(),
        });
        provider.record_scope(7);
        provider.record_allocation(MemoryAllocation {
            address: 0x10,
            size: 64,
            root_heap: 0,
            callstack_id: 3,
            kind: MemoryAllocationKind::Alloc,
        });
        provider.record_free(MemoryFree {
            address: 0x10,
            root_heap: 0,
            is_realloc: false,
        });

        let dashboard = provider.dashboard();
        assert_eq!(dashboard.init.as_ref().unwrap().size_shift, 3);
        assert_eq!(dashboard.scopes[0].display.as_deref(), Some("Streaming"));
        assert_eq!(dashboard.allocs.count, 1);
        assert_eq!(dashboard.allocs.bytes_allocated, 64);
        assert_eq!(dashboard.allocs.bytes_freed, 64);
        assert_eq!(dashboard.allocs.net_bytes, 0);
        assert_eq!(dashboard.allocs.outstanding_allocations, 0);
    }

    #[test]
    fn outstanding_tracking_stops_at_a_fixed_bound() {
        let mut provider = MemoryProvider::default();
        for address in 0..=u64::try_from(MAX_OUTSTANDING_ALLOCS).unwrap() {
            provider.record_allocation(MemoryAllocation {
                address,
                size: 1,
                root_heap: 0,
                callstack_id: 0,
                kind: MemoryAllocationKind::Alloc,
            });
        }

        let dashboard = provider.dashboard();
        assert!(dashboard.allocs.outstanding_overflow);
        assert_eq!(
            dashboard.allocs.outstanding_allocations,
            u64::try_from(MAX_OUTSTANDING_ALLOCS).unwrap()
        );
        assert_eq!(dashboard.allocs.outstanding_dropped, 1);
    }
}
