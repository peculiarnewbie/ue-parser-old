#[cfg(feature = "utrace")]
fn main() {
    let path = std::env::args()
        .nth(1)
        .expect("usage: utrace_memory_probe <trace> [dashboard|inventory|index|verify-inventory]");
    let mode = std::env::args()
        .nth(2)
        .unwrap_or_else(|| "index".to_owned());
    let bytes = std::fs::read(&path).expect("read trace");
    let decode_started = std::time::Instant::now();
    let mut session = utrace_parser::utrace::ProgressiveDashboardSession::new(
        utrace_parser::utrace::DashboardOptions::default(),
    );
    for chunk in bytes.chunks(1024 * 1024) {
        session.push_chunk(chunk).expect("stream trace chunk");
    }
    let push_elapsed = decode_started.elapsed();
    let finish_started = std::time::Instant::now();
    if mode == "dashboard" {
        session.finish().expect("finish dashboard");
        println!(
            "mode=dashboard decode_ms={} push_ms={} finish_ms={}",
            decode_started.elapsed().as_millis(),
            push_elapsed.as_millis(),
            finish_started.elapsed().as_millis(),
        );
        return;
    }
    if mode == "inventory" {
        session
            .finish_with_inventory()
            .expect("finish dashboard and inventory");
        println!(
            "mode=inventory decode_ms={} push_ms={} finish_ms={}",
            decode_started.elapsed().as_millis(),
            push_elapsed.as_millis(),
            finish_started.elapsed().as_millis(),
        );
        return;
    }
    if mode == "verify-inventory" {
        let (_, actual) = session
            .finish_with_inventory()
            .expect("finish progressive inventory");
        let expected =
            utrace_parser::utrace::inventory(&bytes).expect("build standalone inventory");
        assert_eq!(
            actual, expected,
            "progressive inventory changed the contract"
        );
        println!("mode=verify-inventory equal=true");
        return;
    }
    assert_eq!(mode, "index", "unknown probe mode");
    let (dashboard, _, index, gpu_index) = session
        .finish_with_inventory_and_memory_timeline_index()
        .expect("finish dashboard with memory index");
    let finish_elapsed = finish_started.elapsed();
    let decode_elapsed = decode_started.elapsed();
    println!(
        "mode=index decode_ms={} push_ms={} finish_ms={} cpu_intervals={} cpu_indexed={} cpu_truncated={}",
        decode_elapsed.as_millis(),
        push_elapsed.as_millis(),
        finish_elapsed.as_millis(),
        index.info().total_interval_count,
        index.info().indexed_interval_count,
        index.info().truncated,
    );
    if let Some(frame) = dashboard.gpu.frames.first() {
        let query_started = std::time::Instant::now();
        let timeline = gpu_index.query(frame.frame_number, Some(2_500));
        println!(
            "gpu_frame={} gpu_intervals={} gpu_returned={} gpu_truncated={} query_us={}",
            timeline.frame_number,
            timeline.interval_count,
            timeline.intervals.len(),
            timeline.truncated,
            query_started.elapsed().as_micros(),
        );
    }
}

#[cfg(not(feature = "utrace"))]
fn main() {
    panic!("utrace_memory_probe requires the utrace feature");
}
