use rayon::prelude::*;

use super::*;

#[derive(Clone, Copy, Debug)]
pub(super) struct BatchContext {
    pub generation: u32,
    pub order: u32,
}

pub(super) struct ParallelCpuAggregate {
    pub batches: CpuBatchSummary,
    pub scope_totals: FxHashMap<u32, (u64, u64)>,
    pub metadata_scope_totals: FxHashMap<u32, (u64, u64)>,
    pub metadata_interval_state: CpuMetadataIntervalState,
    pub frame_scope_totals: FxHashMap<u32, FxHashMap<u32, (u64, u64)>>,
    pub frame_cycle_bounds: FxHashMap<u32, (u64, u64)>,
    pub thread_scope_totals: FxHashMap<u16, FxHashMap<u32, (u64, u64)>>,
}

struct StableMetadataCatalog<'a> {
    records: &'a FxHashMap<u32, CpuMetadataRecord>,
    introductions: &'a FxHashMap<u32, u32>,
}

impl CpuMetadataLookup for StableMetadataCatalog<'_> {
    fn get_at(&self, metadata_id: u32, generation: u64) -> Option<&CpuMetadataRecord> {
        (u64::from(self.introductions.get(&metadata_id).copied()?) <= generation)
            .then(|| self.records.get(&metadata_id))
            .flatten()
    }
}

#[derive(Default)]
struct ThreadCpuAggregate {
    thread_id: u16,
    batches: CpuBatchSummary,
    scope_totals: FxHashMap<u32, (u64, u64)>,
    metadata_scope_totals: FxHashMap<u32, (u64, u64)>,
    metadata_interval_state: CpuMetadataIntervalState,
    frame_scope_totals: FxHashMap<u32, FxHashMap<u32, (u64, u64)>>,
    frame_cycle_bounds: FxHashMap<u32, (u64, u64)>,
    thread_scope_totals: FxHashMap<u32, (u64, u64)>,
}

pub(super) struct ParallelCpuInputs<'a> {
    pub streams: &'a BTreeMap<u16, Vec<u8>>,
    pub registry: &'a BTreeMap<u16, &'a EventTypeInfo>,
    pub events_by_uid: &'a [Option<&'a EventTypeInfo>],
    pub event_kinds: &'a [DashboardEventKind],
    pub specs: &'a BTreeMap<u32, CpuScopeSpec>,
    pub metadata: &'a FxHashMap<u32, CpuMetadataRecord>,
    pub metadata_introductions: &'a FxHashMap<u32, u32>,
    pub batch_contexts: &'a FxHashMap<u16, Vec<BatchContext>>,
    pub known_scope_ids: Option<&'a [bool]>,
    pub cycle_frequency: Option<u64>,
    pub prologue_start_cycle: Option<u64>,
}

