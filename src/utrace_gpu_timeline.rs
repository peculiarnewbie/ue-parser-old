//! Bounded in-memory GPU timeline index for repeated browser queries.

use std::collections::BTreeMap;

use rustc_hash::FxHashMap;

use crate::utrace::{GpuTimelineDashboard, GpuTimelineInterval, GpuTimelineIntervalKind};
use crate::utrace_timeline::SinkAppetite;

pub const DEFAULT_MAX_GPU_INDEXED_INTERVALS: usize = 1_000_000;
pub const MAX_GPU_QUERY_INTERVALS: usize = 10_000;
const MAX_GPU_INDEXED_FRAMES: usize = 100_000;
const NONE_SPEC_ID: u32 = u32::MAX;

pub(crate) trait GpuTimelineSink {
    fn note(
        &mut self,
        active_frame: Option<u32>,
        start_timestamp: u64,
        end_timestamp: u64,
    ) -> SinkAppetite;

    fn record(&mut self, interval: GpuTimelineInterval, active_frame: Option<u32>);
}

#[derive(Clone, Copy, Debug)]
struct StoredGpuInterval {
    queue_id: u32,
    kind: GpuTimelineIntervalKind,
    spec_id: u32,
    name_id: u32,
    start_timestamp: u64,
    end_timestamp: u64,
    duration: u64,
}

#[derive(Clone, Debug, Default)]
struct StoredGpuFrame {
    begin_timestamp: Option<u64>,
    end_timestamp: Option<u64>,
    interval_count: u64,
    truncated: bool,
    intervals: Vec<StoredGpuInterval>,
}

#[derive(Clone, Debug)]
pub struct GpuTimelineMemoryIndex {
    globally_truncated: bool,
    strings: Vec<String>,
    frames: BTreeMap<u32, StoredGpuFrame>,
}

impl GpuTimelineMemoryIndex {
    #[must_use]
    pub fn query(&self, frame_number: u32, limit: Option<usize>) -> GpuTimelineDashboard {
        let limit = limit.unwrap_or(500).clamp(1, MAX_GPU_QUERY_INTERVALS);
        let Some(frame) = self.frames.get(&frame_number) else {
            return GpuTimelineDashboard {
                frame_number,
                truncated: self.globally_truncated,
                ..GpuTimelineDashboard::default()
            };
        };
        let begin_timestamp = frame.begin_timestamp.unwrap_or(0);
        let intervals = frame
            .intervals
            .iter()
            .take(limit)
            .map(|record| GpuTimelineInterval {
                queue_id: record.queue_id,
                kind: record.kind,
                spec_id: (record.spec_id != NONE_SPEC_ID).then_some(record.spec_id),
                name: self.strings[usize::try_from(record.name_id).unwrap()].clone(),
                start_timestamp: record.start_timestamp,
                end_timestamp: record.end_timestamp,
                duration: record.duration,
            })
            .collect::<Vec<_>>();
        GpuTimelineDashboard {
            frame_number,
            begin_timestamp,
            end_timestamp: frame.end_timestamp.unwrap_or(begin_timestamp),
            interval_count: frame.interval_count,
            truncated: frame.truncated
                || frame.interval_count > u64::try_from(limit).unwrap_or(u64::MAX),
            intervals,
        }
    }
}

pub(crate) struct GpuTimelineIndexBuilder {
    max_intervals: usize,
    indexed_intervals: usize,
    globally_truncated: bool,
    strings: Vec<String>,
    string_ids: FxHashMap<String, u32>,
    frames: BTreeMap<u32, StoredGpuFrame>,
}

impl GpuTimelineIndexBuilder {
    pub(crate) fn new(max_intervals: usize) -> Self {
        Self {
            max_intervals,
            indexed_intervals: 0,
            globally_truncated: false,
            strings: Vec::new(),
            string_ids: FxHashMap::default(),
            frames: BTreeMap::new(),
        }
    }

    pub(crate) fn finish(self) -> GpuTimelineMemoryIndex {
        GpuTimelineMemoryIndex {
            globally_truncated: self.globally_truncated,
            strings: self.strings,
            frames: self.frames,
        }
    }

