#[cfg(feature = "utrace")]
fn main() {
    let path = std::env::args()
        .nth(1)
        .expect("usage: utrace_memory_probe <trace>");
    let bytes = std::fs::read(&path).expect("read trace");
    let mut session = uasset_parser::utrace::ProgressiveDashboardSession::new(
        uasset_parser::utrace::DashboardOptions::default(),
    );
    for chunk in bytes.chunks(1024 * 1024) {
        session.push_chunk(chunk).expect("stream trace chunk");
    }
    let (_, _, index) = session
        .finish_with_inventory_and_memory_timeline_index()
        .expect("finish dashboard with memory index");
    println!(
        "intervals={} indexed={} truncated={}",
        index.info().total_interval_count,
        index.info().indexed_interval_count,
        index.info().truncated,
    );
}

#[cfg(not(feature = "utrace"))]
fn main() {
    panic!("utrace_memory_probe requires the utrace feature");
}