pub(super) fn aggregate(inputs: ParallelCpuInputs<'_>) -> Result<ParallelCpuAggregate, TraceError> {
    let work = inputs
        .batch_contexts
        .iter()
        .filter_map(|(&thread_id, contexts)| {
            inputs
                .streams
                .get(&thread_id)
                .map(|stream| (thread_id, stream.as_slice(), contexts.as_slice()))
        })
        .collect::<Vec<_>>();
    let catalog = StableMetadataCatalog {
        records: inputs.metadata,
        introductions: inputs.metadata_introductions,
    };

    let thread_results = work
        .par_iter()
        .map(|&(thread_id, stream, contexts)| {
            aggregate_thread(thread_id, stream, contexts, &inputs, &catalog)
        })
        .collect::<Vec<_>>();

    let mut aggregate = ParallelCpuAggregate {
        batches: CpuBatchSummary::default(),
        scope_totals: FxHashMap::default(),
        metadata_scope_totals: FxHashMap::default(),
        metadata_interval_state: CpuMetadataIntervalState::default(),
        frame_scope_totals: FxHashMap::default(),
        frame_cycle_bounds: FxHashMap::default(),
        thread_scope_totals: FxHashMap::default(),
    };
    let mut ordered_samples = Vec::new();
    for result in thread_results {
        let result = result?;
        merge_batches(&mut aggregate.batches, &result.batches);
        merge_totals(&mut aggregate.scope_totals, result.scope_totals);
        merge_totals(
            &mut aggregate.metadata_scope_totals,
            result.metadata_scope_totals,
        );
        merge_rendered_totals(
            &mut aggregate.metadata_interval_state.rendered_scope_totals,
            result.metadata_interval_state.rendered_scope_totals,
        );
        ordered_samples.extend(
            result
                .metadata_interval_state
                .sample_orders
                .into_iter()
                .zip(result.metadata_interval_state.samples),
        );
        merge_frame_totals(&mut aggregate.frame_scope_totals, result.frame_scope_totals);
        merge_frame_bounds(&mut aggregate.frame_cycle_bounds, result.frame_cycle_bounds);
        aggregate
            .thread_scope_totals
            .insert(result.thread_id, result.thread_scope_totals);
    }
    ordered_samples.sort_by_key(|(order, _)| *order);
    aggregate.metadata_interval_state.samples = ordered_samples
        .into_iter()
        .take(40)
        .map(|(_, sample)| sample)
        .collect();
    Ok(aggregate)
}

pub(super) fn aggregate_serial_fallback(
    inputs: ParallelCpuInputs<'_>,
    metadata_specs: &BTreeMap<u32, CpuMetadataSpec>,
    sync_count: u64,
    serial_dispatch_hint: Option<crate::utrace_dispatch::SerialDispatchHint>,
) -> Result<ParallelCpuAggregate, TraceError> {
    let mut aggregate = ParallelCpuAggregate {
        batches: CpuBatchSummary::default(),
        scope_totals: FxHashMap::default(),
        metadata_scope_totals: FxHashMap::default(),
        metadata_interval_state: CpuMetadataIntervalState::default(),
        frame_scope_totals: FxHashMap::default(),
        frame_cycle_bounds: FxHashMap::default(),
        thread_scope_totals: FxHashMap::default(),
    };
    let mut metadata = FxHashMap::<u32, CpuMetadataRecord>::default();
    let mut metadata_generation = 0_u64;
    let mut thread_states = FxHashMap::<u16, CpuBatchThreadState>::default();
    let mut metadata_stack_contexts = FxHashMap::<u16, CpuMetadataStackRuntimeState>::default();
    crate::utrace_dispatch::dispatch_normal_events_with_hint(
        inputs.streams,
        inputs.registry,
        sync_count,
        serial_dispatch_hint,
        |raw_event| {
            let Some(event) = inputs
                .events_by_uid
                .get(usize::from(raw_event.uid))
                .copied()
                .flatten()
            else {
                return Ok(());
            };
            match inputs
                .event_kinds
                .get(usize::from(raw_event.uid))
                .copied()
                .unwrap_or(DashboardEventKind::Unknown)
            {
                DashboardEventKind::CpuProfilerMetadata => {
                    let mut record = decode_cpu_metadata_record(event, raw_event.data, 0)?;
                    enrich_cpu_metadata_record(metadata_specs, &mut record);
                    metadata.insert(record.metadata_id, record);
                    metadata_generation = metadata_generation.saturating_add(1);
                }
                DashboardEventKind::CpuProfilerEventBatchV3 => {
                    let Some(data) = read_aux_bytes(event, raw_event.data, "Data", 0)? else {
                        return Ok(());
                    };
                    let thread_id = raw_event.thread_id;
                    let mut state = CpuBatchDecodeState {
                        batches: &mut aggregate.batches,
                        scope_totals: &mut aggregate.scope_totals,
                        metadata_scope_totals: &mut aggregate.metadata_scope_totals,
                        metadata_interval_state: &mut aggregate.metadata_interval_state,
                        metadata_stack_context: metadata_stack_contexts
                            .entry(thread_id)
                            .or_default(),
                        thread_state: thread_states.entry(thread_id).or_default(),
                        batch_base_cycle: raw_event.scope_cycle.or(inputs.prologue_start_cycle),
                        frame_scope_totals: &mut aggregate.frame_scope_totals,
                        frame_cycle_bounds: &mut aggregate.frame_cycle_bounds,
                        thread_scope_totals: aggregate
                            .thread_scope_totals
                            .entry(thread_id)
                            .or_default(),
                        timeline: None,
                        monotonic_timeline: None,
                        thread_id,
                        cycle_frequency: inputs.cycle_frequency,
                        known_scope_ids: inputs.known_scope_ids,
                        metadata_generation,
                    };
                    decode_cpu_batch::<false, _>(&data, inputs.specs, &metadata, &mut state)?;
                }
                DashboardEventKind::MetadataStack => {
                    apply_metadata_stack_event_to_cpu_context(
                        event,
                        raw_event.data,
                        metadata_stack_contexts
                            .entry(raw_event.thread_id)
                            .or_default(),
                        0,
                    )?;
                }
                _ => {}
            }
            Ok(())
        },
    )?;
    aggregate.batches.unterminated_scopes = aggregate.batches.unterminated_scopes.saturating_add(
        thread_states
            .values()
            .map(cpu_batch_thread_state_unterminated_scopes)
            .sum::<u64>(),
    );
    Ok(aggregate)
}

