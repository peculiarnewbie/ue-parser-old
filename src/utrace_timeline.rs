//! Bounded, disk-backed CPU timeline indexes for repeated UTrace queries.

use std::error::Error;
use std::fmt;
use std::fs::{self, File};
use std::io::{self, BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use rustc_hash::FxHashMap;
use serde::Serialize;

use crate::utrace::CpuTimelineInterval;

const MAGIC: [u8; 8] = *b"UTLIDX01";
const VERSION: u32 = 1;
const HEADER_LEN: u64 = 96;
const RECORD_LEN: u64 = 52;
const NONE_STRING_ID: u32 = u32::MAX;
const MAX_STRING_COUNT: u32 = 1_000_000;
const MAX_STRING_BYTES: u32 = 1_048_576;
pub const DEFAULT_MAX_INDEXED_INTERVALS: usize = 1_000_000;
pub const MAX_QUERY_INTERVALS: usize = 10_000;

/// Metadata written next to a disk-backed CPU timeline index.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
pub struct CpuTimelineIndexInfo {
    pub source_bytes: u64,
    pub source_fingerprint: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cycle_frequency: Option<u64>,
    pub total_interval_count: u64,
    pub indexed_interval_count: u64,
    pub truncated: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub begin_cycle: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end_cycle: Option<u64>,
}

/// Bounded filters for a CPU timeline index query.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CpuTimelineQuery {
    pub start_cycle: Option<u64>,
    pub end_cycle: Option<u64>,
    pub thread_id: Option<u16>,
    pub search: Option<String>,
    pub limit: Option<usize>,
}

/// One indexed CPU timeline query result.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct CpuTimelineQueryResult {
    pub index: CpuTimelineIndexInfo,
    pub begin_cycle: u64,
    pub end_cycle: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_seconds: Option<f64>,
    pub interval_count: u64,
    /// True when the source index was bounded or this response hit its limit.
    pub truncated: bool,
    pub intervals: Vec<CpuTimelineInterval>,
}

/// Sorted CPU scope index retained by a browser/WASM session.
///
/// Unlike the native sidecar, this stays in process so range queries never
/// upload the capture or invoke a server after parsing completes.
#[derive(Clone, Debug)]
pub struct CpuTimelineMemoryIndex {
    info: CpuTimelineIndexInfo,
    strings: Vec<String>,
    records: Vec<StoredRecord>,
}

impl CpuTimelineMemoryIndex {
    #[must_use]
    pub fn info(&self) -> &CpuTimelineIndexInfo {
        &self.info
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
        let limit = query.limit.unwrap_or(500).clamp(1, MAX_QUERY_INTERVALS);
        let first = self
            .records
            .partition_point(|record| record.prefix_end_cycle < start_cycle);
        let after_last = self
            .records
            .partition_point(|record| record.start_cycle <= end_cycle);
        let needle = query.search.as_ref().map(|value| value.to_lowercase());
        let mut intervals = Vec::new();
        let mut interval_count = 0_u64;
        for record in &self.records[first..after_last] {
            if record.end_cycle < start_cycle
                || query
                    .thread_id
                    .is_some_and(|thread_id| record.thread_id != u32::from(thread_id))
                || !matches_search(record, &self.strings, needle.as_deref())
            {
                continue;
            }
            interval_count = interval_count.saturating_add(1);
            if intervals.len() < limit {
                intervals.push((*record).into_interval(&self.strings, self.info.cycle_frequency));
            }
        }
        Ok(CpuTimelineQueryResult {
            index: self.info.clone(),
            begin_cycle: start_cycle,
            end_cycle,
            duration_seconds: self
                .info
                .cycle_frequency
                .map(|frequency| end_cycle.saturating_sub(start_cycle) as f64 / frequency as f64),
            interval_count,
            truncated: self.info.truncated
                || interval_count > u64::try_from(limit).unwrap_or(u64::MAX),
            intervals,
        })
    }
}

/// Failures while writing or querying the sidecar index.
#[derive(Debug)]
#[non_exhaustive]
pub enum TimelineIndexError {
    Io(io::Error),
    Malformed(String),
    InvalidQuery(String),
    ResourceLimit(String),
}

impl fmt::Display for TimelineIndexError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "{error}"),
            Self::Malformed(message)
            | Self::InvalidQuery(message)
            | Self::ResourceLimit(message) => formatter.write_str(message),
        }
    }
}

impl Error for TimelineIndexError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::Malformed(_) | Self::InvalidQuery(_) | Self::ResourceLimit(_) => None,
        }
    }
}

