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
pub mod version;

#[cfg(test)]
mod test_support;

pub use archive::{ArchiveError, ArchiveErrorKind, ArchiveLimits, Reader, Span};
pub use package::{Package, PackageError, PackageErrorKind, PackageSummary};