    fn intern(&mut self, value: String) -> u32 {
        if let Some(&id) = self.string_ids.get(value.as_str()) {
            return id;
        }
        let id = u32::try_from(self.strings.len()).unwrap();
        self.strings.push(value.clone());
        self.string_ids.insert(value, id);
        id
    }
}

impl GpuTimelineSink for GpuTimelineIndexBuilder {
    fn note(
        &mut self,
        active_frame: Option<u32>,
        start_timestamp: u64,
        end_timestamp: u64,
    ) -> SinkAppetite {
        let Some(frame_number) = active_frame else {
            return SinkAppetite::Full;
        };
        if !self.frames.contains_key(&frame_number) && self.frames.len() >= MAX_GPU_INDEXED_FRAMES {
            self.globally_truncated = true;
            return SinkAppetite::Full;
        }
        let frame = self.frames.entry(frame_number).or_default();
        frame.begin_timestamp = Some(
            frame
                .begin_timestamp
                .map_or(start_timestamp, |begin| begin.min(start_timestamp)),
        );
        frame.end_timestamp = Some(
            frame
                .end_timestamp
                .map_or(end_timestamp, |end| end.max(end_timestamp)),
        );
        frame.interval_count = frame.interval_count.saturating_add(1);
        if self.indexed_intervals >= self.max_intervals {
            self.globally_truncated = true;
            frame.truncated = true;
            return SinkAppetite::Full;
        }
        SinkAppetite::WantsRecord
    }

    fn record(&mut self, interval: GpuTimelineInterval, active_frame: Option<u32>) {
        let frame_number = active_frame.expect("GPU sink requested a record without a frame");
        let name_id = self.intern(interval.name);
        self.frames
            .get_mut(&frame_number)
            .expect("GPU sink frame was registered by note")
            .intervals
            .push(StoredGpuInterval {
                queue_id: interval.queue_id,
                kind: interval.kind,
                spec_id: interval.spec_id.unwrap_or(NONE_SPEC_ID),
                name_id,
                start_timestamp: interval.start_timestamp,
                end_timestamp: interval.end_timestamp,
                duration: interval.duration,
            });
        self.indexed_intervals += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(
        builder: &mut GpuTimelineIndexBuilder,
        frame_number: u32,
        name: &str,
        start_timestamp: u64,
        end_timestamp: u64,
    ) {
        if builder.note(Some(frame_number), start_timestamp, end_timestamp)
            == SinkAppetite::WantsRecord
        {
            builder.record(
                GpuTimelineInterval {
                    queue_id: 3,
                    kind: GpuTimelineIntervalKind::Breadcrumb,
                    spec_id: Some(7),
                    name: name.to_owned(),
                    start_timestamp,
                    end_timestamp,
                    duration: end_timestamp - start_timestamp,
                },
                Some(frame_number),
            );
        }
    }

    #[test]
    fn queries_one_frame_without_exposing_other_frames() {
        let mut builder = GpuTimelineIndexBuilder::new(10);
        record(&mut builder, 4, "Frame four", 10, 20);
        record(&mut builder, 5, "Frame five", 30, 50);
        let result = builder.finish().query(5, Some(10));

        assert_eq!(result.frame_number, 5);
        assert_eq!(result.begin_timestamp, 30);
        assert_eq!(result.end_timestamp, 50);
        assert_eq!(result.interval_count, 1);
        assert_eq!(result.intervals[0].name, "Frame five");
        assert!(!result.truncated);
    }

    #[test]
    fn reports_index_and_query_bounds() {
        let mut builder = GpuTimelineIndexBuilder::new(2);
        record(&mut builder, 9, "One", 10, 20);
        record(&mut builder, 9, "Two", 20, 30);
        record(&mut builder, 9, "Three", 30, 40);
        let index = builder.finish();
        let bounded = index.query(9, Some(1));

        assert_eq!(bounded.interval_count, 3);
        assert_eq!(bounded.intervals.len(), 1);
        assert!(bounded.truncated);
    }
}