impl From<io::Error> for TimelineIndexError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SinkAppetite {
    WantsRecord,
    Full,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct CpuTimelineIntervalView<'a> {
    pub(crate) thread_id: u16,
    pub(crate) spec_id: u32,
    pub(crate) name: &'a str,
    pub(crate) start_cycle: u64,
    pub(crate) end_cycle: u64,
    pub(crate) duration: u64,
    pub(crate) duration_seconds: Option<f64>,
    pub(crate) metadata_id: Option<u32>,
    pub(crate) rendered_name: Option<&'a str>,
}

impl CpuTimelineIntervalView<'_> {
    pub(crate) fn into_owned(self) -> CpuTimelineInterval {
        CpuTimelineInterval {
            thread_id: self.thread_id,
            spec_id: self.spec_id,
            name: self.name.to_owned(),
            start_cycle: self.start_cycle,
            end_cycle: self.end_cycle,
            duration: self.duration,
            duration_seconds: self.duration_seconds,
            metadata_id: self.metadata_id,
            rendered_name: self.rendered_name.map(str::to_owned),
        }
    }
}

impl<'a> From<&'a CpuTimelineInterval> for CpuTimelineIntervalView<'a> {
    fn from(interval: &'a CpuTimelineInterval) -> Self {
        Self {
            thread_id: interval.thread_id,
            spec_id: interval.spec_id,
            name: &interval.name,
            start_cycle: interval.start_cycle,
            end_cycle: interval.end_cycle,
            duration: interval.duration,
            duration_seconds: interval.duration_seconds,
            metadata_id: interval.metadata_id,
            rendered_name: interval.rendered_name.as_deref(),
        }
    }
}

pub(crate) trait CpuTimelineSink {
    /// Performs scalar accounting and decides whether constructing this interval
    /// would be useful. This must stay allocation-free: it is called for every
    /// decoded CPU scope, including scopes beyond a bounded collector's cap.
    fn note(&mut self, start_cycle: u64, end_cycle: u64, active_frame: Option<u32>)
    -> SinkAppetite;

    /// Receives an interval only after [`Self::note`] returned
    /// [`SinkAppetite::WantsRecord`].
    fn record(&mut self, interval: CpuTimelineIntervalView<'_>, active_frame: Option<u32>);
}

/// Stable identity of a trace source, embedded in a `.utix` header.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SourceIdentity {
    pub source_bytes: u64,
    pub fingerprint: u64,
}

impl SourceIdentity {
    #[must_use]
    pub fn from_bytes(source: &[u8]) -> Self {
        let mut fingerprint = SourceFingerprint::new();
        fingerprint.update(source);
        fingerprint.finish()
    }
}

/// Incremental FNV-1a identity calculator for sources that are read in chunks.
#[derive(Clone, Copy, Debug)]
pub struct SourceFingerprint {
    source_bytes: u64,
    hash: u64,
}

impl SourceFingerprint {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            source_bytes: 0,
            hash: 0xcbf2_9ce4_8422_2325,
        }
    }

    pub fn update(&mut self, bytes: &[u8]) {
        self.source_bytes = self
            .source_bytes
            .saturating_add(u64::try_from(bytes.len()).unwrap_or(u64::MAX));
        for &byte in bytes {
            self.hash ^= u64::from(byte);
            self.hash = self.hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }

    #[must_use]
    pub const fn finish(self) -> SourceIdentity {
        SourceIdentity {
            source_bytes: self.source_bytes,
            fingerprint: self.hash,
        }
    }
}

impl Default for SourceFingerprint {
    fn default() -> Self {
        Self::new()
    }
}

/// Opt-in request to persist a bounded CPU timeline sidecar while projecting a
/// dashboard. The source identity is maintained by the progressive session,
/// so callers only supply the bounded output destination and retention cap.
#[derive(Clone, Debug)]
pub struct TimelineIndexRequest {
    pub output: PathBuf,
    pub max_intervals: usize,
}

/// Outcome of an opt-in timeline-sidecar write. A write failure is deliberately
/// represented as data so a successfully decoded dashboard can still be used.
#[derive(Debug)]
pub struct TimelineIndexBuild {
    pub output: PathBuf,
    pub result: Result<CpuTimelineIndexInfo, TimelineIndexError>,
}

#[derive(Clone, Copy, Debug)]
struct StoredRecord {
    start_cycle: u64,
    end_cycle: u64,
    prefix_end_cycle: u64,
    duration: u64,
    thread_id: u32,
    spec_id: u32,
    metadata_id: u32,
    name_id: u32,
    rendered_name_id: u32,
}

