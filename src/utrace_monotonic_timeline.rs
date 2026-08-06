//! Exact CPU timeline storage modeled after TraceServices `TMonotonicTimeline`.
//!
//! Each thread owns append-only, page-addressable columns. Cycles and begin
//! bits are parallel columns; timer identities are stored only for begins.

use std::collections::BinaryHeap;
use std::mem::size_of;

use rustc_hash::FxHashMap;
use serde::Serialize;

use crate::utrace::{CpuTimelineInterval, CpuTimelineQuery, CpuTimelineQueryResult};
use crate::utrace_timeline::{CpuTimelineIndexInfo, SourceIdentity, TimelineIndexError};

const PAGE_ENTRIES: usize = 65_536;
const NONE_METADATA_ID: u32 = u32::MAX;

#[derive(Clone, Copy, Debug)]
pub(crate) struct CpuMonotonicEventView<'a> {
    pub(crate) spec_id: u32,
    pub(crate) name: &'a str,
    pub(crate) metadata_id: Option<u32>,
    pub(crate) rendered_name: Option<&'a str>,
}

pub(crate) trait CpuMonotonicTimelineSink {
    fn append_begin(&mut self, thread_id: u16, cycle: u64, event: CpuMonotonicEventView<'_>);
    fn append_end(&mut self, thread_id: u16, cycle: u64);
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
pub struct CpuMonotonicTimelineStats {
    pub thread_count: u64,
    pub page_count: u64,
    pub entry_count: u64,
    pub begin_count: u64,
    pub completed_scope_count: u64,
    pub event_count: u64,
    pub payload_bytes: u64,
    pub uncompressed_payload_bytes: u64,
    pub allocated_bytes: u64,
    pub column_allocated_bytes: u64,
    pub page_allocated_bytes: u64,
    pub catalog_allocated_bytes: u64,
    pub bytes_per_begin: u64,
}

#[derive(Clone, Debug)]
pub struct CpuMonotonicTimelineIndex {
    info: CpuTimelineIndexInfo,
    stats: CpuMonotonicTimelineStats,
    events: Vec<OwnedEvent>,
    strings: Vec<u8>,
    threads: FxHashMap<u16, ThreadTimeline>,
}

impl CpuMonotonicTimelineIndex {
    #[must_use]
    pub fn info(&self) -> &CpuTimelineIndexInfo {
        &self.info
    }

    #[must_use]
    pub fn stats(&self) -> &CpuMonotonicTimelineStats {
        &self.stats
    }

    pub fn query(
        &self,
        query: &CpuTimelineQuery,
    ) -> Result<CpuTimelineQueryResult, TimelineIndexError> {
        let start_cycle = query.start_cycle.or(self.info.begin_cycle).unwrap_or(0);
        let end_cycle = query
            .end_cycle
            .or(self.info.end_cycle)
            .unwrap_or(start_cycle);
        if start_cycle > end_cycle {
            return Err(TimelineIndexError::InvalidQuery(
                "timeline start_cycle must not exceed end_cycle".to_owned(),
            ));
        }
        let limit = query.limit.unwrap_or(500).clamp(1, 10_000);
        let needle = query.search.as_ref().map(|value| value.to_lowercase());
        let mut hits = BinaryHeap::with_capacity(limit);
        let mut interval_count = 0_u64;
        for (&thread_id, timeline) in &self.threads {
            if query.thread_id.is_some_and(|wanted| wanted != thread_id) {
                continue;
            }
            timeline.enumerate(start_cycle, end_cycle, |event_id, begin, end| {
                let Some(event) = self.events.get(event_id as usize) else {
                    return;
                };
                let name = text_at(&self.strings, event.name).unwrap_or("<invalid name>");
                let rendered_name = event
                    .rendered_name
                    .and_then(|reference| text_at(&self.strings, reference));
                if needle.as_deref().is_some_and(|needle| {
                    !name.to_lowercase().contains(needle)
                        && !rendered_name.is_some_and(|name| name.to_lowercase().contains(needle))
                }) {
                    return;
                }
                interval_count = interval_count.saturating_add(1);
                let hit = QueryHit {
                    start_cycle: begin,
                    end_cycle: end,
                    thread_id,
                    event_id,
                };
                if hits.len() < limit {
                    hits.push(hit);
                } else if hits.peek().is_some_and(|latest| hit < *latest) {
                    hits.pop();
                    hits.push(hit);
                }
            });
        }
        let intervals = hits
            .into_sorted_vec()
            .into_iter()
            .filter_map(|hit| {
                let event = self.events.get(hit.event_id as usize)?;
                let name = text_at(&self.strings, event.name).unwrap_or("<invalid name>");
                let duration = hit.end_cycle.saturating_sub(hit.start_cycle);
                Some(CpuTimelineInterval {
                    thread_id: hit.thread_id,
                    spec_id: event.spec_id,
                    name: name.to_owned(),
                    start_cycle: hit.start_cycle,
                    end_cycle: hit.end_cycle,
                    duration,
                    duration_seconds: self
                        .info
                        .cycle_frequency
                        .map(|frequency| duration as f64 / frequency as f64),
                    metadata_id: (event.metadata_id != NONE_METADATA_ID)
                        .then_some(event.metadata_id),
                    rendered_name: event
                        .rendered_name
                        .and_then(|reference| text_at(&self.strings, reference))
                        .map(str::to_owned),
                })
            })
            .collect();
        Ok(CpuTimelineQueryResult {
            index: self.info.clone(),
            begin_cycle: start_cycle,
            end_cycle,
            duration_seconds: self
                .info
                .cycle_frequency
                .map(|frequency| end_cycle.saturating_sub(start_cycle) as f64 / frequency as f64),
            interval_count,
            truncated: interval_count > u64::try_from(limit).unwrap_or(u64::MAX),
            intervals,
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct QueryHit {
    start_cycle: u64,
    end_cycle: u64,
    thread_id: u16,
    event_id: u32,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct EventKey {
    spec_id: u32,
    metadata_id: u32,
}

#[derive(Clone, Debug)]
struct OwnedEvent {
    spec_id: u32,
    metadata_id: u32,
    name: StringRef,
    rendered_name: Option<StringRef>,
}

#[derive(Clone, Copy, Debug)]
struct StringRef {
    offset: u32,
    len: u32,
}

#[derive(Clone, Copy, Debug)]
struct OpenScope {
    start_cycle: u64,
    event_id: u32,
}

#[derive(Clone, Debug)]
struct Page {
    begin_cycle: u64,
    end_cycle: u64,
    initial_stack: Vec<OpenScope>,
    begin_bits: Vec<u64>,
    columns: PageColumns,
}

#[derive(Clone, Debug)]
enum PageColumns {
    Open {
        cycles: Vec<u64>,
        event_ids: Vec<u32>,
    },
    Encoded {
        entry_count: usize,
        cycles: Vec<u8>,
        event_ids: Vec<u8>,
    },
}

impl Page {
    fn new(cycle: u64, initial_stack: &[OpenScope]) -> Self {
        Self {
            begin_cycle: cycle,
            end_cycle: cycle,
            initial_stack: initial_stack.to_vec(),
            begin_bits: Vec::with_capacity(PAGE_ENTRIES / 64),
            columns: PageColumns::Open {
                cycles: Vec::with_capacity(PAGE_ENTRIES),
                event_ids: Vec::with_capacity(PAGE_ENTRIES / 2),
            },
        }
    }

    fn start_entry(&mut self, cycle: u64) -> usize {
        self.end_cycle = cycle;
        let entry_index = self.entry_count();
        if entry_index % 64 == 0 {
            self.begin_bits.push(0);
        }
        entry_index
    }

    fn entry_count(&self) -> usize {
        match &self.columns {
            PageColumns::Open { cycles, .. } => cycles.len(),
            PageColumns::Encoded { entry_count, .. } => *entry_count,
        }
    }

    fn is_full(&self) -> bool {
        self.entry_count() == PAGE_ENTRIES
    }

    fn append_begin(&mut self, cycle: u64, event_id: u32) {
        let entry_index = self.start_entry(cycle);
        self.begin_bits[entry_index / 64] |= 1_u64 << (entry_index % 64);
        let PageColumns::Open { cycles, event_ids } = &mut self.columns else {
            unreachable!("encoded pages are immutable");
        };
        cycles.push(cycle);
        event_ids.push(event_id);
    }

    fn append_end(&mut self, cycle: u64) {
        self.start_entry(cycle);
        let PageColumns::Open { cycles, .. } = &mut self.columns else {
            unreachable!("encoded pages are immutable");
        };
        cycles.push(cycle);
    }

    fn encode(&mut self) {
        if matches!(self.columns, PageColumns::Encoded { .. }) {
            return;
        }
        let PageColumns::Open { cycles, event_ids } = std::mem::replace(
            &mut self.columns,
            PageColumns::Encoded {
                entry_count: 0,
                cycles: Vec::new(),
                event_ids: Vec::new(),
            },
        ) else {
            return;
        };
        let entry_count = cycles.len();
        let mut encoded_cycles = Vec::with_capacity(entry_count.saturating_mul(3));
        let mut previous_cycle = 0_u64;
        for cycle in cycles {
            append_varint(&mut encoded_cycles, zigzag_delta(cycle, previous_cycle));
            previous_cycle = cycle;
        }
        let mut encoded_event_ids = Vec::with_capacity(event_ids.len().saturating_mul(3));
        let mut previous_id = 0_u32;
        for event_id in event_ids {
            append_varint(
                &mut encoded_event_ids,
                zigzag_delta(u64::from(event_id), u64::from(previous_id)),
            );
            previous_id = event_id;
        }
        encoded_cycles.shrink_to_fit();
        encoded_event_ids.shrink_to_fit();
        self.columns = PageColumns::Encoded {
            entry_count,
            cycles: encoded_cycles,
            event_ids: encoded_event_ids,
        };
    }

    fn is_begin(&self, index: usize) -> bool {
        self.begin_bits
            .get(index / 64)
            .is_some_and(|word| (word & (1_u64 << (index % 64))) != 0)
    }

    fn for_each_entry(&self, mut visit: impl FnMut(usize, u64, Option<u32>) -> bool) {
        match &self.columns {
            PageColumns::Open { cycles, event_ids } => {
                let mut event_index = 0;
                for (entry_index, &cycle) in cycles.iter().enumerate() {
                    let event_id = self.is_begin(entry_index).then(|| {
                        let id = event_ids[event_index];
                        event_index += 1;
                        id
                    });
                    if !visit(entry_index, cycle, event_id) {
                        break;
                    }
                }
            }
            PageColumns::Encoded {
                entry_count,
                cycles,
                event_ids,
            } => {
                let mut cycle_cursor = 0;
                let mut event_cursor = 0;
                let mut cycle = 0_u64;
                let mut event_id = 0_u64;
                for entry_index in 0..*entry_count {
                    let Some(delta) = read_varint(cycles, &mut cycle_cursor) else {
                        break;
                    };
                    cycle = apply_zigzag_delta(cycle, delta);
                    let decoded_event_id = if self.is_begin(entry_index) {
                        let Some(delta) = read_varint(event_ids, &mut event_cursor) else {
                            break;
                        };
                        event_id = apply_zigzag_delta(event_id, delta);
                        Some(u32::try_from(event_id).unwrap_or(u32::MAX))
                    } else {
                        None
                    };
                    if !visit(entry_index, cycle, decoded_event_id) {
                        break;
                    }
                }
            }
        }
    }
}

#[derive(Clone, Debug, Default)]
struct ThreadTimeline {
    pages: Vec<Page>,
    stack: Vec<OpenScope>,
    completed_scope_count: u64,
}

impl ThreadTimeline {
    fn page_for_append(&mut self, cycle: u64) -> &mut Page {
        if self.pages.last().is_none_or(Page::is_full) {
            if let Some(page) = self.pages.last_mut() {
                page.encode();
            }
            self.pages.push(Page::new(cycle, &self.stack));
        }
        self.pages.last_mut().expect("timeline page was appended")
    }

    fn append_begin(&mut self, cycle: u64, event_id: u32) {
        let page = self.page_for_append(cycle);
        page.append_begin(cycle, event_id);
        self.stack.push(OpenScope {
            start_cycle: cycle,
            event_id,
        });
    }

    fn append_end(&mut self, cycle: u64) {
        let Some(_) = self.stack.last() else {
            return;
        };
        let page = self.page_for_append(cycle);
        page.append_end(cycle);
        self.stack.pop();
        self.completed_scope_count = self.completed_scope_count.saturating_add(1);
    }

    fn enumerate(&self, start_cycle: u64, end_cycle: u64, mut emit: impl FnMut(u32, u64, u64)) {
        let Some(_) = self.pages.first() else {
            return;
        };
        let page_index = self
            .pages
            .partition_point(|page| page.begin_cycle <= start_cycle)
            .saturating_sub(1);
        let mut stack = self.pages[page_index]
            .initial_stack
            .iter()
            .map(|open| (open.event_id, open.start_cycle))
            .collect::<Vec<_>>();
        'pages: for page in &self.pages[page_index..] {
            let mut continue_page = true;
            page.for_each_entry(|_entry_index, cycle, event_id| {
                if cycle > end_cycle {
                    continue_page = false;
                    return false;
                }
                if let Some(event_id) = event_id {
                    stack.push((event_id, cycle));
                } else if let Some((event_id, begin_cycle)) = stack.pop()
                    && cycle >= start_cycle
                {
                    emit(event_id, begin_cycle, cycle);
                }
                true
            });
            if !continue_page {
                break 'pages;
            }
        }
        for (event_id, begin_cycle) in stack {
            if begin_cycle <= end_cycle {
                emit(event_id, begin_cycle, end_cycle);
            }
        }
    }
}

pub(crate) struct CpuMonotonicTimelineBuilder {
    begin_count: u64,
    begin_cycle: Option<u64>,
    end_cycle: Option<u64>,
    event_ids: FxHashMap<EventKey, u32>,
    spec_names: FxHashMap<u32, StringRef>,
    events: Vec<OwnedEvent>,
    strings: Vec<u8>,
    threads: FxHashMap<u16, ThreadTimeline>,
}

impl CpuMonotonicTimelineBuilder {
    pub(crate) fn new() -> Self {
        Self {
            begin_count: 0,
            begin_cycle: None,
            end_cycle: None,
            event_ids: FxHashMap::default(),
            spec_names: FxHashMap::default(),
            events: Vec::new(),
            strings: Vec::new(),
            threads: FxHashMap::default(),
        }
    }

    pub(crate) fn finish(
        mut self,
        source: SourceIdentity,
        cycle_frequency: Option<u64>,
    ) -> CpuMonotonicTimelineIndex {
        for timeline in self.threads.values_mut() {
            for page in &mut timeline.pages {
                page.encode();
            }
        }
        let entry_count = self
            .threads
            .values()
            .flat_map(|timeline| &timeline.pages)
            .map(|page| page.entry_count() as u64)
            .sum::<u64>();
        let completed_scope_count = self
            .threads
            .values()
            .map(|timeline| timeline.completed_scope_count)
            .sum::<u64>();
        let page_count = self
            .threads
            .values()
            .map(|timeline| timeline.pages.len() as u64)
            .sum::<u64>();
        let payload_bytes = self.threads.values().map(thread_payload_bytes).sum::<u64>();
        let uncompressed_payload_bytes = entry_count
            .saturating_mul(8)
            .saturating_add(self.begin_count.saturating_mul(4))
            .saturating_add(
                self.threads
                    .values()
                    .flat_map(|timeline| &timeline.pages)
                    .map(|page| (page.begin_bits.len() * size_of::<u64>()) as u64)
                    .sum::<u64>(),
            );
        let column_allocated_bytes = self
            .threads
            .values()
            .map(thread_column_allocated_bytes)
            .sum::<u64>();
        let page_allocated_bytes = self
            .threads
            .values()
            .map(thread_page_allocated_bytes)
            .sum::<u64>();
        let catalog_allocated_bytes =
            (self.events.capacity() * size_of::<OwnedEvent>() + self.strings.capacity()) as u64;
        let allocated_bytes = column_allocated_bytes
            .saturating_add(page_allocated_bytes)
            .saturating_add(catalog_allocated_bytes);
        let stats = CpuMonotonicTimelineStats {
            thread_count: self.threads.len() as u64,
            page_count,
            entry_count,
            begin_count: self.begin_count,
            completed_scope_count,
            event_count: self.events.len() as u64,
            payload_bytes,
            uncompressed_payload_bytes,
            allocated_bytes,
            column_allocated_bytes,
            page_allocated_bytes,
            catalog_allocated_bytes,
            bytes_per_begin: payload_bytes
                .saturating_add(self.begin_count.saturating_sub(1))
                .checked_div(self.begin_count)
                .unwrap_or(0),
        };
        CpuMonotonicTimelineIndex {
            info: CpuTimelineIndexInfo {
                source_bytes: source.source_bytes,
                source_fingerprint: source.fingerprint,
                cycle_frequency,
                total_interval_count: self.begin_count,
                indexed_interval_count: completed_scope_count,
                truncated: false,
                begin_cycle: self.begin_cycle,
                end_cycle: self.end_cycle,
            },
            stats,
            events: self.events,
            strings: self.strings,
            threads: self.threads,
        }
    }

    fn intern(&mut self, event: CpuMonotonicEventView<'_>) -> u32 {
        let key = EventKey {
            spec_id: event.spec_id,
            metadata_id: event.metadata_id.unwrap_or(NONE_METADATA_ID),
        };
        if let Some(&id) = self.event_ids.get(&key) {
            return id;
        }
        let id = u32::try_from(self.events.len()).unwrap_or(u32::MAX);
        let name = if let Some(&name) = self.spec_names.get(&event.spec_id) {
            name
        } else {
            let name = append_text(&mut self.strings, event.name);
            self.spec_names.insert(event.spec_id, name);
            name
        };
        let rendered_name = event
            .rendered_name
            .map(|value| append_text(&mut self.strings, value));
        self.events.push(OwnedEvent {
            spec_id: event.spec_id,
            metadata_id: key.metadata_id,
            name,
            rendered_name,
        });
        self.event_ids.insert(key, id);
        id
    }
}

impl CpuMonotonicTimelineSink for CpuMonotonicTimelineBuilder {
    fn append_begin(&mut self, thread_id: u16, cycle: u64, event: CpuMonotonicEventView<'_>) {
        let event_id = self.intern(event);
        self.threads
            .entry(thread_id)
            .or_default()
            .append_begin(cycle, event_id);
        self.begin_count = self.begin_count.saturating_add(1);
        self.begin_cycle = Some(self.begin_cycle.map_or(cycle, |begin| begin.min(cycle)));
        self.end_cycle = Some(self.end_cycle.map_or(cycle, |end| end.max(cycle)));
    }

    fn append_end(&mut self, thread_id: u16, cycle: u64) {
        self.threads.entry(thread_id).or_default().append_end(cycle);
        self.end_cycle = Some(self.end_cycle.map_or(cycle, |end| end.max(cycle)));
    }
}

fn thread_payload_bytes(timeline: &ThreadTimeline) -> u64 {
    timeline
        .pages
        .iter()
        .map(|page| {
            let columns = match &page.columns {
                PageColumns::Open { cycles, event_ids } => {
                    cycles.len() * size_of::<u64>() + event_ids.len() * size_of::<u32>()
                }
                PageColumns::Encoded {
                    cycles, event_ids, ..
                } => cycles.len() + event_ids.len(),
            };
            (columns + page.begin_bits.len() * size_of::<u64>()) as u64
        })
        .sum()
}

fn thread_column_allocated_bytes(timeline: &ThreadTimeline) -> u64 {
    timeline
        .pages
        .iter()
        .map(|page| {
            let columns = match &page.columns {
                PageColumns::Open { cycles, event_ids } => {
                    cycles.capacity() * size_of::<u64>() + event_ids.capacity() * size_of::<u32>()
                }
                PageColumns::Encoded {
                    cycles, event_ids, ..
                } => cycles.capacity() + event_ids.capacity(),
            };
            (columns + page.begin_bits.capacity() * size_of::<u64>()) as u64
        })
        .sum()
}

fn thread_page_allocated_bytes(timeline: &ThreadTimeline) -> u64 {
    let pages = timeline.pages.capacity() * size_of::<Page>()
        + timeline.stack.capacity() * size_of::<OpenScope>();
    let stacks = timeline
        .pages
        .iter()
        .map(|page| page.initial_stack.capacity() * size_of::<OpenScope>())
        .sum::<usize>();
    (pages + stacks) as u64
}

fn append_text(strings: &mut Vec<u8>, value: &str) -> StringRef {
    let offset = u32::try_from(strings.len()).expect("trace string arena is bounded below 4 GiB");
    let len = u32::try_from(value.len()).expect("individual trace strings are bounded below 4 GiB");
    strings.extend_from_slice(value.as_bytes());
    StringRef { offset, len }
}

fn text_at(strings: &[u8], reference: StringRef) -> Option<&str> {
    let start = reference.offset as usize;
    let end = start.checked_add(reference.len as usize)?;
    std::str::from_utf8(strings.get(start..end)?).ok()
}

fn append_varint(output: &mut Vec<u8>, mut value: u64) {
    while value >= 0x80 {
        output.push((value as u8) | 0x80);
        value >>= 7;
    }
    output.push(value as u8);
}

fn read_varint(input: &[u8], cursor: &mut usize) -> Option<u64> {
    let mut value = 0_u64;
    for shift in (0..=63).step_by(7) {
        let byte = *input.get(*cursor)?;
        *cursor += 1;
        value |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Some(value);
        }
    }
    None
}

fn zigzag_delta(value: u64, previous: u64) -> u64 {
    if value >= previous {
        value.saturating_sub(previous).saturating_mul(2)
    } else {
        previous
            .saturating_sub(value)
            .saturating_mul(2)
            .saturating_add(1)
    }
}

fn apply_zigzag_delta(previous: u64, encoded: u64) -> u64 {
    let magnitude = encoded >> 1;
    if encoded & 1 == 0 {
        previous.saturating_add(magnitude)
    } else {
        previous.saturating_sub(magnitude)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(spec_id: u32, name: &str) -> CpuMonotonicEventView<'_> {
        CpuMonotonicEventView {
            spec_id,
            name,
            metadata_id: None,
            rendered_name: None,
        }
    }

    #[test]
    fn nested_scopes_query_from_parallel_columns() {
        let mut builder = CpuMonotonicTimelineBuilder::new();
        builder.append_begin(2, 10, event(1, "outer"));
        builder.append_begin(2, 20, event(2, "inner"));
        builder.append_end(2, 30);
        builder.append_end(2, 40);
        let index = builder.finish(SourceIdentity::from_bytes(b"trace"), Some(10));
        let result = index
            .query(&CpuTimelineQuery {
                start_cycle: Some(15),
                end_cycle: Some(35),
                ..CpuTimelineQuery::default()
            })
            .unwrap();
        assert_eq!(result.interval_count, 2);
        assert_eq!(result.intervals[0].name, "outer");
        assert_eq!(result.intervals[0].end_cycle, 35);
        assert_eq!(result.intervals[1].name, "inner");
        assert_eq!(result.intervals[1].end_cycle, 30);
        assert_eq!(index.stats().payload_bytes, 14);
        assert_eq!(index.stats().uncompressed_payload_bytes, 48);
    }

    #[test]
    fn completed_pages_remain_queryable_after_finish() {
        let mut builder = CpuMonotonicTimelineBuilder::new();
        for scope in 0..=(PAGE_ENTRIES / 2) {
            let begin = (scope * 2) as u64;
            builder.append_begin(7, begin, event(1, "scope"));
            builder.append_end(7, begin + 1);
        }
        let index = builder.finish(SourceIdentity::from_bytes(b"trace"), None);
        assert_eq!(index.stats().entry_count, PAGE_ENTRIES as u64 + 2);
        let result = index
            .query(&CpuTimelineQuery {
                start_cycle: Some(PAGE_ENTRIES as u64 - 2),
                end_cycle: Some(PAGE_ENTRIES as u64 + 1),
                thread_id: Some(7),
                ..CpuTimelineQuery::default()
            })
            .unwrap();
        assert_eq!(result.interval_count, 2);
    }

    #[test]
    fn signed_varint_delta_round_trips_both_directions() {
        for (previous, value) in [(0, 10), (10, 10), (10, 3), (3, u32::MAX as u64)] {
            assert_eq!(
                apply_zigzag_delta(previous, zigzag_delta(value, previous)),
                value
            );
        }
    }

    #[test]
    fn bounded_query_keeps_earliest_scope_across_threads() {
        let mut builder = CpuMonotonicTimelineBuilder::new();
        builder.append_begin(10, 100, event(1, "late"));
        builder.append_end(10, 110);
        builder.append_begin(2, 10, event(2, "early"));
        builder.append_end(2, 20);
        let index = builder.finish(SourceIdentity::from_bytes(b"trace"), None);
        let result = index
            .query(&CpuTimelineQuery {
                limit: Some(1),
                ..CpuTimelineQuery::default()
            })
            .unwrap();
        assert_eq!(result.interval_count, 2);
        assert_eq!(result.intervals[0].name, "early");
        assert!(result.truncated);
    }
}
