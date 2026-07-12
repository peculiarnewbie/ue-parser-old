//! Bounded decode for Unreal CSV profiler sample events.

use std::collections::BTreeMap;

use crate::utrace::{
    CsvDashboard, CsvDurationSample, CsvValueSample, EventTypeInfo, TraceError, read_f32_field,
    read_i32_field, read_u8_field, read_u64_field,
};

const MAX_DURATION_SAMPLES: usize = 40;
const MAX_VALUE_SAMPLES: usize = 40;
const MAX_OPEN_STATS: usize = 4_096;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct OpenCsvStat {
    thread_id: u16,
    stat_id: u64,
    begin_cycle: u64,
}

/// Aggregates non-exclusive CSV Begin/End and CustomStat samples.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct CsvSampleProvider {
    sample_events: u64,
    begin_events: u64,
    end_events: u64,
    unmatched_ends: u64,
    custom_int_samples: u64,
    custom_float_samples: u64,
    open: Vec<OpenCsvStat>,
    duration_samples: Vec<CsvDurationSample>,
    value_samples: Vec<CsvValueSample>,
    unresolved_stats: u64,
}

impl CsvSampleProvider {
    pub(crate) fn record_event(
        &mut self,
        event: &EventTypeInfo,
        data: &[u8],
        thread_id: u16,
        known_stats: &BTreeMap<u64, impl Sized>,
        base_offset: u64,
    ) -> Result<(), TraceError> {
        match event.event.as_str() {
            "BeginStat" => {
                let stat_id = read_u64_field(event, data, "StatId", base_offset)?;
                let cycle = read_u64_field(event, data, "Cycle", base_offset)?;
                self.sample_events = self.sample_events.saturating_add(1);
                self.begin_events = self.begin_events.saturating_add(1);
                if !known_stats.contains_key(&stat_id) {
                    self.unresolved_stats = self.unresolved_stats.saturating_add(1);
                }
                if self.open.len() < MAX_OPEN_STATS {
                    self.open.push(OpenCsvStat {
                        thread_id,
                        stat_id,
                        begin_cycle: cycle,
                    });
                }
            }
            "EndStat" => {
                let stat_id = read_u64_field(event, data, "StatId", base_offset)?;
                let cycle = read_u64_field(event, data, "Cycle", base_offset)?;
                self.sample_events = self.sample_events.saturating_add(1);
                self.end_events = self.end_events.saturating_add(1);
                if let Some(index) = self
                    .open
                    .iter()
                    .rposition(|open| open.thread_id == thread_id && open.stat_id == stat_id)
                {
                    let open = self.open.remove(index);
                    let duration = cycle.saturating_sub(open.begin_cycle);
                    if self.duration_samples.len() < MAX_DURATION_SAMPLES {
                        self.duration_samples.push(CsvDurationSample {
                            thread_id,
                            stat_id,
                            begin_cycle: open.begin_cycle,
                            end_cycle: cycle,
                            duration_cycles: duration,
                        });
                    }
                } else {
                    self.unmatched_ends = self.unmatched_ends.saturating_add(1);
                }
            }
            "CustomStatInt" => {
                let stat_id = read_u64_field(event, data, "StatId", base_offset)?;
                let cycle = read_u64_field(event, data, "Cycle", base_offset)?;
                let value = i64::from(read_i32_compatible(event, data, base_offset)?);
                let op_type = read_u8_field(event, data, "OpType", base_offset)?;
                self.sample_events = self.sample_events.saturating_add(1);
                self.custom_int_samples = self.custom_int_samples.saturating_add(1);
                if !known_stats.contains_key(&stat_id) {
                    self.unresolved_stats = self.unresolved_stats.saturating_add(1);
                }
                if self.value_samples.len() < MAX_VALUE_SAMPLES {
                    self.value_samples.push(CsvValueSample {
                        thread_id,
                        stat_id,
                        cycle,
                        value: value as f64,
                        op_type,
                        kind: "int".to_owned(),
                    });
                }
            }
            "CustomStatFloat" => {
                let stat_id = read_u64_field(event, data, "StatId", base_offset)?;
                let cycle = read_u64_field(event, data, "Cycle", base_offset)?;
                let value = f64::from(read_f32_field(event, data, "Value", base_offset)?);
                let op_type = read_u8_field(event, data, "OpType", base_offset)?;
                self.sample_events = self.sample_events.saturating_add(1);
                self.custom_float_samples = self.custom_float_samples.saturating_add(1);
                if !known_stats.contains_key(&stat_id) {
                    self.unresolved_stats = self.unresolved_stats.saturating_add(1);
                }
                if self.value_samples.len() < MAX_VALUE_SAMPLES {
                    self.value_samples.push(CsvValueSample {
                        thread_id,
                        stat_id,
                        cycle,
                        value,
                        op_type,
                        kind: "float".to_owned(),
                    });
                }
            }
            _ => {}
        }
        Ok(())
    }