impl StoredRecord {
    fn into_interval(
        self,
        strings: &[String],
        cycle_frequency: Option<u64>,
    ) -> CpuTimelineInterval {
        CpuTimelineInterval {
            thread_id: u16::try_from(self.thread_id).unwrap_or(u16::MAX),
            spec_id: self.spec_id,
            name: string_for(strings, self.name_id)
                .unwrap_or("<invalid name>")
                .to_owned(),
            start_cycle: self.start_cycle,
            end_cycle: self.end_cycle,
            duration: self.duration,
            duration_seconds: cycle_frequency
                .map(|frequency| self.duration as f64 / frequency as f64),
            metadata_id: (self.metadata_id != NONE_STRING_ID).then_some(self.metadata_id),
            rendered_name: (self.rendered_name_id != NONE_STRING_ID).then(|| {
                string_for(strings, self.rendered_name_id)
                    .unwrap_or("<invalid name>")
                    .to_owned()
            }),
        }
    }
}

#[derive(Debug)]
struct Header {
    info: CpuTimelineIndexInfo,
    string_count: u32,
    records_offset: u64,
}

pub(crate) struct CpuTimelineIndexBuilder {
    max_intervals: usize,
    retention: IndexRetention,
    total_interval_count: u64,
    truncated: bool,
    begin_cycle: Option<u64>,
    end_cycle: Option<u64>,
    strings: Vec<String>,
    string_ids: FxHashMap<String, u32>,
    records: Vec<StoredRecord>,
}

/// Retention policy for CPU scopes collected during a dashboard pass.
///
/// Native sidecars retain the first bounded records. Browser memory indexes
/// instead use a deterministic reservoir, so a large capture remains
/// navigable across its whole duration without retaining every scope.
#[derive(Clone, Copy, Debug)]
enum IndexRetention {
    Prefix,
    ReservoirSample {
        random_state: u64,
        replacement_slot: Option<usize>,
    },
}

impl CpuTimelineIndexBuilder {
    pub(crate) fn new(max_intervals: usize) -> Result<Self, TimelineIndexError> {
        Self::with_retention(max_intervals, IndexRetention::Prefix)
    }

    /// Builds a bounded, capture-wide representative sample for an in-memory
    /// browser index. The fixed seed makes results reproducible for a capture.
    pub(crate) fn new_reservoir_sample(max_intervals: usize) -> Result<Self, TimelineIndexError> {
        Self::with_retention(
            max_intervals,
            IndexRetention::ReservoirSample {
                random_state: 0x4d59_5df4_d0f3_3173,
                replacement_slot: None,
            },
        )
    }

    fn with_retention(
        max_intervals: usize,
        retention: IndexRetention,
    ) -> Result<Self, TimelineIndexError> {
        if max_intervals == 0 {
            return Err(TimelineIndexError::ResourceLimit(
                "timeline index max_intervals must be at least one".to_owned(),
            ));
        }
        Ok(Self {
            max_intervals,
            retention,
            total_interval_count: 0,
            truncated: false,
            begin_cycle: None,
            end_cycle: None,
            strings: Vec::new(),
            string_ids: FxHashMap::default(),
            records: Vec::new(),
        })
    }

    pub(crate) fn finish(
        mut self,
        output: &Path,
        source: SourceIdentity,
        cycle_frequency: Option<u64>,
    ) -> Result<CpuTimelineIndexInfo, TimelineIndexError> {
        let info = self.finalize(source, cycle_frequency)?;
        let string_count = u32::try_from(self.strings.len()).map_err(|_| {
            TimelineIndexError::ResourceLimit("timeline index string count exceeds u32".to_owned())
        })?;
        let dictionary_bytes = self.strings.iter().try_fold(0_u64, |total, value| {
            let byte_len = u64::try_from(value.len()).map_err(|_| {
                TimelineIndexError::ResourceLimit("timeline index string is too large".to_owned())
            })?;
            total
                .checked_add(4)
                .and_then(|value| value.checked_add(byte_len))
                .ok_or_else(|| {
                    TimelineIndexError::ResourceLimit(
                        "timeline index dictionary exceeds u64".to_owned(),
                    )
                })
        })?;
        let records_offset = HEADER_LEN.checked_add(dictionary_bytes).ok_or_else(|| {
            TimelineIndexError::ResourceLimit("timeline index offset exceeds u64".to_owned())
        })?;
        let header = Header {
            info: info.clone(),
            string_count,
            records_offset,
        };

        let temporary = temporary_path(output);
        let write_result = (|| -> Result<(), TimelineIndexError> {
            let file = File::create(&temporary)?;
            let mut writer = BufWriter::new(file);
            write_header(&mut writer, &header)?;
            for value in &self.strings {
                let byte_len = u32::try_from(value.len()).map_err(|_| {
                    TimelineIndexError::ResourceLimit(
                        "timeline index string length exceeds u32".to_owned(),
                    )
                })?;
                write_u32(&mut writer, byte_len)?;
                writer.write_all(value.as_bytes())?;
            }
            for record in &self.records {
                write_record(&mut writer, *record)?;
            }
            writer.flush()?;
            writer.get_ref().sync_all()?;
            Ok(())
        })();
        if let Err(error) = write_result {
            let _ = fs::remove_file(&temporary);
            return Err(error);
        }
        fs::rename(&temporary, output)?;
        Ok(info)
    }