fn aggregate_thread(
    thread_id: u16,
    stream: &[u8],
    contexts: &[BatchContext],
    inputs: &ParallelCpuInputs<'_>,
    metadata: &StableMetadataCatalog<'_>,
) -> Result<ThreadCpuAggregate, TraceError> {
    let mut aggregate = ThreadCpuAggregate {
        thread_id,
        ..ThreadCpuAggregate::default()
    };
    let mut thread_state = CpuBatchThreadState::default();
    let mut metadata_stack_context = CpuMetadataStackRuntimeState::default();
    let mut batch_index = 0_usize;
    crate::utrace_dispatch::visit_normal_thread_events(
        thread_id,
        stream,
        inputs.registry,
        |raw_event| {
            let kind = inputs
                .event_kinds
                .get(usize::from(raw_event.uid))
                .copied()
                .unwrap_or(DashboardEventKind::Unknown);
            if kind != DashboardEventKind::CpuProfilerEventBatchV3 {
                return Ok(());
            }
            let context = contexts.get(batch_index).copied().ok_or_else(|| {
                TraceError::new(
                    TraceErrorKind::MalformedData,
                    0,
                    "CpuProfiler.EventBatchV3",
                    "parallel CPU batch order did not match the thread stream",
                )
            })?;
            batch_index = batch_index.saturating_add(1);
            let event = inputs
                .events_by_uid
                .get(usize::from(raw_event.uid))
                .copied()
                .flatten()
                .ok_or_else(|| {
                    TraceError::new(
                        TraceErrorKind::MalformedData,
                        0,
                        "CpuProfiler.EventBatchV3",
                        "parallel CPU batch event type was not registered",
                    )
                })?;
            let Some(data) = read_aux_bytes(event, raw_event.data, "Data", 0)? else {
                return Ok(());
            };
            aggregate
                .metadata_interval_state
                .begin_parallel_batch(u64::from(context.order));
            let mut state = CpuBatchDecodeState {
                batches: &mut aggregate.batches,
                scope_totals: &mut aggregate.scope_totals,
                metadata_scope_totals: &mut aggregate.metadata_scope_totals,
                metadata_interval_state: &mut aggregate.metadata_interval_state,
                metadata_stack_context: &mut metadata_stack_context,
                thread_state: &mut thread_state,
                batch_base_cycle: raw_event.scope_cycle.or(inputs.prologue_start_cycle),
                frame_scope_totals: &mut aggregate.frame_scope_totals,
                frame_cycle_bounds: &mut aggregate.frame_cycle_bounds,
                thread_scope_totals: &mut aggregate.thread_scope_totals,
                timeline: None,
                monotonic_timeline: None,
                thread_id,
                cycle_frequency: inputs.cycle_frequency,
                known_scope_ids: inputs.known_scope_ids,
                metadata_generation: u64::from(context.generation),
            };
            decode_cpu_batch::<false, _>(&data, inputs.specs, metadata, &mut state)
        },
    )?;
    if batch_index != contexts.len() {
        return Err(TraceError::new(
            TraceErrorKind::MalformedData,
            0,
            "CpuProfiler.EventBatchV3",
            "parallel CPU batch count did not match the serial dispatch pass",
        ));
    }
    aggregate.batches.unterminated_scopes = aggregate
        .batches
        .unterminated_scopes
        .saturating_add(cpu_batch_thread_state_unterminated_scopes(&thread_state));
    Ok(aggregate)
}

