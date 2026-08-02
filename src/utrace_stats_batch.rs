//! Bounded decode for Unreal `Stats.EventBatch2` sample streams.

use std::collections::BTreeMap;

use crate::utrace::{
    EventTypeInfo, StatSamplePoint, StatSampleSummary, StatsDashboard, TraceError, TraceErrorKind,
    event_data_size, parse_protocol5_aux, read_required_aux_bytes,
};

const MAX_SAMPLE_POINTS_PER_STAT: usize = 40;
const MAX_HOT_STATS: usize = 64;
const MAX_STAT_STATES: usize = 4_096;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
enum StatOp {
    Increment = 0,
    Decrement = 1,
    AddInteger = 2,
    SetInteger = 3,
    AddFloat = 4,
    SetFloat = 5,
}

impl StatOp {
    fn from_u8(value: u8) -> Option<Self> {
        match value {
            0 => Some(Self::Increment),
            1 => Some(Self::Decrement),
            2 => Some(Self::AddInteger),
            3 => Some(Self::SetInteger),
            4 => Some(Self::AddFloat),
            5 => Some(Self::SetFloat),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
struct RawStatOp {
    stat_id: u32,
    cycle: u64,
    op: StatOp,
    amount: f64,
}

#[derive(Clone, Debug, Default, PartialEq)]
struct StatValueState {
    samples: u64,
    first_cycle: Option<u64>,
    last_cycle: Option<u64>,
    min: Option<f64>,
    max: Option<f64>,
    latest: Option<f64>,
    sample_points: Vec<StatSamplePoint>,
}

impl StatValueState {
    fn apply(&mut self, cycle: u64, op: StatOp, amount: f64) {
        let value = match op {
            StatOp::Increment => self.latest.unwrap_or(0.0) + 1.0,
            StatOp::Decrement => self.latest.unwrap_or(0.0) - 1.0,
            StatOp::AddInteger | StatOp::AddFloat => self.latest.unwrap_or(0.0) + amount,
            StatOp::SetInteger | StatOp::SetFloat => amount,
        };
        self.samples = self.samples.saturating_add(1);
        self.first_cycle.get_or_insert(cycle);
        self.last_cycle = Some(cycle);
        self.min = Some(self.min.map_or(value, |min| min.min(value)));
        self.max = Some(self.max.map_or(value, |max| max.max(value)));
        self.latest = Some(value);
        if self.sample_points.len() < MAX_SAMPLE_POINTS_PER_STAT {
            self.sample_points.push(StatSamplePoint { cycle, value });
        }
    }
}

/// Streaming aggregator for `Stats.EventBatch2` payloads.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct StatsSampleProvider {
    sample_events: u64,
    unresolved_samples: u64,
    state_overflow: u64,
    malformed_batches: u64,
    states: BTreeMap<u32, StatValueState>,
}

impl StatsSampleProvider {
    pub(crate) fn record_batch(
        &mut self,
        event: &EventTypeInfo,
        data: &[u8],
        known_stats: &BTreeMap<u32, impl Sized>,
        base_offset: u64,
    ) -> Result<(), TraceError> {
        let aux = parse_protocol5_aux(data, event_data_size(event), base_offset)?;
        let batch = read_required_aux_bytes(event, &aux, "Data")?;
        match decode_event_batch2(batch, base_offset) {
            Ok(ops) => {
                for raw in ops {
                    self.sample_events = self.sample_events.saturating_add(1);
                    if !known_stats.contains_key(&raw.stat_id) {
                        self.unresolved_samples = self.unresolved_samples.saturating_add(1);
                    }
                    if let Some(state) = self.states.get_mut(&raw.stat_id) {
                        state.apply(raw.cycle, raw.op, raw.amount);
                    } else if self.states.len() < MAX_STAT_STATES {
                        let mut state = StatValueState::default();
                        state.apply(raw.cycle, raw.op, raw.amount);
                        self.states.insert(raw.stat_id, state);
                    } else {
                        self.state_overflow = self.state_overflow.saturating_add(1);
                    }
                }
                Ok(())
            }
            Err(error) => {
                self.malformed_batches = self.malformed_batches.saturating_add(1);
                Err(error)
            }
        }
    }

    pub(crate) fn apply_to_dashboard(self, dashboard: &mut StatsDashboard) {
        dashboard.sample_events = self.sample_events;
        dashboard.unresolved_samples = self.unresolved_samples;
        dashboard.sample_state_overflow = self.state_overflow;
        dashboard.malformed_batches = self.malformed_batches;
        let mut samples = self
            .states
            .into_iter()
            .map(|(id, state)| StatSampleSummary {
                id,
                samples: state.samples,
                first_cycle: state.first_cycle,
                last_cycle: state.last_cycle,
                min: state.min,
                max: state.max,
                latest: state.latest,
                sample_points: state.sample_points,
            })
            .collect::<Vec<_>>();
        samples.sort_by(|left, right| {
            right
                .samples
                .cmp(&left.samples)
                .then_with(|| left.id.cmp(&right.id))
        });
        samples.truncate(MAX_HOT_STATS);
        dashboard.samples = samples;
    }
}

fn decode_event_batch2(batch: &[u8], base_offset: u64) -> Result<Vec<RawStatOp>, TraceError> {
    let mut reader = StatsVarintReader::new(batch, base_offset);
    // EventBatch2 encoder resets LastCycle to 0 after each flush, so each
    // payload starts with an absolute timestamp as the first CycleDiff.
    let mut last_cycle = 0_u64;
    let mut ops = Vec::new();
    while !reader.is_empty() {
        let id_and_op = reader.read_u64()?;
        let op = StatOp::from_u8((id_and_op & 0x7) as u8).ok_or_else(|| {
            TraceError::new(
                TraceErrorKind::MalformedData,
                base_offset + reader.offset(),
                "Stats.EventBatch2.Data",
                format!("unknown stats opcode {}", id_and_op & 0x7),
            )
        })?;
        let stat_id = u32::try_from(id_and_op >> 3).map_err(|_| {
            TraceError::new(
                TraceErrorKind::MalformedData,
                base_offset + reader.offset(),
                "Stats.EventBatch2.Data",
                "stat id does not fit in u32",
            )
        })?;
        let cycle_diff = reader.read_u64()?;
        let cycle = last_cycle.checked_add(cycle_diff).ok_or_else(|| {
            TraceError::new(
                TraceErrorKind::MalformedData,
                base_offset + reader.offset(),
                "Stats.EventBatch2.Data",
                "cycle overflow",
            )
        })?;
        last_cycle = cycle;

        let amount = match op {
            StatOp::Increment | StatOp::Decrement => 0.0,
            StatOp::AddInteger | StatOp::SetInteger => reader.read_zigzag()? as f64,
            StatOp::AddFloat | StatOp::SetFloat => reader.read_f64()?,
        };
        ops.push(RawStatOp {
            stat_id,
            cycle,
            op,
            amount,
        });
    }
    Ok(ops)
}

struct StatsVarintReader<'a> {
    bytes: &'a [u8],
    cursor: usize,
    base_offset: u64,
}

impl<'a> StatsVarintReader<'a> {
    const fn new(bytes: &'a [u8], base_offset: u64) -> Self {
        Self {
            bytes,
            cursor: 0,
            base_offset,
        }
    }