    pub(crate) fn finish_in_memory(
        mut self,
        source: SourceIdentity,
        cycle_frequency: Option<u64>,
    ) -> Result<CpuTimelineMemoryIndex, TimelineIndexError> {
        let info = self.finalize(source, cycle_frequency)?;
        Ok(CpuTimelineMemoryIndex {
            info,
            strings: self.strings,
            records: self.records,
        })
    }

    fn finalize(
        &mut self,
        source: SourceIdentity,
        cycle_frequency: Option<u64>,
    ) -> Result<CpuTimelineIndexInfo, TimelineIndexError> {
        self.records.sort_by(|left, right| {
            left.start_cycle
                .cmp(&right.start_cycle)
                .then_with(|| left.end_cycle.cmp(&right.end_cycle))
                .then_with(|| left.thread_id.cmp(&right.thread_id))
                .then_with(|| left.spec_id.cmp(&right.spec_id))
        });
        let mut prefix_end_cycle = 0_u64;
        for record in &mut self.records {
            prefix_end_cycle = prefix_end_cycle.max(record.end_cycle);
            record.prefix_end_cycle = prefix_end_cycle;
        }

        let indexed_interval_count = u64::try_from(self.records.len()).map_err(|_| {
            TimelineIndexError::ResourceLimit("timeline index record count exceeds u64".to_owned())
        })?;
        Ok(CpuTimelineIndexInfo {
            source_bytes: source.source_bytes,
            source_fingerprint: source.fingerprint,
            cycle_frequency,
            total_interval_count: self.total_interval_count,
            indexed_interval_count,
            truncated: self.truncated,
            begin_cycle: self.begin_cycle,
            end_cycle: self.end_cycle,
        })
    }

    fn intern(&mut self, value: &str) -> u32 {
        if let Some(&id) = self.string_ids.get(value) {
            return id;
        }
        let id = u32::try_from(self.strings.len()).unwrap_or(u32::MAX);
        self.strings.push(value.to_owned());
        self.string_ids.insert(value.to_owned(), id);
        id
    }
}

impl CpuTimelineSink for CpuTimelineIndexBuilder {
    fn note(
        &mut self,
        start_cycle: u64,
        end_cycle: u64,
        _active_frame: Option<u32>,
    ) -> SinkAppetite {
        self.total_interval_count = self.total_interval_count.saturating_add(1);
        self.begin_cycle = Some(
            self.begin_cycle
                .map_or(start_cycle, |begin| begin.min(start_cycle)),
        );
        self.end_cycle = Some(self.end_cycle.map_or(end_cycle, |end| end.max(end_cycle)));
        if self.records.len() >= self.max_intervals {
            self.truncated = true;
            let retained = u64::try_from(self.records.len()).unwrap_or(u64::MAX);
            match &mut self.retention {
                IndexRetention::Prefix => SinkAppetite::Full,
                IndexRetention::ReservoirSample {
                    random_state,
                    replacement_slot,
                } => {
                    *replacement_slot = None;
                    let candidate = next_reservoir_value(random_state) % self.total_interval_count;
                    if candidate >= retained {
                        return SinkAppetite::Full;
                    }
                    let Some(slot) = usize::try_from(candidate).ok() else {
                        return SinkAppetite::Full;
                    };
                    *replacement_slot = Some(slot);
                    SinkAppetite::WantsRecord
                }
            }
        } else {
            SinkAppetite::WantsRecord
        }
    }

