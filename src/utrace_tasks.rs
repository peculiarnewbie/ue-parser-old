//! TaskTrace lifecycle/wait summaries and thread-group membership helpers.

use std::collections::BTreeMap;

use crate::utrace::{
    EventTypeInfo, TaskNameSummary, TaskWaitSample, TasksDashboard, TraceError, event_data_size,
    parse_protocol5_aux, read_aux_string, read_required_aux_bytes, read_u32_field, read_u64_field,
};

const MAX_WAIT_SAMPLES: usize = 40;
const MAX_NAMED_TASKS: usize = 64;
const MAX_WAIT_TASK_IDS: usize = 32;
const MAX_OPEN_WAITS: usize = 4_096;

#[derive(Clone, Debug, Eq, PartialEq)]
struct OpenWait {
    thread_id: u16,
    start_cycle: u64,
    task_ids: Vec<u64>,
}

/// Aggregates TaskTrace lifecycle and wait intervals.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct TaskProvider {
    init_version: Option<u32>,
    created: u64,
    launched: u64,
    scheduled: u64,
    started: u64,
    finished: u64,
    completed: u64,
    destroyed: u64,
    subsequent_added: u64,
    wait_started: u64,
    wait_finished: u64,
    unmatched_wait_ends: u64,
    names: BTreeMap<u64, String>,
    open_waits: Vec<OpenWait>,
    wait_samples: Vec<TaskWaitSample>,
}

impl TaskProvider {
    pub(crate) fn record_event(
        &mut self,
        event: &EventTypeInfo,
        data: &[u8],
        thread_id: u16,
        base_offset: u64,
    ) -> Result<(), TraceError> {
        match event.event.as_str() {
            "Init" => {
                self.init_version = Some(read_u32_field(event, data, "Version", base_offset)?);
            }
            "Created" => {
                let _timestamp = read_u64_field(event, data, "Timestamp", base_offset)?;
                let _task_id = read_u64_field(event, data, "TaskId", base_offset)?;
                self.created = self.created.saturating_add(1);
            }
            "Launched" => {
                let aux = parse_protocol5_aux(data, event_data_size(event), base_offset)?;
                let task_id = read_u64_field(event, data, "TaskId", base_offset)?;
                let debug_name = read_aux_string(event, &aux, "DebugName").unwrap_or_default();
                self.launched = self.launched.saturating_add(1);
                if !debug_name.is_empty() && self.names.len() < MAX_NAMED_TASKS {
                    self.names.entry(task_id).or_insert(debug_name);
                }
            }
            "Scheduled" => {
                self.scheduled = self.scheduled.saturating_add(1);
            }
            "SubsequentAdded" => {
                self.subsequent_added = self.subsequent_added.saturating_add(1);
            }
            "Started" => {
                self.started = self.started.saturating_add(1);
            }
            "Finished" => {
                self.finished = self.finished.saturating_add(1);
            }
            "Completed" => {
                self.completed = self.completed.saturating_add(1);
            }
            "Destroyed" => {
                self.destroyed = self.destroyed.saturating_add(1);
            }
            "WaitingStarted" => {
                let timestamp = read_u64_field(event, data, "Timestamp", base_offset)?;
                let aux = parse_protocol5_aux(data, event_data_size(event), base_offset)?;
                let tasks_bytes = read_required_aux_bytes(event, &aux, "Tasks").unwrap_or(&[]);
                let mut task_ids = Vec::new();
                if tasks_bytes.len() % 8 == 0 {
                    for chunk in tasks_bytes.chunks_exact(8).take(MAX_WAIT_TASK_IDS) {
                        task_ids.push(u64::from_le_bytes(chunk.try_into().unwrap()));
                    }
                }
                self.wait_started = self.wait_started.saturating_add(1);
                if self.open_waits.len() < MAX_OPEN_WAITS {
                    self.open_waits.push(OpenWait {
                        thread_id,
                        start_cycle: timestamp,
                        task_ids,
                    });
                }
            }
            "WaitingFinished" => {
                let timestamp = read_u64_field(event, data, "Timestamp", base_offset)?;
                self.wait_finished = self.wait_finished.saturating_add(1);
                if let Some(index) = self
                    .open_waits
                    .iter()
                    .rposition(|wait| wait.thread_id == thread_id)
                {
                    let open = self.open_waits.remove(index);
                    if self.wait_samples.len() < MAX_WAIT_SAMPLES {
                        self.wait_samples.push(TaskWaitSample {
                            thread_id,
                            start_cycle: open.start_cycle,
                            end_cycle: timestamp,
                            duration_cycles: timestamp.saturating_sub(open.start_cycle),
                            task_ids: open.task_ids,
                        });
                    }
                } else {
                    self.unmatched_wait_ends = self.unmatched_wait_ends.saturating_add(1);
                }
            }
            _ => {}
        }
        Ok(())
    }

