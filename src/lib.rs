//! Read-only parser foundation for classic Unreal Engine asset packages.
//!
//! Dependencies flow from asset adapters down toward the archive layer:
//! `asset -> codec/property/schema -> package -> archive/version`.

pub mod archive;
pub mod asset;
pub mod codec;
pub mod package;
pub mod property;
pub mod schema;
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
pub(crate) mod utrace_memory;
#[cfg(feature = "utrace")]
pub(crate) mod utrace_modules;
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
pub mod version;
#[cfg(all(feature = "utrace-wasm", target_arch = "wasm32"))]
mod wasm;

#[cfg(test)]
mod test_support;

pub use archive::{ArchiveError, ArchiveErrorKind, ArchiveLimits, Reader, Span};
pub use package::{Package, PackageError, PackageErrorKind, PackageSummary};