    fn record(&mut self, interval: CpuTimelineIntervalView<'_>, _active_frame: Option<u32>) {
        let CpuTimelineIntervalView {
            thread_id,
            spec_id,
            name,
            start_cycle,
            end_cycle,
            duration,
            duration_seconds: _,
            metadata_id,
            rendered_name,
        } = interval;
        let name_id = self.intern(name);
        let rendered_name_id = rendered_name
            .map(|value| self.intern(value))
            .unwrap_or(NONE_STRING_ID);
        let record = StoredRecord {
            start_cycle,
            end_cycle,
            prefix_end_cycle: end_cycle,
            duration,
            thread_id: u32::from(thread_id),
            spec_id,
            metadata_id: metadata_id.unwrap_or(NONE_STRING_ID),
            name_id,
            rendered_name_id,
        };
        if let IndexRetention::ReservoirSample {
            replacement_slot, ..
        } = &mut self.retention
        {
            if let Some(index) = replacement_slot.take() {
                if let Some(existing) = self.records.get_mut(index) {
                    *existing = record;
                }
                return;
            }
        }
        self.records.push(record);
    }
}

fn next_reservoir_value(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9e37_79b9_7f4a_7c15);
    let mut value = *state;
    value = (value ^ (value >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    value = (value ^ (value >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    value ^ (value >> 31)
}

/// Query a previously-built CPU timeline sidecar without reparsing the trace.
pub fn query_cpu_timeline_index(
    path: &Path,
    query: &CpuTimelineQuery,
) -> Result<CpuTimelineQueryResult, TimelineIndexError> {
    let file = File::open(path)?;
    let mut reader = BufReader::new(file);
    let header = read_header(&mut reader)?;
    let strings = read_dictionary(&mut reader, &header)?;
    let start_cycle = query.start_cycle.or(header.info.begin_cycle).unwrap_or(0);
    let end_cycle = query
        .end_cycle
        .or(header.info.end_cycle)
        .unwrap_or(start_cycle);
    if start_cycle > end_cycle {
        return Err(TimelineIndexError::InvalidQuery(
            "timeline start_cycle must not exceed end_cycle".to_owned(),
        ));
    }
    let requested_limit = query.limit.unwrap_or(500).clamp(1, MAX_QUERY_INTERVALS);
    let limit = u64::try_from(requested_limit).map_err(|_| {
        TimelineIndexError::ResourceLimit("timeline query limit exceeds u64".to_owned())
    })?;
    let first = first_overlapping_record(&mut reader, &header, start_cycle)?;
    let after_last = first_starting_after(&mut reader, &header, end_cycle)?;
    let needle = query.search.as_ref().map(|value| value.to_lowercase());
    let mut intervals = Vec::new();
    let mut interval_count = 0_u64;
    for index in first..after_last {
        let record = read_record_at(&mut reader, &header, index)?;
        if record.end_cycle < start_cycle
            || query
                .thread_id
                .is_some_and(|thread_id| record.thread_id != u32::from(thread_id))
            || !matches_search(&record, &strings, needle.as_deref())
        {
            continue;
        }
        interval_count = interval_count.saturating_add(1);
        if u64::try_from(intervals.len()).unwrap_or(u64::MAX) < limit {
            intervals.push(record.into_interval(&strings, header.info.cycle_frequency));
        }
    }
    Ok(CpuTimelineQueryResult {
        duration_seconds: header
            .info
            .cycle_frequency
            .map(|frequency| end_cycle.saturating_sub(start_cycle) as f64 / frequency as f64),
        index: header.info.clone(),
        begin_cycle: start_cycle,
        end_cycle,
        interval_count,
        truncated: header.info.truncated || interval_count > limit,
        intervals,
    })
}

fn matches_search(record: &StoredRecord, strings: &[String], needle: Option<&str>) -> bool {
    let Some(needle) = needle else {
        return true;
    };
    string_for(strings, record.name_id).is_some_and(|value| value.to_lowercase().contains(needle))
        || (record.rendered_name_id != NONE_STRING_ID
            && string_for(strings, record.rendered_name_id)
                .is_some_and(|value| value.to_lowercase().contains(needle)))
}

fn first_overlapping_record(
    reader: &mut (impl Read + Seek),
    header: &Header,
    start_cycle: u64,
) -> Result<u64, TimelineIndexError> {
    let mut low = 0_u64;
    let mut high = header.info.indexed_interval_count;
    while low < high {
        let middle = low + (high - low) / 2;
        if read_record_at(reader, header, middle)?.prefix_end_cycle >= start_cycle {
            high = middle;
        } else {
            low = middle.saturating_add(1);
        }
    }
    Ok(low)
}

fn first_starting_after(
    reader: &mut (impl Read + Seek),
    header: &Header,
    end_cycle: u64,
) -> Result<u64, TimelineIndexError> {
    let mut low = 0_u64;
    let mut high = header.info.indexed_interval_count;
    while low < high {
        let middle = low + (high - low) / 2;
        if read_record_at(reader, header, middle)?.start_cycle <= end_cycle {
            low = middle.saturating_add(1);
        } else {
            high = middle;
        }
    }
    Ok(low)
}

fn string_for(strings: &[String], id: u32) -> Option<&str> {
    strings.get(usize::try_from(id).ok()?).map(String::as_str)
}

fn temporary_path(output: &Path) -> PathBuf {
    let mut name = output.file_name().unwrap_or_default().to_os_string();
    name.push(format!(".{}.tmp", std::process::id()));
    output.with_file_name(name)
}

fn write_header(writer: &mut impl Write, header: &Header) -> Result<(), TimelineIndexError> {
    writer.write_all(&MAGIC)?;
    write_u32(writer, VERSION)?;
    write_u32(writer, u32::try_from(RECORD_LEN).unwrap_or(u32::MAX))?;
    write_u64(writer, header.info.source_bytes)?;
    write_u64(writer, header.info.source_fingerprint)?;
    write_u64(writer, header.info.cycle_frequency.unwrap_or(0))?;
    write_u64(writer, header.info.total_interval_count)?;
    write_u64(writer, header.info.indexed_interval_count)?;
    writer.write_all(&[u8::from(header.info.truncated)])?;
    writer.write_all(&[0; 7])?;
    write_u32(writer, header.string_count)?;
    write_u32(writer, 0)?;
    write_u64(writer, header.records_offset)?;
    write_u64(writer, header.info.begin_cycle.unwrap_or(0))?;
    write_u64(writer, header.info.end_cycle.unwrap_or(0))?;
    Ok(())
}

fn read_header(reader: &mut impl Read) -> Result<Header, TimelineIndexError> {
    let magic = read_array::<8>(reader)?;
    if magic != MAGIC {
        return Err(TimelineIndexError::Malformed(
            "not a uasset CPU timeline index".to_owned(),
        ));
    }
    let version = read_u32(reader)?;
    if version != VERSION {
        return Err(TimelineIndexError::Malformed(format!(
            "unsupported CPU timeline index version {version}"
        )));
    }
    let record_len = read_u32(reader)?;
    if u64::from(record_len) != RECORD_LEN {
        return Err(TimelineIndexError::Malformed(format!(
            "CPU timeline index record size {record_len} is unsupported"
        )));
    }
    let source_bytes = read_u64(reader)?;
    let source_fingerprint = read_u64(reader)?;
    let cycle_frequency = read_u64(reader)?;
    let total_interval_count = read_u64(reader)?;
    let indexed_interval_count = read_u64(reader)?;
    let truncated = read_array::<1>(reader)?[0] != 0;
    let _reserved = read_array::<7>(reader)?;
    let string_count = read_u32(reader)?;
    if string_count > MAX_STRING_COUNT {
        return Err(TimelineIndexError::ResourceLimit(format!(
            "CPU timeline index declares too many strings ({string_count})"
        )));
    }
    let _reserved = read_u32(reader)?;
    let records_offset = read_u64(reader)?;
    let begin_cycle = read_u64(reader)?;
    let end_cycle = read_u64(reader)?;
    if records_offset < HEADER_LEN {
        return Err(TimelineIndexError::Malformed(
            "CPU timeline index records precede its header".to_owned(),
        ));
    }
    Ok(Header {
        info: CpuTimelineIndexInfo {
            source_bytes,
            source_fingerprint,
            cycle_frequency: (cycle_frequency != 0).then_some(cycle_frequency),
            total_interval_count,
            indexed_interval_count,
            truncated,
            begin_cycle: (total_interval_count != 0).then_some(begin_cycle),
            end_cycle: (total_interval_count != 0).then_some(end_cycle),
        },
        string_count,
        records_offset,
    })
}

fn read_dictionary(
    reader: &mut (impl Read + Seek),
    header: &Header,
) -> Result<Vec<String>, TimelineIndexError> {
    reader.seek(SeekFrom::Start(HEADER_LEN))?;
    let capacity = usize::try_from(header.string_count).map_err(|_| {
        TimelineIndexError::ResourceLimit(
            "CPU timeline index string count exceeds usize".to_owned(),
        )
    })?;
    let mut strings = Vec::with_capacity(capacity);
    for _ in 0..header.string_count {
        let byte_len = read_u32(reader)?;
        if byte_len > MAX_STRING_BYTES {
            return Err(TimelineIndexError::ResourceLimit(format!(
                "CPU timeline index string exceeds {MAX_STRING_BYTES} bytes"
            )));
        }
        let byte_len = usize::try_from(byte_len).map_err(|_| {
            TimelineIndexError::ResourceLimit(
                "CPU timeline index string length exceeds usize".to_owned(),
            )
        })?;
        let mut bytes = vec![0; byte_len];
        reader.read_exact(&mut bytes)?;
        let value = String::from_utf8(bytes).map_err(|_| {
            TimelineIndexError::Malformed("CPU timeline index contains invalid UTF-8".to_owned())
        })?;
        strings.push(value);
    }
    let actual_records_offset = reader.stream_position()?;
    if actual_records_offset != header.records_offset {
        return Err(TimelineIndexError::Malformed(
            "CPU timeline index dictionary size does not match its header".to_owned(),
        ));
    }
    Ok(strings)
}

fn read_record_at(
    reader: &mut (impl Read + Seek),
    header: &Header,
    index: u64,
) -> Result<StoredRecord, TimelineIndexError> {
    if index >= header.info.indexed_interval_count {
        return Err(TimelineIndexError::Malformed(
            "CPU timeline index record lookup is out of bounds".to_owned(),
        ));
    }
    let offset = index
        .checked_mul(RECORD_LEN)
        .and_then(|offset| header.records_offset.checked_add(offset))
        .ok_or_else(|| {
            TimelineIndexError::ResourceLimit(
                "CPU timeline index record offset overflows u64".to_owned(),
            )
        })?;
    reader.seek(SeekFrom::Start(offset))?;
    Ok(StoredRecord {
        start_cycle: read_u64(reader)?,
        end_cycle: read_u64(reader)?,
        prefix_end_cycle: read_u64(reader)?,
        duration: read_u64(reader)?,
        thread_id: read_u32(reader)?,
        spec_id: read_u32(reader)?,
        metadata_id: read_u32(reader)?,
        name_id: read_u32(reader)?,
        rendered_name_id: read_u32(reader)?,
    })
}

fn write_record(writer: &mut impl Write, record: StoredRecord) -> Result<(), TimelineIndexError> {
    write_u64(writer, record.start_cycle)?;
    write_u64(writer, record.end_cycle)?;
    write_u64(writer, record.prefix_end_cycle)?;
    write_u64(writer, record.duration)?;
    write_u32(writer, record.thread_id)?;
    write_u32(writer, record.spec_id)?;
    write_u32(writer, record.metadata_id)?;
    write_u32(writer, record.name_id)?;
    write_u32(writer, record.rendered_name_id)?;
    Ok(())
}

fn write_u32(writer: &mut impl Write, value: u32) -> Result<(), TimelineIndexError> {
    writer.write_all(&value.to_le_bytes())?;
    Ok(())
}

fn write_u64(writer: &mut impl Write, value: u64) -> Result<(), TimelineIndexError> {
    writer.write_all(&value.to_le_bytes())?;
    Ok(())
}

fn read_u32(reader: &mut impl Read) -> Result<u32, TimelineIndexError> {
    Ok(u32::from_le_bytes(read_array(reader)?))
}

fn read_u64(reader: &mut impl Read) -> Result<u64, TimelineIndexError> {
    Ok(u64::from_le_bytes(read_array(reader)?))
}

fn read_array<const N: usize>(reader: &mut impl Read) -> Result<[u8; N], TimelineIndexError> {
    let mut value = [0; N];
    reader.read_exact(&mut value)?;
    Ok(value)
}

#[cfg(test)]
mod tests {
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;

    fn index_path(label: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("uasset-parser-{label}-{nonce}.utix"))
    }

    fn interval(
        thread_id: u16,
        name: &str,
        start_cycle: u64,
        end_cycle: u64,
    ) -> CpuTimelineInterval {
        CpuTimelineInterval {
            thread_id,
            spec_id: 7,
            name: name.to_owned(),
            start_cycle,
            end_cycle,
            duration: end_cycle - start_cycle,
            duration_seconds: None,
            metadata_id: None,
            rendered_name: None,
        }
    }

    fn record(builder: &mut CpuTimelineIndexBuilder, interval: CpuTimelineInterval) {
        if builder.note(interval.start_cycle, interval.end_cycle, None) == SinkAppetite::WantsRecord
        {
            builder.record((&interval).into(), None);
        }
    }

    fn source() -> SourceIdentity {
        SourceIdentity::from_bytes(b"trace")
    }

    #[test]
    fn index_query_finds_long_scope_that_started_before_window() {
        let path = index_path("overlap");
        let mut builder = CpuTimelineIndexBuilder::new(10).unwrap();
        record(&mut builder, interval(2, "Outer work", 10, 100));
        record(&mut builder, interval(3, "Inner work", 50, 60));
        let info = builder.finish(&path, source(), Some(100)).unwrap();
        assert_eq!(info.indexed_interval_count, 2);

        let result = query_cpu_timeline_index(
            &path,
            &CpuTimelineQuery {
                start_cycle: Some(80),
                end_cycle: Some(90),
                ..CpuTimelineQuery::default()
            },
        )
        .unwrap();
        assert_eq!(result.interval_count, 1);
        assert_eq!(result.intervals[0].name, "Outer work");
        assert_eq!(result.duration_seconds, Some(0.1));
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn memory_index_answers_the_same_overlapping_range_query() {
        let mut builder = CpuTimelineIndexBuilder::new(usize::MAX).unwrap();
        record(&mut builder, interval(2, "Outer work", 10, 100));
        record(&mut builder, interval(3, "Inner work", 50, 60));
        let index = builder.finish_in_memory(source(), Some(100)).unwrap();

        let result = index
            .query(&CpuTimelineQuery {
                start_cycle: Some(80),
                end_cycle: Some(90),
                ..CpuTimelineQuery::default()
            })
            .unwrap();
        assert_eq!(result.interval_count, 1);
        assert_eq!(result.intervals[0].name, "Outer work");
        assert_eq!(result.duration_seconds, Some(0.1));
        assert!(!result.truncated);
    }

    #[test]
    fn index_query_applies_thread_search_and_response_bounds() {
        let path = index_path("filters");
        let mut builder = CpuTimelineIndexBuilder::new(10).unwrap();
        record(&mut builder, interval(2, "Render Shadow", 10, 20));
        record(&mut builder, interval(3, "Game Tick", 15, 25));
        record(&mut builder, interval(2, "Render Lights", 20, 30));
        builder.finish(&path, source(), None).unwrap();

        let result = query_cpu_timeline_index(
            &path,
            &CpuTimelineQuery {
                thread_id: Some(2),
                search: Some("RENDER".to_owned()),
                limit: Some(1),
                ..CpuTimelineQuery::default()
            },
        )
        .unwrap();
        assert_eq!(result.interval_count, 2);
        assert!(result.truncated);
        assert_eq!(result.intervals.len(), 1);
        assert_eq!(result.intervals[0].thread_id, 2);
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn index_cap_is_reported_to_later_queries() {
        let path = index_path("cap");
        let mut builder = CpuTimelineIndexBuilder::new(1).unwrap();
        record(&mut builder, interval(2, "One", 10, 20));
        record(&mut builder, interval(2, "Two", 30, 40));
        let info = builder.finish(&path, source(), None).unwrap();
        assert_eq!(info.total_interval_count, 2);
        assert_eq!(info.indexed_interval_count, 1);
        assert!(info.truncated);

        let result = query_cpu_timeline_index(&path, &CpuTimelineQuery::default()).unwrap();
        assert!(result.truncated);
        assert_eq!(result.interval_count, 1);
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn sampled_memory_index_stays_bounded_without_becoming_an_early_capture_prefix() {
        let mut builder = CpuTimelineIndexBuilder::new_reservoir_sample(2).unwrap();
        for index in 0..64 {
            let start = u64::try_from(index).unwrap() * 10;
            record(
                &mut builder,
                interval(2, &format!("Scope {index}"), start, start + 5),
            );
        }
        let index = builder.finish_in_memory(source(), None).unwrap();

        assert_eq!(index.info().total_interval_count, 64);
        assert_eq!(index.info().indexed_interval_count, 2);
        assert!(index.info().truncated);

        let result = index.query(&CpuTimelineQuery::default()).unwrap();
        assert!(result.truncated);
        assert_eq!(result.intervals.len(), 2);
        assert!(
            result
                .intervals
                .iter()
                .any(|interval| interval.start_cycle >= 320),
            "a browser index needs capture-wide coverage, not only the first scopes"
        );
    }

    #[test]
    fn source_fingerprint_is_invariant_across_chunk_boundaries() {
        let source = b"a timeline source split at awkward boundaries";
        let whole = SourceIdentity::from_bytes(source);
        let mut chunked = SourceFingerprint::new();
        for chunk in source.chunks(3) {
            chunked.update(chunk);
        }
        assert_eq!(chunked.finish(), whole);
    }
}
