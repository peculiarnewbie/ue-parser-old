#![cfg(feature = "utrace")]

use std::time::Instant;

use utrace_parser::utrace::{
    CpuMonotonicTimelineIndex, CpuTimelineQuery, CpuTimerStatsQuery, DashboardOptions,
    GpuTimelineMemoryIndex, ProgressiveDashboardSession, TraceDashboard, TraceInventory,
};

/// A real-capture correctness probe and native counterpart to the browser harness.
/// Save UTRACE_PROFILE_OUTPUT before/after an optimization to compare all providers,
/// inventory, and exact timeline/range aggregation results byte for byte.
#[test]
#[ignore = "requires UTRACE_FIXTURE; optionally set UTRACE_PROFILE_OUTPUT"]
fn progressive_capture_profile() {
    let path = std::env::var_os("UTRACE_FIXTURE").expect("set UTRACE_FIXTURE to a real capture");
    let bytes = std::fs::read(path).expect("read UTRACE_FIXTURE");
    let options = DashboardOptions {
        max_frames: Some(100_000),
        ..DashboardOptions::default()
    };
    let started = Instant::now();
    #[cfg(feature = "utrace-parallel")]
    let mut session = ProgressiveDashboardSession::new_with_parallel_cpu_timeline(options);
    #[cfg(not(feature = "utrace-parallel"))]
    let mut session = ProgressiveDashboardSession::new_with_eager_cpu_timeline(options);
    for chunk in bytes.chunks(1024 * 1024) {
        session.push_chunk(chunk).unwrap();
    }
    let push_ms = started.elapsed().as_secs_f64() * 1000.0;
    let finish_started = Instant::now();
    let (dashboard, inventory, index, gpu_index) = session
        .finish_with_inventory_and_monotonic_timeline_index()
        .unwrap();
    let finish_ms = finish_started.elapsed().as_secs_f64() * 1000.0;
    let output = capture_snapshot(&dashboard, &inventory, &index, &gpu_index);
    #[cfg(feature = "utrace-parallel")]
    {
        let mut eager = ProgressiveDashboardSession::new_with_eager_cpu_timeline(options);
        for chunk in bytes.chunks(257 * 1024) {
            eager.push_chunk(chunk).unwrap();
        }
        let (dashboard, inventory, eager_index, eager_gpu_index) = eager
            .finish_with_inventory_and_monotonic_timeline_index()
            .unwrap();
        assert!(
            output == capture_snapshot(&dashboard, &inventory, &eager_index, &eager_gpu_index),
            "parallel pages differ from eager decode across different chunk boundaries"
        );
        assert_eq!(index.stats(), eager_index.stats());
    }
    if let Some(path) = std::env::var_os("UTRACE_PROFILE_REFERENCE") {
        let reference = std::fs::read(path).expect("read UTRACE_PROFILE_REFERENCE");
        assert!(
            output == reference,
            "progressive output differs from reference"
        );
    }
    if let Some(path) = std::env::var_os("UTRACE_PROFILE_OUTPUT") {
        std::fs::write(path, &output).unwrap();
    }
    eprintln!(
        "push={push_ms:.2}ms finish={finish_ms:.2}ms total={:.2}ms output={}B timeline={}B",
        push_ms + finish_ms,
        output.len(),
        index.stats().allocated_bytes,
    );
}

fn capture_snapshot(
    dashboard: &TraceDashboard,
    inventory: &TraceInventory,
    index: &CpuMonotonicTimelineIndex,
    gpu_index: &GpuTimelineMemoryIndex,
) -> Vec<u8> {
    assert!(index.stats().completed_scope_count > 0);
    let begin = index.info().begin_cycle.unwrap();
    let end = index.info().end_cycle.unwrap();
    assert!(end > begin);
    let mut queries = Vec::new();
    let mut timers = Vec::new();
    for position in 0..=10 {
        let start = begin + (end - begin) / 11 * position;
        let stop = start + (end - begin) / 1000;
        queries.push(
            index
                .query(&CpuTimelineQuery {
                    start_cycle: Some(start),
                    end_cycle: Some(stop),
                    limit: Some(500),
                    ..CpuTimelineQuery::default()
                })
                .unwrap(),
        );
        timers.push(
            index
                .aggregate_timers(&CpuTimerStatsQuery {
                    start_cycle: Some(start),
                    end_cycle: Some(stop),
                    ..CpuTimerStatsQuery::default()
                })
                .unwrap(),
        );
    }
    assert!(queries.iter().any(|result| !result.intervals.is_empty()));
    assert!(timers.iter().any(|result| !result.timers.is_empty()));
    let gpu_queries = dashboard
        .gpu
        .frames
        .iter()
        .map(|frame| frame.frame_number)
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .map(|frame| gpu_index.query(frame, Some(10_000)))
        .collect::<Vec<_>>();
    serde_json::to_vec(&serde_json::json!({
        "dashboard": dashboard,
        "inventory": inventory,
        "index": index.info(),
        "queries": queries,
        "timers": timers,
        "gpu_queries": gpu_queries,
    }))
    .unwrap()
}
