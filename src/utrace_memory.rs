//! Bounded aggregation for Unreal Memory trace events.

use std::collections::{BTreeMap, HashMap};

use crate::utrace::{
    CallstackResolution, MemoryAllocationDashboard, MemoryAllocationKind, MemoryAllocationSample,
    MemoryDashboard, MemoryInitSummary, MemoryLlmDashboard, MemoryLlmTagSetSummary,
    MemoryLlmTagSummary, MemoryLlmTrackerSummary, MemoryLlmValueSummary, MemoryRootHeapSummary,
    MemoryScopeSummary, MemoryTagSummary,
};

const MAX_TAGS: usize = 4_096;
const MAX_SCOPE_TAGS: usize = 4_096;
const MAX_OUTSTANDING_ALLOCS: usize = 262_144;
const MAX_ALLOCATION_SAMPLES: usize = 40;
const MAX_LLM_TAGS: usize = 4_096;
const MAX_LLM_TRACKERS: usize = 256;
const MAX_LLM_TAG_SETS: usize = 256;
const MAX_LLM_LATEST_VALUES: usize = 4_096;

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

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct LlmTag {
    pub(crate) tag: i64,
    pub(crate) parent: i64,
    pub(crate) tag_set: u8,
    pub(crate) name: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct LlmTracker {
    pub(crate) tracker_id: u8,
    pub(crate) name: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct LlmTagSet {
    pub(crate) tag_set: u8,
    pub(crate) name: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct LlmLatestValue {
    cycle: u64,
    value: i64,
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
    llm_tags: BTreeMap<i64, LlmTag>,
    llm_tag_count: u64,
    llm_tag_overflow: u64,
    llm_trackers: BTreeMap<u8, LlmTracker>,
    llm_tracker_count: u64,
    llm_tag_sets: BTreeMap<u8, LlmTagSet>,
    llm_tag_set_count: u64,
    llm_sample_events: u64,
    llm_latest_values: BTreeMap<(u8, i64), LlmLatestValue>,
    llm_latest_values_overflow: bool,
    llm_latest_values_dropped: u64,
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
                callstack: CallstackResolution::None,
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

    pub(crate) fn record_llm_tag(&mut self, tag: LlmTag) {
        self.llm_tag_count = self.llm_tag_count.saturating_add(1);
        if self.llm_tags.contains_key(&tag.tag) || self.llm_tags.len() < MAX_LLM_TAGS {
            self.llm_tags.insert(tag.tag, tag);
        } else {
            self.llm_tag_overflow = self.llm_tag_overflow.saturating_add(1);
        }
    }

    pub(crate) fn record_llm_tracker(&mut self, tracker: LlmTracker) {
        self.llm_tracker_count = self.llm_tracker_count.saturating_add(1);
        if self.llm_trackers.contains_key(&tracker.tracker_id)
            || self.llm_trackers.len() < MAX_LLM_TRACKERS
        {
            self.llm_trackers.insert(tracker.tracker_id, tracker);
        }
    }

    pub(crate) fn record_llm_tag_set(&mut self, tag_set: LlmTagSet) {
        self.llm_tag_set_count = self.llm_tag_set_count.saturating_add(1);
        if self.llm_tag_sets.contains_key(&tag_set.tag_set)
            || self.llm_tag_sets.len() < MAX_LLM_TAG_SETS
        {
            self.llm_tag_sets.insert(tag_set.tag_set, tag_set);
        }
    }

    pub(crate) fn record_llm_tag_values(
        &mut self,
        tracker_id: u8,
        cycle: u64,
        values: &[(i64, i64)],
        dropped_values: u64,
    ) {
        self.llm_sample_events = self.llm_sample_events.saturating_add(1);
        self.llm_latest_values_dropped = self
            .llm_latest_values_dropped
            .saturating_add(dropped_values);

        for &(tag, value) in values {
            let key = (tracker_id, tag);
            if let Some(latest) = self.llm_latest_values.get_mut(&key) {
                if cycle >= latest.cycle {
                    *latest = LlmLatestValue { cycle, value };
                }
            } else if self.llm_latest_values.len() < MAX_LLM_LATEST_VALUES {
                self.llm_latest_values
                    .insert(key, LlmLatestValue { cycle, value });
            } else {
                self.llm_latest_values_overflow = true;
                self.llm_latest_values_dropped = self.llm_latest_values_dropped.saturating_add(1);
            }
        }
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

        let mut llm_tags = self
            .llm_tags
            .values()
            .map(|tag| MemoryLlmTagSummary {
                tag: tag.tag,
                parent: tag.parent,
                tag_set: tag.tag_set,
                name: tag.name.clone(),
            })
            .collect::<Vec<_>>();
        llm_tags.sort_by_key(|tag| tag.tag);

        let mut llm_trackers = self
            .llm_trackers
            .values()
            .map(|tracker| MemoryLlmTrackerSummary {
                tracker_id: tracker.tracker_id,
                name: tracker.name.clone(),
            })
            .collect::<Vec<_>>();
        llm_trackers.sort_by_key(|tracker| tracker.tracker_id);

        let mut llm_tag_sets = self
            .llm_tag_sets
            .values()
            .map(|tag_set| MemoryLlmTagSetSummary {
                tag_set: tag_set.tag_set,
                name: tag_set.name.clone(),
            })
            .collect::<Vec<_>>();
        llm_tag_sets.sort_by_key(|tag_set| tag_set.tag_set);

        let mut llm_latest_values = self
            .llm_latest_values
            .into_iter()
            .map(|((tracker_id, tag), latest)| MemoryLlmValueSummary {
                tracker_id,
                cycle: latest.cycle,
                tag,
                value: latest.value,
            })
            .collect::<Vec<_>>();
        llm_latest_values.sort_by(|left, right| {
            right
                .value
                .unsigned_abs()
                .cmp(&left.value.unsigned_abs())
                .then_with(|| right.cycle.cmp(&left.cycle))
                .then_with(|| left.tracker_id.cmp(&right.tracker_id))
                .then_with(|| left.tag.cmp(&right.tag))
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
            llm: MemoryLlmDashboard {
                tag_count: self.llm_tag_count,
                tracker_count: self.llm_tracker_count,
                tag_set_count: self.llm_tag_set_count,
                sample_events: self.llm_sample_events,
                tag_overflow: self.llm_tag_overflow,
                tags: llm_tags,
                trackers: llm_trackers,
                tag_sets: llm_tag_sets,
                latest_values: llm_latest_values,
                latest_values_overflow: self.llm_latest_values_overflow,
                latest_values_dropped: self.llm_latest_values_dropped,
            },
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

    #[test]
    fn llm_catalog_and_latest_values_are_bounded_and_cycle_aware() {
        let mut provider = MemoryProvider::default();
        provider.record_llm_tag(LlmTag {
            tag: 101,
            parent: 100,
            tag_set: 2,
            name: "Textures".to_owned(),
        });
        provider.record_llm_tracker(LlmTracker {
            tracker_id: 1,
            name: "Platform".to_owned(),
        });
        provider.record_llm_tag_set(LlmTagSet {
            tag_set: 2,
            name: "Assets".to_owned(),
        });
        provider.record_llm_tag_values(1, 20, &[(101, 64), (102, -16)], 0);
        provider.record_llm_tag_values(1, 10, &[(101, 32)], 0);
        provider.record_llm_tag_values(1, 30, &[(101, 96)], 3);

        let dashboard = provider.dashboard();
        assert_eq!(dashboard.llm.tag_count, 1);
        assert_eq!(dashboard.llm.tracker_count, 1);
        assert_eq!(dashboard.llm.tag_set_count, 1);
        assert_eq!(dashboard.llm.sample_events, 3);
        assert_eq!(dashboard.llm.tags[0].name, "Textures");
        assert_eq!(dashboard.llm.tags[0].parent, 100);
        assert_eq!(dashboard.llm.trackers[0].name, "Platform");
        assert_eq!(dashboard.llm.tag_sets[0].name, "Assets");
        assert_eq!(dashboard.llm.latest_values[0].tag, 101);
        assert_eq!(dashboard.llm.latest_values[0].cycle, 30);
        assert_eq!(dashboard.llm.latest_values[0].value, 96);
        assert_eq!(dashboard.llm.latest_values_dropped, 3);
    }
}