fn merge_batches(target: &mut CpuBatchSummary, source: &CpuBatchSummary) {
    target.count = target.count.saturating_add(source.count);
    target.decoded_records = target
        .decoded_records
        .saturating_add(source.decoded_records);
    target.intervals = target.intervals.saturating_add(source.intervals);
    target.unresolved_specs = target
        .unresolved_specs
        .saturating_add(source.unresolved_specs);
    target.metadata_scopes = target
        .metadata_scopes
        .saturating_add(source.metadata_scopes);
    target.restored_metadata_scopes = target
        .restored_metadata_scopes
        .saturating_add(source.restored_metadata_scopes);
    target.coroutine_records = target
        .coroutine_records
        .saturating_add(source.coroutine_records);
    target.unmatched_ends = target.unmatched_ends.saturating_add(source.unmatched_ends);
    target.unterminated_scopes = target
        .unterminated_scopes
        .saturating_add(source.unterminated_scopes);
    target.preamble_timeline_rebases = target
        .preamble_timeline_rebases
        .saturating_add(source.preamble_timeline_rebases);
    target.implausible_duration_count = target
        .implausible_duration_count
        .saturating_add(source.implausible_duration_count);
    target.implausible_duration_cycles = target
        .implausible_duration_cycles
        .saturating_add(source.implausible_duration_cycles);
}

fn merge_totals(target: &mut FxHashMap<u32, (u64, u64)>, source: FxHashMap<u32, (u64, u64)>) {
    for (id, (count, cycles)) in source {
        let total = target.entry(id).or_insert((0, 0));
        total.0 = total.0.saturating_add(count);
        total.1 = total.1.saturating_add(cycles);
    }
}

fn merge_rendered_totals(
    target: &mut BTreeMap<(u32, String), (u64, u64)>,
    source: BTreeMap<(u32, String), (u64, u64)>,
) {
    for (key, (count, cycles)) in source {
        let total = target.entry(key).or_insert((0, 0));
        total.0 = total.0.saturating_add(count);
        total.1 = total.1.saturating_add(cycles);
    }
}

fn merge_frame_totals(
    target: &mut FxHashMap<u32, FxHashMap<u32, (u64, u64)>>,
    source: FxHashMap<u32, FxHashMap<u32, (u64, u64)>>,
) {
    for (frame, totals) in source {
        merge_totals(target.entry(frame).or_default(), totals);
    }
}

fn merge_frame_bounds(target: &mut FxHashMap<u32, (u64, u64)>, source: FxHashMap<u32, (u64, u64)>) {
    for (frame, (begin, end)) in source {
        let bounds = target.entry(frame).or_insert((begin, end));
        bounds.0 = bounds.0.min(begin);
        bounds.1 = bounds.1.max(end);
    }
}