    const fn is_empty(&self) -> bool {
        self.cursor >= self.bytes.len()
    }

    const fn offset(&self) -> u64 {
        self.cursor as u64
    }

    #[inline]
    fn read_u64(&mut self) -> Result<u64, TraceError> {
        let start = self.cursor;
        let mut value = 0_u64;
        for shift in (0..=63).step_by(7) {
            if self.cursor >= self.bytes.len() {
                return Err(TraceError::new(
                    TraceErrorKind::MalformedData,
                    self.base_offset + u64::try_from(start).unwrap_or(0),
                    "Stats.EventBatch2.Data",
                    "truncated 7-bit varint",
                ));
            }
            let byte = self.bytes[self.cursor];
            self.cursor += 1;
            value |= u64::from(byte & 0x7f) << shift;
            if (byte & 0x80) == 0 {
                return Ok(value);
            }
        }
        Err(TraceError::new(
            TraceErrorKind::MalformedData,
            self.base_offset + u64::try_from(start).unwrap_or(0),
            "Stats.EventBatch2.Data",
            "7-bit varint is too large",
        ))
    }

    fn read_zigzag(&mut self) -> Result<i64, TraceError> {
        let encoded = self.read_u64()?;
        let value = (encoded >> 1) as i64;
        Ok(if (encoded & 1) != 0 { !value } else { value })
    }