    pub(crate) fn apply_to_dashboard(self, dashboard: &mut CsvDashboard) {
        dashboard.sample_events = self.sample_events;
        dashboard.begin_events = self.begin_events;
        dashboard.end_events = self.end_events;
        dashboard.unmatched_ends = self.unmatched_ends;
        dashboard.custom_int_samples = self.custom_int_samples;
        dashboard.custom_float_samples = self.custom_float_samples;
        dashboard.open_begins = u64::try_from(self.open.len()).unwrap_or(u64::MAX);
        dashboard.sample_unresolved_stats = self.unresolved_stats;
        dashboard.duration_samples = self.duration_samples;
        dashboard.value_samples = self.value_samples;
    }
}

fn read_i32_compatible(
    event: &EventTypeInfo,
    data: &[u8],
    base_offset: u64,
) -> Result<i32, TraceError> {
    read_i32_field(event, data, "Value", base_offset)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::utrace::{FieldFamily, FieldInfo};

    fn event(name: &str, fields: &[(&str, u16, u16)]) -> EventTypeInfo {
        EventTypeInfo {
            uid: 1,
            logger: "CsvProfiler".to_owned(),
            event: name.to_owned(),
            flags: Default::default(),
            fields: fields
                .iter()
                .map(|(field_name, offset, size)| FieldInfo {
                    name: (*field_name).to_owned(),
                    offset: *offset,
                    size: *size,
                    family: FieldFamily::Regular,
                    type_name: "uint64".to_owned(),
                    ref_uid: None,
                })
                .collect(),
        }
    }

    #[test]
    fn pairs_begin_end_and_counts_unmatched_end() {
        let begin = event("BeginStat", &[("StatId", 0, 8), ("Cycle", 8, 8)]);
        let end = event("EndStat", &[("StatId", 0, 8), ("Cycle", 8, 8)]);
        let known = BTreeMap::from([(7_u64, ())]);
        let mut provider = CsvSampleProvider::default();

        let mut begin_data = 7_u64.to_le_bytes().to_vec();
        begin_data.extend_from_slice(&100_u64.to_le_bytes());
        provider
            .record_event(&begin, &begin_data, 3, &known, 0)
            .unwrap();

        let mut end_data = 7_u64.to_le_bytes().to_vec();
        end_data.extend_from_slice(&150_u64.to_le_bytes());
        provider
            .record_event(&end, &end_data, 3, &known, 0)
            .unwrap();

        let mut unmatched = 7_u64.to_le_bytes().to_vec();
        unmatched.extend_from_slice(&200_u64.to_le_bytes());
        provider
            .record_event(&end, &unmatched, 3, &known, 0)
            .unwrap();

        let mut dashboard = CsvDashboard::default();
        provider.apply_to_dashboard(&mut dashboard);
        assert_eq!(dashboard.sample_events, 3);
        assert_eq!(dashboard.begin_events, 1);
        assert_eq!(dashboard.end_events, 2);
        assert_eq!(dashboard.unmatched_ends, 1);
        assert_eq!(dashboard.duration_samples.len(), 1);
        assert_eq!(dashboard.duration_samples[0].duration_cycles, 50);
    }
}