    pub(crate) fn dashboard(self) -> TasksDashboard {
        let mut named_tasks = self
            .names
            .into_iter()
            .map(|(task_id, debug_name)| TaskNameSummary {
                task_id,
                debug_name,
            })
            .collect::<Vec<_>>();
        named_tasks.sort_by(|left, right| left.task_id.cmp(&right.task_id));

        TasksDashboard {
            init_version: self.init_version,
            created: self.created,
            launched: self.launched,
            scheduled: self.scheduled,
            started: self.started,
            finished: self.finished,
            completed: self.completed,
            destroyed: self.destroyed,
            subsequent_added: self.subsequent_added,
            wait_count: u64::try_from(self.wait_samples.len()).unwrap_or(u64::MAX),
            wait_started: self.wait_started,
            wait_finished: self.wait_finished,
            unmatched_wait_ends: self.unmatched_wait_ends,
            open_waits: u64::try_from(self.open_waits.len()).unwrap_or(u64::MAX),
            wait_samples: self.wait_samples,
            named_tasks,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::utrace::{FieldFamily, FieldInfo};
    use std::collections::BTreeMap;

    fn event(name: &str, fields: &[(&str, u16, u16)]) -> EventTypeInfo {
        EventTypeInfo {
            uid: 1,
            logger: "TaskTrace".to_owned(),
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

    fn aux_array(field_index: u8, payload: &[u8]) -> Vec<u8> {
        let pack =
            1_u32 | (u32::from(field_index) << 8) | (u32::try_from(payload.len()).unwrap() << 13);
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&pack.to_le_bytes());
        bytes.extend_from_slice(payload);
        bytes
    }

    #[test]
    fn pairs_waiting_started_finished() {
        let started = event("WaitingStarted", &[("Timestamp", 0, 8), ("Tasks", 0, 0)]);
        // Give Tasks an array-looking field at index 1 for aux.
        let mut started = started;
        started.fields = vec![
            FieldInfo {
                name: "Timestamp".to_owned(),
                offset: 0,
                size: 8,
                family: FieldFamily::Regular,
                type_name: "uint64".to_owned(),
                ref_uid: None,
            },
            FieldInfo {
                name: "Tasks".to_owned(),
                offset: 0,
                size: 0,
                family: FieldFamily::Regular,
                type_name: "array".to_owned(),
                ref_uid: None,
            },
        ];
        let finished = event("WaitingFinished", &[("Timestamp", 0, 8)]);
        let mut provider = TaskProvider::default();

        let mut start_data = 100_u64.to_le_bytes().to_vec();
        start_data.extend_from_slice(&aux_array(1, &9_u64.to_le_bytes()));
        start_data.push(3);
        provider.record_event(&started, &start_data, 4, 0).unwrap();

        let end_data = 180_u64.to_le_bytes().to_vec();
        provider.record_event(&finished, &end_data, 4, 0).unwrap();

        let dashboard = provider.dashboard();
        assert_eq!(dashboard.wait_count, 1);
        assert_eq!(dashboard.wait_samples[0].duration_cycles, 80);
        assert_eq!(dashboard.wait_samples[0].task_ids, vec![9]);
        assert_eq!(dashboard.unmatched_wait_ends, 0);
    }

    #[test]
    fn counts_unmatched_wait_end() {
        let finished = event("WaitingFinished", &[("Timestamp", 0, 8)]);
        let mut provider = TaskProvider::default();
        provider
            .record_event(&finished, &50_u64.to_le_bytes(), 1, 0)
            .unwrap();
        let dashboard = provider.dashboard();
        assert_eq!(dashboard.unmatched_wait_ends, 1);
        assert_eq!(dashboard.wait_count, 0);
    }

    #[test]
    fn retains_group_membership_map_shape() {
        let membership: BTreeMap<u32, Vec<String>> =
            BTreeMap::from([(2_u32, vec!["BackgroundThreadPool".to_owned()])]);
        assert_eq!(
            membership
                .get(&2)
                .and_then(|groups| groups.last())
                .map(String::as_str),
            Some("BackgroundThreadPool")
        );
        assert!(!membership.contains_key(&1));
    }
}