    fn read_f64(&mut self) -> Result<f64, TraceError> {
        if self.cursor + 8 > self.bytes.len() {
            return Err(TraceError::new(
                TraceErrorKind::MalformedData,
                self.base_offset + self.offset(),
                "Stats.EventBatch2.Data",
                "truncated float64 payload",
            ));
        }
        let bytes: [u8; 8] = self.bytes[self.cursor..self.cursor + 8]
            .try_into()
            .expect("length checked");
        self.cursor += 8;
        Ok(f64::from_le_bytes(bytes))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::utrace::{EventFlags, FieldFamily, FieldInfo, StatSpecSummary, StatsDashboard};

    fn push_varint(bytes: &mut Vec<u8>, mut value: u64) {
        while value >= 0x80 {
            bytes.push(((value & 0x7f) as u8) | 0x80);
            value >>= 7;
        }
        bytes.push(value as u8);
    }

    fn push_zigzag(bytes: &mut Vec<u8>, value: i64) {
        let encoded = ((value << 1) ^ (value >> 63)) as u64;
        push_varint(bytes, encoded);
    }

    fn batch_event() -> EventTypeInfo {
        EventTypeInfo {
            uid: 1,
            logger: "Stats".to_owned(),
            event: "EventBatch2".to_owned(),
            flags: EventFlags {
                important: false,
                maybe_has_aux: true,
                no_sync: false,
                definition: false,
            },
            fields: vec![FieldInfo {
                name: "Data".to_owned(),
                offset: 0,
                size: 0,
                family: FieldFamily::Regular,
                type_name: "array".to_owned(),
                ref_uid: None,
            }],
        }
    }

    fn encoded_batch(stat_id: u32) -> Vec<u8> {
        let mut payload = Vec::new();
        push_varint(&mut payload, (u64::from(stat_id) << 3) | 3);
        push_varint(&mut payload, 1);
        push_zigzag(&mut payload, 1);
        let pack = 1_u32 | (u32::try_from(payload.len()).unwrap() << 13);
        let mut data = pack.to_le_bytes().to_vec();
        data.extend_from_slice(&payload);
        data.push(3);
        data
    }

    #[test]
    fn decodes_set_integer_add_float_and_increment() {
        let mut batch = Vec::new();
        push_varint(&mut batch, (10 << 3) | 3); // SetInteger
        push_varint(&mut batch, 100);
        push_zigzag(&mut batch, 42);
        push_varint(&mut batch, (10 << 3) | 4); // AddFloat
        push_varint(&mut batch, 5);
        batch.extend_from_slice(&1.5_f64.to_le_bytes());
        push_varint(&mut batch, 10 << 3); // Increment (op 0)
        push_varint(&mut batch, 1);

        let mut provider = StatsSampleProvider::default();
        let ops = decode_event_batch2(&batch, 0).unwrap();
        for raw in ops {
            provider
                .states
                .entry(raw.stat_id)
                .or_default()
                .apply(raw.cycle, raw.op, raw.amount);
            provider.sample_events += 1;
        }
        let state = provider.states.get(&10).unwrap();
        assert_eq!(state.samples, 3);
        assert_eq!(state.latest, Some(44.5));
        assert_eq!(state.min, Some(42.0));
        assert_eq!(state.max, Some(44.5));
    }

    #[test]
    fn rejects_unknown_opcode() {
        let mut batch = Vec::new();
        push_varint(&mut batch, (1 << 3) | 7);
        push_varint(&mut batch, 1);
        assert!(decode_event_batch2(&batch, 0).is_err());
    }

    #[test]
    fn distinct_stat_state_catalog_is_bounded() {
        let mut provider = StatsSampleProvider::default();
        let event = batch_event();
        let known_stats = BTreeMap::<u32, ()>::new();
        for stat_id in 0..=4_096 {
            provider
                .record_batch(&event, &encoded_batch(stat_id), &known_stats, 0)
                .unwrap();
        }

        assert_eq!(provider.states.len(), 4_096);
        let mut dashboard = StatsDashboard::default();
        provider.apply_to_dashboard(&mut dashboard);
        assert_eq!(dashboard.sample_events, 4_097);
        assert_eq!(dashboard.sample_state_overflow, 1);
    }

    #[test]
    fn unresolved_samples_are_compared_with_the_spec_catalog() {
        let mut provider = StatsSampleProvider::default();
        let known_stats = BTreeMap::from([(1_u32, ())]);
        provider
            .record_batch(&batch_event(), &encoded_batch(2), &known_stats, 0)
            .unwrap();
        let mut dashboard = StatsDashboard {
            stats: vec![StatSpecSummary {
                id: 1,
                name: "Known".to_owned(),
                description: String::new(),
                group: String::new(),
                is_floating_point: false,
                is_memory: false,
                should_clear_every_frame: false,
            }],
            ..StatsDashboard::default()
        };

        provider.apply_to_dashboard(&mut dashboard);

        assert_eq!(dashboard.unresolved_samples, 1);
    }
}
