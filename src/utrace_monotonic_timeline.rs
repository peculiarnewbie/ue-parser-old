//! Exact CPU timeline storage modeled after TraceServices `TMonotonicTimeline`.
//!
//! Each thread owns append-only, page-addressable columns. Cycles and begin
//! bits are parallel columns; timer identities are stored only for begins.

use std::borrow::Cow;
use std::collections::BinaryHeap;
use std::mem::size_of;

use rustc_hash::FxHashMap;
use serde::Serialize;

use crate::utrace::{CpuTimelineInterval, CpuTimelineQuery, CpuTimelineQueryResult};
use crate::utrace_timeline::{CpuTimelineIndexInfo, SourceIdentity, TimelineIndexError};

const PAGE_ENTRIES: usize = 65_536;
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) enum CpuTimerRef {
    Spec(u32),
    Metadata(u32),
}

pub(crate) trait CpuTimelineCatalogSink {
    fn register_spec(&mut self, spec_id: u32, name: &str);
    fn register_metadata(&mut self, metadata_id: u32, spec_id: u32, rendered_name: Option<&str>);
}

pub(crate) trait CpuMonotonicTimelineSink {
    fn append_begin(&mut self, thread_id: u16, cycle: u64, timer: CpuTimerRef);
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
    specs: Vec<SpecCatalogRecord>,
    metadata: Vec<MetadataCatalogRecord>,
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
            timeline.enumerate(start_cycle, end_cycle, |timer_ref, begin, end| {
                let Some(timer) = self.resolve_timer(timer_ref) else {
                    return;
                };
                let name = self.timer_name(timer);
                let rendered_name = timer
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
                    timer: timer.timer,
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
                let timer = self.resolve_timer(hit.timer)?;
                let name = self.timer_name(timer);
                let duration = hit.end_cycle.saturating_sub(hit.start_cycle);
                Some(CpuTimelineInterval {
                    thread_id: hit.thread_id,
                    spec_id: timer.spec_id,
                    name: name.into_owned(),
                    start_cycle: hit.start_cycle,
                    end_cycle: hit.end_cycle,
                    duration,
                    duration_seconds: self
                        .info
                        .cycle_frequency
                        .map(|frequency| duration as f64 / frequency as f64),
                    metadata_id: timer.metadata_id,
                    rendered_name: timer
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

    fn resolve_timer(&self, timer: CpuTimerRef) -> Option<ResolvedTimer> {
        match timer {
            CpuTimerRef::Spec(spec_id) => Some(ResolvedTimer {
                timer,
                spec_id,
                name: self.spec(spec_id).map(|spec| spec.name),
                metadata_id: None,
                rendered_name: None,
            }),
            CpuTimerRef::Metadata(metadata_id) => {
                let metadata = self.metadata(metadata_id);
                Some(ResolvedTimer {
                    timer,
                    spec_id: metadata.map_or(u32::MAX, |metadata| metadata.spec_id),
                    name: metadata
                        .and_then(|metadata| self.spec(metadata.spec_id))
                        .map(|spec| spec.name),
                    metadata_id: Some(metadata_id),
                    rendered_name: metadata.and_then(|metadata| metadata.rendered_name),
                })
            }
        }
    }

    fn timer_name<'a>(&'a self, timer: ResolvedTimer) -> Cow<'a, str> {
        timer
            .name
            .and_then(|reference| text_at(&self.strings, reference))
            .map(Cow::Borrowed)
            .unwrap_or_else(|| Cow::Owned(format!("#{}", timer.spec_id)))
    }

    fn spec(&self, id: u32) -> Option<&SpecCatalogRecord> {
        self.specs
            .binary_search_by_key(&id, |spec| spec.id)
            .ok()
            .map(|index| &self.specs[index])
    }

    fn metadata(&self, id: u32) -> Option<&MetadataCatalogRecord> {
        self.metadata
            .binary_search_by_key(&id, |metadata| metadata.id)
            .ok()
            .map(|index| &self.metadata[index])
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct QueryHit {
    start_cycle: u64,
    end_cycle: u64,
    thread_id: u16,
    timer: CpuTimerRef,
}

#[derive(Clone, Copy, Debug)]
struct ResolvedTimer {
    timer: CpuTimerRef,
    spec_id: u32,
    name: Option<StringRef>,
    metadata_id: Option<u32>,
    rendered_name: Option<StringRef>,
}

#[derive(Clone, Copy, Debug)]
struct SpecCatalogRecord {
    id: u32,
    name: StringRef,
}

#[derive(Clone, Copy, Debug)]
struct MetadataCatalogRecord {
    id: u32,
    spec_id: u32,
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
    timer: CpuTimerRef,
}

#[derive(Clone, Debug)]
struct Page {
    begin_cycle: u64,
    end_cycle: u64,
    initial_stack: Vec<OpenScope>,
    begin_bits: Vec<u64>,
    metadata_bits: Vec<u64>,
    columns: PageColumns,
}

#[derive(Clone, Debug)]
enum PageColumns {
    Open {
        cycles: Vec<u64>,
        timer_ids: Vec<u32>,
    },
    Encoded {
        entry_count: usize,
        cycles: Vec<u8>,
        timer_ids: Vec<u8>,
    },
}

impl Page {
    fn new(cycle: u64, initial_stack: &[OpenScope]) -> Self {
        Self {
            begin_cycle: cycle,
            end_cycle: cycle,
            initial_stack: initial_stack.to_vec(),
            begin_bits: Vec::with_capacity(PAGE_ENTRIES / 64),
            metadata_bits: Vec::with_capacity(PAGE_ENTRIES / 128),
            columns: PageColumns::Open {
                cycles: Vec::with_capacity(PAGE_ENTRIES),
                timer_ids: Vec::with_capacity(PAGE_ENTRIES / 2),
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

    fn append_begin(&mut self, cycle: u64, timer: CpuTimerRef) {
        let entry_index = self.start_entry(cycle);
        self.begin_bits[entry_index / 64] |= 1_u64 << (entry_index % 64);
        let PageColumns::Open { cycles, timer_ids } = &mut self.columns else {
            unreachable!("encoded pages are immutable");
        };
        let timer_index = timer_ids.len();
        if timer_index % 64 == 0 {
            self.metadata_bits.push(0);
        }
        let timer_id = match timer {
            CpuTimerRef::Spec(id) => id,
            CpuTimerRef::Metadata(id) => {
                self.metadata_bits[timer_index / 64] |= 1_u64 << (timer_index % 64);
                id
            }
        };
        cycles.push(cycle);
        timer_ids.push(timer_id);
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
        let PageColumns::Open { cycles, timer_ids } = std::mem::replace(
            &mut self.columns,
            PageColumns::Encoded {
                entry_count: 0,
                cycles: Vec::new(),
                timer_ids: Vec::new(),
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
        let mut encoded_timer_ids = Vec::with_capacity(timer_ids.len().saturating_mul(3));
        let mut previous_id = 0_u32;
        for timer_id in timer_ids {
            append_varint(
                &mut encoded_timer_ids,
                zigzag_delta(u64::from(timer_id), u64::from(previous_id)),
            );
            previous_id = timer_id;
        }
        encoded_cycles.shrink_to_fit();
        encoded_timer_ids.shrink_to_fit();
        self.columns = PageColumns::Encoded {
            entry_count,
            cycles: encoded_cycles,
            timer_ids: encoded_timer_ids,
        };
    }

    fn is_begin(&self, index: usize) -> bool {
        self.begin_bits
            .get(index / 64)
            .is_some_and(|word| (word & (1_u64 << (index % 64))) != 0)
    }

    fn timer_ref(&self, timer_index: usize, id: u32) -> CpuTimerRef {
        if self
            .metadata_bits
            .get(timer_index / 64)
            .is_some_and(|word| (word & (1_u64 << (timer_index % 64))) != 0)
        {
            CpuTimerRef::Metadata(id)
        } else {
            CpuTimerRef::Spec(id)
        }
    }

    fn for_each_entry(&self, mut visit: impl FnMut(usize, u64, Option<CpuTimerRef>) -> bool) {
        match &self.columns {
            PageColumns::Open { cycles, timer_ids } => {
                let mut timer_index = 0;
                for (entry_index, &cycle) in cycles.iter().enumerate() {
                    let timer = self.is_begin(entry_index).then(|| {
                        let id = timer_ids[timer_index];
                        let timer = self.timer_ref(timer_index, id);
                        timer_index += 1;
                        timer
                    });
                    if !visit(entry_index, cycle, timer) {
                        break;
                    }
                }
            }
            PageColumns::Encoded {
                entry_count,
                cycles,
                timer_ids,
            } => {
                let mut cycle_cursor = 0;
                let mut timer_cursor = 0;
                let mut cycle = 0_u64;
                let mut timer_id = 0_u64;
                let mut timer_index = 0;
                for entry_index in 0..*entry_count {
                    let Some(delta) = read_varint(cycles, &mut cycle_cursor) else {
                        break;
                    };
                    cycle = apply_zigzag_delta(cycle, delta);
                    let decoded_timer = if self.is_begin(entry_index) {
                        let Some(delta) = read_varint(timer_ids, &mut timer_cursor) else {
                            break;
                        };
                        timer_id = apply_zigzag_delta(timer_id, delta);
                        let timer = self
                            .timer_ref(timer_index, u32::try_from(timer_id).unwrap_or(u32::MAX));
                        timer_index += 1;
                        Some(timer)
                    } else {
                        None
                    };
                    if !visit(entry_index, cycle, decoded_timer) {
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

    fn append_begin(&mut self, cycle: u64, timer: CpuTimerRef) {
        let page = self.page_for_append(cycle);
        page.append_begin(cycle, timer);
        self.stack.push(OpenScope {
            start_cycle: cycle,
            timer,
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

    fn enumerate(
        &self,
        start_cycle: u64,
        end_cycle: u64,
        mut emit: impl FnMut(CpuTimerRef, u64, u64),
    ) {
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
            .map(|open| (open.timer, open.start_cycle))
            .collect::<Vec<_>>();
        'pages: for page in &self.pages[page_index..] {
            let mut continue_page = true;
            page.for_each_entry(|_entry_index, cycle, timer| {
                if cycle > end_cycle {
                    continue_page = false;
                    return false;
                }
                if let Some(timer) = timer {
                    stack.push((timer, cycle));
                } else if let Some((timer, begin_cycle)) = stack.pop()
                    && cycle >= start_cycle
                {
                    emit(timer, begin_cycle, cycle);
                }
                true
            });
            if !continue_page {
                break 'pages;
            }
        }
        for (timer, begin_cycle) in stack {
            if begin_cycle <= end_cycle {
                emit(timer, begin_cycle, end_cycle);
            }
        }
    }
}

pub(crate) struct CpuMonotonicTimelineBuilder {
    begin_count: u64,
    begin_cycle: Option<u64>,
    end_cycle: Option<u64>,
    specs: FxHashMap<u32, SpecCatalogRecord>,
    metadata: FxHashMap<u32, MetadataCatalogRecord>,
    strings: Vec<u8>,
    threads: FxHashMap<u16, ThreadTimeline>,
}

impl CpuMonotonicTimelineBuilder {
    pub(crate) fn new() -> Self {
        Self {
            begin_count: 0,
            begin_cycle: None,
            end_cycle: None,
            specs: FxHashMap::default(),
            metadata: FxHashMap::default(),
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
                    .map(|page| {
                        ((page.begin_bits.len() + page.metadata_bits.len()) * size_of::<u64>())
                            as u64
                    })
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
        let mut specs = self.specs.into_values().collect::<Vec<_>>();
        specs.sort_unstable_by_key(|spec| spec.id);
        specs.shrink_to_fit();
        let mut metadata = self.metadata.into_values().collect::<Vec<_>>();
        metadata.sort_unstable_by_key(|metadata| metadata.id);
        metadata.shrink_to_fit();
        self.strings.shrink_to_fit();
        let catalog_allocated_bytes = (specs.capacity() * size_of::<SpecCatalogRecord>()
            + metadata.capacity() * size_of::<MetadataCatalogRecord>()
            + self.strings.capacity()) as u64;
        let allocated_bytes = column_allocated_bytes
            .saturating_add(page_allocated_bytes)
            .saturating_add(catalog_allocated_bytes);
        let stats = CpuMonotonicTimelineStats {
            thread_count: self.threads.len() as u64,
            page_count,
            entry_count,
            begin_count: self.begin_count,
            completed_scope_count,
            event_count: specs.len().saturating_add(metadata.len()) as u64,
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
            specs,
            metadata,
            strings: self.strings,
            threads: self.threads,
        }
    }
}

impl CpuTimelineCatalogSink for CpuMonotonicTimelineBuilder {
    fn register_spec(&mut self, spec_id: u32, name: &str) {
        if self.specs.contains_key(&spec_id) {
            return;
        }
        let name = append_text(&mut self.strings, name);
        self.specs
            .insert(spec_id, SpecCatalogRecord { id: spec_id, name });
    }

    fn register_metadata(&mut self, metadata_id: u32, spec_id: u32, rendered_name: Option<&str>) {
        if self.metadata.contains_key(&metadata_id) {
            return;
        }
        let rendered_name = rendered_name.map(|name| append_text(&mut self.strings, name));
        self.metadata.insert(
            metadata_id,
            MetadataCatalogRecord {
                id: metadata_id,
                spec_id,
                rendered_name,
            },
        );
    }
}

impl CpuMonotonicTimelineSink for CpuMonotonicTimelineBuilder {
    fn append_begin(&mut self, thread_id: u16, cycle: u64, timer: CpuTimerRef) {
        self.threads
            .entry(thread_id)
            .or_default()
            .append_begin(cycle, timer);
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
                PageColumns::Open { cycles, timer_ids } => {
                    cycles.len() * size_of::<u64>() + timer_ids.len() * size_of::<u32>()
                }
                PageColumns::Encoded {
                    cycles, timer_ids, ..
                } => cycles.len() + timer_ids.len(),
            };
            (columns
                + page.begin_bits.len() * size_of::<u64>()
                + page.metadata_bits.len() * size_of::<u64>()) as u64
        })
        .sum()
}

fn thread_column_allocated_bytes(timeline: &ThreadTimeline) -> u64 {
    timeline
        .pages
        .iter()
        .map(|page| {
            let columns = match &page.columns {
                PageColumns::Open { cycles, timer_ids } => {
                    cycles.capacity() * size_of::<u64>() + timer_ids.capacity() * size_of::<u32>()
                }
                PageColumns::Encoded {
                    cycles, timer_ids, ..
                } => cycles.capacity() + timer_ids.capacity(),
            };
            (columns
                + page.begin_bits.capacity() * size_of::<u64>()
                + page.metadata_bits.capacity() * size_of::<u64>()) as u64
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

    #[test]
    fn nested_scopes_query_from_parallel_columns() {
        let mut builder = CpuMonotonicTimelineBuilder::new();
        builder.register_spec(1, "outer");
        builder.register_spec(2, "inner");
        builder.append_begin(2, 10, CpuTimerRef::Spec(1));
        builder.append_begin(2, 20, CpuTimerRef::Spec(2));
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
        assert_eq!(index.stats().payload_bytes, 22);
        assert_eq!(index.stats().uncompressed_payload_bytes, 56);
    }

    #[test]
    fn completed_pages_remain_queryable_after_finish() {
        let mut builder = CpuMonotonicTimelineBuilder::new();
        builder.register_spec(1, "scope");
        for scope in 0..=(PAGE_ENTRIES / 2) {
            let begin = (scope * 2) as u64;
            builder.append_begin(7, begin, CpuTimerRef::Spec(1));
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
        builder.register_spec(1, "late");
        builder.register_spec(2, "early");
        builder.append_begin(10, 100, CpuTimerRef::Spec(1));
        builder.append_end(10, 110);
        builder.append_begin(2, 10, CpuTimerRef::Spec(2));
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
