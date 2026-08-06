//! Read-only parser for Unreal Engine UTrace captures.
//!
//! The bounded byte reader and archive error taxonomy come from UE Shed's
//! `uasset-parser` crate. UTrace owns the trace transport, event registry,
//! provider aggregation, dashboards, and progressive session model here.
#[cfg(feature = "utrace")]
pub mod utrace;
#[cfg(feature = "utrace")]
pub(crate) mod utrace_callstacks;
#[cfg(feature = "utrace")]
pub(crate) mod utrace_csv;
#[cfg(feature = "utrace")]
pub mod utrace_dispatch;
#[cfg(feature = "utrace")]
pub(crate) mod utrace_format_args;
#[cfg(feature = "utrace")]
pub(crate) mod utrace_gpu_timeline;
#[cfg(feature = "utrace")]
pub(crate) mod utrace_memory;
#[cfg(feature = "utrace")]
pub(crate) mod utrace_modules;
#[cfg(feature = "utrace")]
pub(crate) mod utrace_monotonic_timeline;
#[cfg(feature = "utrace")]
pub(crate) mod utrace_platform_file;
#[cfg(feature = "utrace")]
pub mod utrace_progress;
#[cfg(feature = "utrace")]
mod utrace_session;
#[cfg(feature = "utrace")]
pub(crate) mod utrace_stats_batch;
#[cfg(feature = "utrace")]
pub mod utrace_symbols;
#[cfg(feature = "utrace")]
pub(crate) mod utrace_tasks;
#[cfg(feature = "utrace")]
pub(crate) mod utrace_timeline;
#[cfg(all(feature = "utrace-wasm", target_arch = "wasm32"))]
mod wasm;
#[cfg(all(feature = "utrace-wasm-threads", target_arch = "wasm32"))]
pub use wasm_bindgen_rayon::init_thread_pool;

pub use uasset_parser::{ArchiveError, ArchiveErrorKind, ArchiveLimits, Reader, Span};
