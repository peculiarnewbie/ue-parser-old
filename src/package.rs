//! Classic Unreal package summary parsing.

use std::collections::BTreeMap;
use std::fmt;

use crate::archive::{ArchiveError, ArchiveErrorKind, Guid, IoHash, Reader, Span};
use crate::version::{PackageFlags, VersionContext};

pub const PACKAGE_FILE_TAG: u32 = 0x9E2A_83C1;
pub const PACKAGE_FILE_TAG_SWAPPED: u32 = 0xC183_2A9E;

const UE5_NAMES_REFERENCED_FROM_EXPORT_DATA: i32 = 1001;
const UE5_PAYLOAD_TOC: i32 = 1002;
const UE5_OPTIONAL_RESOURCES: i32 = 1003;
const UE5_REMOVE_OBJECT_EXPORT_PACKAGE_GUID: i32 = 1005;
const UE5_TRACK_OBJECT_EXPORT_IS_INHERITED: i32 = 1006;
const UE5_ADD_SOFT_OBJECT_PATH_LIST: i32 = 1008;
const UE5_DATA_RESOURCES: i32 = 1009;
const UE5_SCRIPT_SERIALIZATION_OFFSET: i32 = 1010;
const UE5_METADATA_SERIALIZATION_OFFSET: i32 = 1014;
const UE5_VERSE_CELLS: i32 = 1015;
const UE5_PACKAGE_SAVED_HASH: i32 = 1016;
const UE5_IMPORT_TYPE_HIERARCHIES: i32 = 1018;

const UE4_WORLD_LEVEL_INFO: i32 = 224;
const UE4_CHANGED_CHUNK_ID_TO_ARRAY: i32 = 326;
const UE4_ENGINE_VERSION_OBJECT: i32 = 336;
const UE4_LOAD_FOR_EDITOR_GAME: i32 = 365;
const UE4_ADD_STRING_ASSET_REFERENCES_MAP: i32 = 384;
const UE4_PACKAGE_SUMMARY_HAS_COMPATIBLE_ENGINE_VERSION: i32 = 444;
const UE4_SERIALIZE_TEXT_IN_PACKAGES: i32 = 459;
const UE4_COOKED_ASSETS_IN_EDITOR_SUPPORT: i32 = 485;
const UE4_NAME_HASHES_SERIALIZED: i32 = 504;
const UE4_PRELOAD_DEPENDENCIES_IN_COOKED_EXPORTS: i32 = 507;
const UE4_TEMPLATE_INDEX_IN_COOKED_EXPORTS: i32 = 508;
const UE4_ADDED_SEARCHABLE_NAMES: i32 = 510;
const UE4_64BIT_EXPORTMAP_SERIALSIZES: i32 = 511;
const UE4_ADDED_PACKAGE_SUMMARY_LOCALIZATION_ID: i32 = 516;
const UE4_ADDED_PACKAGE_OWNER: i32 = 518;
const UE4_NON_OUTER_PACKAGE_IMPORT: i32 = 520;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PackageErrorKind {
    MalformedData,
    UnsupportedFormat,
    UnsupportedVersion,
    UnsupportedCapability,
}

#[derive(Debug)]
pub struct PackageError {
    kind: PackageErrorKind,
    offset: Option<u64>,
    path: String,
    detail: String,
    source: Option<Box<ArchiveError>>,
}

impl PackageError {
    fn new(
        kind: PackageErrorKind,
        offset: Option<u64>,
        path: impl Into<String>,
        detail: impl Into<String>,
    ) -> Self {
        Self {
            kind,
            offset,
            path: path.into(),
            detail: detail.into(),
            source: None,
        }
    }

    #[must_use]
    pub const fn kind(&self) -> PackageErrorKind {
        self.kind
    }

    #[must_use]
    pub const fn offset(&self) -> Option<u64> {
        self.offset
    }

    #[must_use]
    pub fn path(&self) -> &str {
        &self.path
    }

    #[must_use]
    pub fn detail(&self) -> &str {
        &self.detail
    }
}

impl fmt::Display for PackageError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.offset {
            Some(offset) => write!(
                formatter,
                "{:?} at byte {offset} while reading {}: {}",
                self.kind, self.path, self.detail
            ),
            None => write!(
                formatter,
                "{:?} while reading {}: {}",
                self.kind, self.path, self.detail
            ),
        }
    }
}

impl std::error::Error for PackageError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.source
            .as_ref()
            .map(|source| source.as_ref() as &(dyn std::error::Error + 'static))
    }
}

impl From<ArchiveError> for PackageError {
    fn from(source: ArchiveError) -> Self {
        let kind = match source.kind() {
            ArchiveErrorKind::OutOfBounds
            | ArchiveErrorKind::InvalidSeek
            | ArchiveErrorKind::InvalidCount
            | ArchiveErrorKind::AllocationLimit
            | ArchiveErrorKind::MissingNullTerminator
            | ArchiveErrorKind::InvalidString
            | ArchiveErrorKind::InvalidNameReference
            | ArchiveErrorKind::IntegerOverflow => PackageErrorKind::MalformedData,
        };
        Self {
            kind,
            offset: Some(source.offset()),
            path: source.path().to_owned(),
            detail: source.detail().to_owned(),
            source: Some(Box::new(source)),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct FileOffset(u64);

impl FileOffset {
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TableLocation {
    pub count: u32,
    pub offset: FileOffset,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CustomVersion {
    pub key: Guid,
    pub version: i32,
    pub friendly_name: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GenerationInfo {
    pub export_count: u32,
    pub name_count: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EngineVersion {
    pub major: u16,
    pub minor: u16,
    pub patch: u16,
    pub changelist: u32,
    pub branch: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PackageSummary {
    pub span: Span,
    pub versions: VersionContext,
    pub custom_versions: Vec<CustomVersion>,
    pub saved_hash: IoHash,
    pub total_header_size: u32,
    pub package_name: String,
    pub names: TableLocation,
    pub soft_object_paths: Option<TableLocation>,
    pub localization_id: Option<String>,
    pub gatherable_text_data: Option<TableLocation>,
    pub exports: TableLocation,
    pub imports: TableLocation,
    pub cell_exports: Option<TableLocation>,
    pub cell_imports: Option<TableLocation>,
    pub metadata_offset: Option<FileOffset>,
    pub depends_offset: FileOffset,
    pub soft_package_references: Option<TableLocation>,
    pub searchable_names_offset: Option<FileOffset>,
    pub thumbnail_table_offset: FileOffset,
    pub import_type_hierarchies: Option<TableLocation>,
    pub persistent_guid: Option<Guid>,
    pub generations: Vec<GenerationInfo>,
    pub saved_by_engine_version: EngineVersion,
    pub compatible_with_engine_version: EngineVersion,
    pub compression_flags: u32,
    pub package_source: u32,
    pub asset_registry_data_offset: FileOffset,
    pub bulk_data_start_offset: u64,
    pub world_tile_info_data_offset: Option<FileOffset>,
    pub chunk_ids: Vec<i32>,
    pub preload_dependencies: Option<TableLocation>,
    pub names_referenced_from_export_data_count: u32,
    pub payload_toc_offset: Option<FileOffset>,
    pub data_resource_offset: Option<FileOffset>,
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum PackageIndex {
    Null,
    Import(u32),
    Export(u32),
}

impl PackageIndex {
    fn parse(value: i32) -> Self {
        match value.cmp(&0) {
            std::cmp::Ordering::Greater => {
                Self::Export(u32::try_from(value - 1).expect("positive i32 minus one fits in u32"))
            }
            std::cmp::Ordering::Less => Self::Import(value.unsigned_abs() - 1),
            std::cmp::Ordering::Equal => Self::Null,
        }
    }

    #[must_use]
    pub const fn from_raw(value: i32) -> Self {
        if value > 0 {
            Self::Export((value as u32) - 1)
        } else if value < 0 {
            Self::Import(value.unsigned_abs() - 1)
        } else {
            Self::Null
        }
    }
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ObjectPath(String);

impl ObjectPath {
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ObjectPath {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Import {
    pub class_package: crate::archive::NameRef,
    pub class_name: crate::archive::NameRef,
    pub outer_index: PackageIndex,
    pub object_name: crate::archive::NameRef,
    pub package_name: Option<crate::archive::NameRef>,
    pub import_optional: Option<bool>,
    pub object_path: ObjectPath,
    pub class_path: ObjectPath,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Export {
    pub class_index: PackageIndex,
    pub super_index: PackageIndex,
    pub template_index: Option<PackageIndex>,
    pub outer_index: PackageIndex,
    pub object_name: crate::archive::NameRef,
    pub object_flags: u32,
    pub serial_size: u64,
    pub serial_offset: FileOffset,
    pub forced_export: bool,
    pub not_for_client: bool,
    pub not_for_server: bool,
    pub inherited_instance: Option<bool>,
    pub package_flags: u32,
    pub not_always_loaded_for_editor_game: Option<bool>,
    pub is_asset: Option<bool>,
    pub generate_public_hash: Option<bool>,
    pub script_serialization_start_offset: Option<u64>,
    pub script_serialization_end_offset: Option<u64>,
    pub object_path: ObjectPath,
    pub class_path: Option<ObjectPath>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Package {
    pub summary: PackageSummary,
    pub names: Vec<String>,
    pub imports: Vec<Import>,
    pub exports: Vec<Export>,
}

impl Package {
    /// Parses the package summary plus name, import, and export maps.
    ///
    /// # Errors
    ///
    /// Returns an error if any table is malformed, unsupported, or points
    /// outside the package.
    pub fn parse(source: &[u8]) -> Result<Self, PackageError> {
        let summary = PackageSummary::parse(source)?;
        let mut reader = Reader::new(source);
        let names = read_name_map(&mut reader, &summary)?;
        let mut imports = read_import_map(&mut reader, &summary, &names)?;
        let mut exports = read_export_map(&mut reader, &summary, &names)?;

        for index in 0..imports.len() {
            let object_path = resolve_index_path(
                PackageIndex::Import(u32::try_from(index).expect("import index fits in u32")),
                &summary.package_name,
                &names,
                &imports,
                &exports,
                "Imports",
            )?;
            let class_path = resolve_import_class_path(&imports[index], &names)?;
            imports[index].object_path = object_path;
            imports[index].class_path = class_path;
        }

        for index in 0..exports.len() {
            let object_path = resolve_index_path(
                PackageIndex::Export(u32::try_from(index).expect("export index fits in u32")),
                &summary.package_name,
                &names,
                &imports,
                &exports,
                "Exports",
            )?;
            let class_path = match exports[index].class_index {
                PackageIndex::Null => None,
                class_index => Some(normalize_object_path(resolve_index_path(
                    class_index,
                    &summary.package_name,
                    &names,
                    &imports,
                    &exports,
                    "Exports.ClassIndex",
                )?)),
            };
            exports[index].object_path = object_path;
            exports[index].class_path = class_path;
        }

        Ok(Self {
            summary,
            names,
            imports,
            exports,
        })
    }

    #[must_use]
    pub fn resolve_name(&self, name: crate::archive::NameRef) -> Option<String> {
        resolve_name_ref(&self.names, name).ok()
    }

    #[must_use]
    pub fn resolve_index(&self, index: PackageIndex) -> Option<ObjectPath> {
        resolve_index_path(
            index,
            &self.summary.package_name,
            &self.names,
            &self.imports,
            &self.exports,
            "PackageIndex",
        )
        .ok()
        .map(normalize_object_path)
    }

    pub fn export_reader<'a>(
        &self,
        source: &'a [u8],
        export: &Export,
    ) -> Result<Reader<'a>, PackageError> {
        let reader = Reader::new(source);
        Ok(reader.bounded(
            Span::new(export.serial_offset.get(), export.serial_size)?,
            export.object_path.as_str(),
        )?)
    }
}

impl PackageSummary {
    /// Parses and validates a classic package summary.
    ///
    /// # Errors
    ///
    /// Returns an error for malformed data, unsupported versions, swapped
    /// endianness, cooked packages, unversioned properties, or invalid table
    /// ranges.
    pub fn parse(source: &[u8]) -> Result<Self, PackageError> {
        let mut reader = Reader::new(source);
        let start = reader.tell();

        let tag = reader.read_u32("Summary.Tag")?;
        match tag {
            PACKAGE_FILE_TAG => {}
            PACKAGE_FILE_TAG_SWAPPED => {
                return Err(PackageError::new(
                    PackageErrorKind::UnsupportedCapability,
                    Some(0),
                    "Summary.Tag",
                    "big-endian package byte swapping is not implemented",
                ));
            }
            _ => {
                return Err(PackageError::new(
                    PackageErrorKind::UnsupportedFormat,
                    Some(0),
                    "Summary.Tag",
                    format!("expected package tag 0x{PACKAGE_FILE_TAG:08X}, got 0x{tag:08X}"),
                ));
            }
        }

        let legacy_file_version = reader.read_i32("Summary.LegacyFileVersion")?;
        validate_legacy_version(legacy_file_version, reader.tell() - 4)?;
        let legacy_ue3 = if legacy_file_version != -4 {
            Some(reader.read_i32("Summary.LegacyUE3Version")?)
        } else {
            None
        };
        let ue4 = reader.read_i32("Summary.FileVersionUE4")?;
        let ue5 = if legacy_file_version <= -8 {
            reader.read_i32("Summary.FileVersionUE5")?
        } else {
            0
        };
        let licensee = reader.read_i32("Summary.FileVersionLicenseeUE")?;

        let mut versions = VersionContext {
            legacy_file_version,
            legacy_ue3,
            ue4,
            ue5,
            licensee,
            package_flags: PackageFlags::default(),
        };
        validate_package_versions(&versions, reader.tell())?;

        let (saved_hash, total_header_size) = if versions.is_at_least_ue5(UE5_PACKAGE_SAVED_HASH) {
            (
                reader.read_io_hash("Summary.SavedHash")?,
                read_non_negative_u32(&mut reader, "Summary.TotalHeaderSize")?,
            )
        } else {
            (IoHash::default(), 0)
        };

        let custom_versions = read_custom_versions(&mut reader, legacy_file_version)?;
        let total_header_size = if versions.is_at_least_ue5(UE5_PACKAGE_SAVED_HASH) {
            total_header_size
        } else {
            read_non_negative_u32(&mut reader, "Summary.TotalHeaderSize")?
        };
        let package_name = reader.read_fstring("Summary.PackageName")?;
        let package_flags = PackageFlags::from_bits(reader.read_u32("Summary.PackageFlags")?);
        versions.package_flags = package_flags;

        reject_unsupported_flags(package_flags, reader.tell() - 4)?;

        let names = read_table(&mut reader, "Summary.Names")?;
        let soft_object_paths = versions
            .is_at_least_ue5(UE5_ADD_SOFT_OBJECT_PATH_LIST)
            .then(|| read_table(&mut reader, "Summary.SoftObjectPaths"))
            .transpose()?;
        let filter_editor_only = package_flags.contains(PackageFlags::FILTER_EDITOR_ONLY);
        let localization_id = (!filter_editor_only
            && versions.is_at_least_ue4(UE4_ADDED_PACKAGE_SUMMARY_LOCALIZATION_ID))
        .then(|| reader.read_fstring("Summary.LocalizationId"))
        .transpose()?;
        let gatherable_text_data = versions
            .is_at_least_ue4(UE4_SERIALIZE_TEXT_IN_PACKAGES)
            .then(|| read_table(&mut reader, "Summary.GatherableTextData"))
            .transpose()?;
        let exports = read_table(&mut reader, "Summary.Exports")?;
        let imports = read_table(&mut reader, "Summary.Imports")?;
        let cell_exports = versions
            .is_at_least_ue5(UE5_VERSE_CELLS)
            .then(|| read_table(&mut reader, "Summary.CellExports"))
            .transpose()?;
        let cell_imports = versions
            .is_at_least_ue5(UE5_VERSE_CELLS)
            .then(|| read_table(&mut reader, "Summary.CellImports"))
            .transpose()?;
        let metadata_offset = versions
            .is_at_least_ue5(UE5_METADATA_SERIALIZATION_OFFSET)
            .then(|| read_offset(&mut reader, "Summary.MetaDataOffset"))
            .transpose()?;
        let depends_offset = read_offset(&mut reader, "Summary.DependsOffset")?;
        let soft_package_references = versions
            .is_at_least_ue4(UE4_ADD_STRING_ASSET_REFERENCES_MAP)
            .then(|| read_table(&mut reader, "Summary.SoftPackageReferences"))
            .transpose()?;
        let searchable_names_offset = versions
            .is_at_least_ue4(UE4_ADDED_SEARCHABLE_NAMES)
            .then(|| read_offset(&mut reader, "Summary.SearchableNamesOffset"))
            .transpose()?;
        let thumbnail_table_offset = read_offset(&mut reader, "Summary.ThumbnailTableOffset")?;
        let import_type_hierarchies = versions
            .is_at_least_ue5(UE5_IMPORT_TYPE_HIERARCHIES)
            .then(|| read_table(&mut reader, "Summary.ImportTypeHierarchies"))
            .transpose()?;

        let legacy_guid = (!versions.is_at_least_ue5(UE5_PACKAGE_SAVED_HASH))
            .then(|| reader.read_guid("Summary.LegacyGuid"))
            .transpose()?;
        let saved_hash = legacy_guid.map_or(saved_hash, io_hash_from_guid);
        let persistent_guid = (!filter_editor_only
            && versions.is_at_least_ue4(UE4_ADDED_PACKAGE_OWNER))
        .then(|| reader.read_guid("Summary.PersistentGuid"))
        .transpose()?;
        if !filter_editor_only
            && versions.is_at_least_ue4(UE4_ADDED_PACKAGE_OWNER)
            && !versions.is_at_least_ue4(UE4_NON_OUTER_PACKAGE_IMPORT)
        {
            reader.read_guid("Summary.OwnerPersistentGuid")?;
        }

        let raw_generations = reader.read_tarray("Summary.Generations", 8, |reader, index| {
            Ok((
                reader.read_i32(&format!("Summary.Generations[{index}].ExportCount"))?,
                reader.read_i32(&format!("Summary.Generations[{index}].NameCount"))?,
            ))
        })?;
        let generations = raw_generations
            .into_iter()
            .enumerate()
            .map(|(index, (export_count, name_count))| {
                Ok(GenerationInfo {
                    export_count: checked_non_negative_i32(
                        export_count,
                        &format!("Summary.Generations[{index}].ExportCount"),
                    )?,
                    name_count: checked_non_negative_i32(
                        name_count,
                        &format!("Summary.Generations[{index}].NameCount"),
                    )?,
                })
            })
            .collect::<Result<Vec<_>, PackageError>>()?;

        let saved_by_engine_version = if versions.is_at_least_ue4(UE4_ENGINE_VERSION_OBJECT) {
            read_engine_version(&mut reader, "Summary.SavedByEngineVersion")?
        } else {
            let changelist = reader.read_u32("Summary.EngineChangelist")?;
            EngineVersion {
                major: 4,
                minor: 0,
                patch: 0,
                changelist,
                branch: String::new(),
            }
        };
        let compatible_with_engine_version =
            if versions.is_at_least_ue4(UE4_PACKAGE_SUMMARY_HAS_COMPATIBLE_ENGINE_VERSION) {
                read_engine_version(&mut reader, "Summary.CompatibleWithEngineVersion")?
            } else {
                saved_by_engine_version.clone()
            };

        let compression_flags = reader.read_u32("Summary.CompressionFlags")?;
        let compressed_chunks =
            reader.read_tarray("Summary.CompressedChunks", 16, |reader, index| {
                reader.skip(16, &format!("Summary.CompressedChunks[{index}]"))?;
                Ok(())
            })?;
        if !compressed_chunks.is_empty() {
            return Err(PackageError::new(
                PackageErrorKind::UnsupportedCapability,
                Some(reader.tell()),
                "Summary.CompressedChunks",
                "package-level compression is not supported",
            ));
        }

        let package_source = reader.read_u32("Summary.PackageSource")?;
        let _: Vec<String> =
            reader.read_tarray("Summary.AdditionalPackagesToCook", 4, |reader, index| {
                reader.read_fstring(&format!("Summary.AdditionalPackagesToCook[{index}]"))
            })?;
        if legacy_file_version > -7 {
            let allocation_count =
                read_non_negative_u32(&mut reader, "Summary.NumTextureAllocations")?;
            if allocation_count != 0 {
                return Err(PackageError::new(
                    PackageErrorKind::UnsupportedCapability,
                    Some(reader.tell() - 4),
                    "Summary.NumTextureAllocations",
                    "legacy texture allocation data is not supported",
                ));
            }
        }

        let asset_registry_data_offset =
            read_offset(&mut reader, "Summary.AssetRegistryDataOffset")?;
        let bulk_data_start_offset =
            read_non_negative_u64(&mut reader, "Summary.BulkDataStartOffset")?;
        let world_tile_info_data_offset = versions
            .is_at_least_ue4(UE4_WORLD_LEVEL_INFO)
            .then(|| read_offset(&mut reader, "Summary.WorldTileInfoDataOffset"))
            .transpose()?;
        let chunk_ids = if versions.is_at_least_ue4(UE4_CHANGED_CHUNK_ID_TO_ARRAY) {
            reader.read_tarray("Summary.ChunkIDs", 4, |reader, index| {
                reader.read_i32(&format!("Summary.ChunkIDs[{index}]"))
            })?
        } else {
            Vec::new()
        };
        let preload_dependencies = versions
            .is_at_least_ue4(UE4_PRELOAD_DEPENDENCIES_IN_COOKED_EXPORTS)
            .then(|| read_optional_table(&mut reader, "Summary.PreloadDependencies"))
            .transpose()?;
        let preload_dependencies = preload_dependencies.flatten();
        let names_referenced_from_export_data_count =
            if versions.is_at_least_ue5(UE5_NAMES_REFERENCED_FROM_EXPORT_DATA) {
                read_non_negative_u32(&mut reader, "Summary.NamesReferencedFromExportDataCount")?
            } else {
                names.count
            };
        let payload_toc_offset = if versions.is_at_least_ue5(UE5_PAYLOAD_TOC) {
            read_optional_offset(&mut reader, "Summary.PayloadTocOffset")?
        } else {
            None
        };
        let data_resource_offset = if versions.is_at_least_ue5(UE5_DATA_RESOURCES) {
            read_optional_i32_offset(&mut reader, "Summary.DataResourceOffset")?
        } else {
            None
        };

        let span = Span::new(start, reader.tell() - start)?;
        let summary = Self {
            span,
            versions,
            custom_versions,
            saved_hash,
            total_header_size,
            package_name,
            names,
            soft_object_paths,
            localization_id,
            gatherable_text_data,
            exports,
            imports,
            cell_exports,
            cell_imports,
            metadata_offset,
            depends_offset,
            soft_package_references,
            searchable_names_offset,
            thumbnail_table_offset,
            import_type_hierarchies,
            persistent_guid,
            generations,
            saved_by_engine_version,
            compatible_with_engine_version,
            compression_flags,
            package_source,
            asset_registry_data_offset,
            bulk_data_start_offset,
            world_tile_info_data_offset,
            chunk_ids,
            preload_dependencies,
            names_referenced_from_export_data_count,
            payload_toc_offset,
            data_resource_offset,
        };
        summary.validate(source.len())?;
        Ok(summary)
    }

    fn validate(&self, file_len: usize) -> Result<(), PackageError> {
        let file_len = u64::try_from(file_len).expect("usize always fits in u64");
        if self.total_header_size == 0 || u64::from(self.total_header_size) > file_len {
            return Err(PackageError::new(
                PackageErrorKind::MalformedData,
                Some(0),
                "Summary.TotalHeaderSize",
                format!(
                    "header size {} is outside file length {file_len}",
                    self.total_header_size
                ),
            ));
        }
        for (path, table) in [
            ("Summary.Names", Some(self.names)),
            ("Summary.Exports", Some(self.exports)),
            ("Summary.Imports", Some(self.imports)),
            ("Summary.SoftObjectPaths", self.soft_object_paths),
            ("Summary.GatherableTextData", self.gatherable_text_data),
            ("Summary.CellExports", self.cell_exports),
            ("Summary.CellImports", self.cell_imports),
            (
                "Summary.SoftPackageReferences",
                self.soft_package_references,
            ),
            (
                "Summary.ImportTypeHierarchies",
                self.import_type_hierarchies,
            ),
            ("Summary.PreloadDependencies", self.preload_dependencies),
        ] {
            if let Some(table) = table {
                validate_table_location(path, table, file_len)?;
            }
        }
        Ok(())
    }
}

fn validate_legacy_version(version: i32, offset: u64) -> Result<(), PackageError> {
    if version >= 0 {
        return Err(PackageError::new(
            PackageErrorKind::UnsupportedFormat,
            Some(offset),
            "Summary.LegacyFileVersion",
            "UE3-style package versions are not supported",
        ));
    }
    if version < VersionContext::CURRENT_LEGACY_FILE_VERSION {
        return Err(PackageError::new(
            PackageErrorKind::UnsupportedVersion,
            Some(offset),
            "Summary.LegacyFileVersion",
            format!("future legacy package version {version} is not safely parseable"),
        ));
    }
    if version > VersionContext::OLDEST_SUPPORTED_LEGACY_FILE_VERSION {
        return Err(PackageError::new(
            PackageErrorKind::UnsupportedVersion,
            Some(offset),
            "Summary.LegacyFileVersion",
            format!("legacy package version {version} predates custom-version support"),
        ));
    }
    Ok(())
}

fn validate_package_versions(versions: &VersionContext, offset: u64) -> Result<(), PackageError> {
    if versions.is_unversioned() {
        return Err(PackageError::new(
            PackageErrorKind::UnsupportedCapability,
            Some(offset),
            "Summary.Version",
            "unversioned package summaries cannot be interpreted safely",
        ));
    }
    if versions.ue4 < VersionContext::OLDEST_LOADABLE_UE4 {
        return Err(PackageError::new(
            PackageErrorKind::UnsupportedVersion,
            Some(offset),
            "Summary.FileVersionUE4",
            format!("UE4 version {} is too old", versions.ue4),
        ));
    }
    if versions.ue4 > VersionContext::LATEST_SUPPORTED_UE4
        || versions.ue5 > VersionContext::LATEST_SUPPORTED_UE5
    {
        return Err(PackageError::new(
            PackageErrorKind::UnsupportedVersion,
            Some(offset),
            "Summary.Version",
            format!(
                "package version UE4={} UE5={} exceeds supported UE4={} UE5={}",
                versions.ue4,
                versions.ue5,
                VersionContext::LATEST_SUPPORTED_UE4,
                VersionContext::LATEST_SUPPORTED_UE5
            ),
        ));
    }
    Ok(())
}

fn reject_unsupported_flags(flags: PackageFlags, offset: u64) -> Result<(), PackageError> {
    if flags.contains(PackageFlags::COOKED) {
        return Err(PackageError::new(
            PackageErrorKind::UnsupportedCapability,
            Some(offset),
            "Summary.PackageFlags",
            "cooked packages are outside the current parser contract",
        ));
    }
    if flags.contains(PackageFlags::UNVERSIONED_PROPERTIES) {
        return Err(PackageError::new(
            PackageErrorKind::UnsupportedCapability,
            Some(offset),
            "Summary.PackageFlags",
            "unversioned property serialization is outside the current parser contract",
        ));
    }
    Ok(())
}

fn read_custom_versions(
    reader: &mut Reader<'_>,
    legacy_file_version: i32,
) -> Result<Vec<CustomVersion>, PackageError> {
    let mut seen = BTreeMap::new();
    let versions = reader.read_tarray("Summary.CustomVersions", 8, |reader, index| {
        let path = format!("Summary.CustomVersions[{index}]");
        let version = match legacy_file_version {
            -2 => CustomVersion {
                key: Guid {
                    a: 0,
                    b: 0,
                    c: 0,
                    d: reader.read_u32(&format!("{path}.Tag"))?,
                },
                version: reader.read_i32(&format!("{path}.Version"))?,
                friendly_name: None,
            },
            -5..=-3 => CustomVersion {
                key: reader.read_guid(&format!("{path}.Key"))?,
                version: reader.read_i32(&format!("{path}.Version"))?,
                friendly_name: Some(reader.read_fstring(&format!("{path}.FriendlyName"))?),
            },
            -9..=-6 => CustomVersion {
                key: reader.read_guid(&format!("{path}.Key"))?,
                version: reader.read_i32(&format!("{path}.Version"))?,
                friendly_name: None,
            },
            _ => unreachable!("legacy version was validated"),
        };
        Ok(version)
    })?;
    for version in &versions {
        if seen.insert(version.key, version.version).is_some() {
            return Err(PackageError::new(
                PackageErrorKind::MalformedData,
                Some(reader.tell()),
                "Summary.CustomVersions",
                "duplicate custom-version GUID",
            ));
        }
    }
    Ok(versions)
}

fn read_table(reader: &mut Reader<'_>, path: &str) -> Result<TableLocation, PackageError> {
    Ok(TableLocation {
        count: read_non_negative_u32(reader, &format!("{path}.Count"))?,
        offset: read_offset(reader, &format!("{path}.Offset"))?,
    })
}

fn read_optional_table(
    reader: &mut Reader<'_>,
    path: &str,
) -> Result<Option<TableLocation>, PackageError> {
    let count_offset = reader.tell();
    let count = reader.read_i32(&format!("{path}.Count"))?;
    let offset = read_offset(reader, &format!("{path}.Offset"))?;
    match count {
        -1 => Ok(None),
        0.. => Ok(Some(TableLocation {
            count: u32::try_from(count).expect("count was checked as non-negative"),
            offset,
        })),
        _ => Err(PackageError::new(
            PackageErrorKind::MalformedData,
            Some(count_offset),
            format!("{path}.Count"),
            format!("optional table count must be -1 or non-negative, got {count}"),
        )),
    }
}

fn read_name_map(
    reader: &mut Reader<'_>,
    summary: &PackageSummary,
) -> Result<Vec<String>, PackageError> {
    reader.seek(summary.names.offset.get(), "Names")?;
    let mut names = Vec::with_capacity(usize::try_from(summary.names.count).map_err(|_| {
        PackageError::new(
            PackageErrorKind::MalformedData,
            Some(summary.names.offset.get()),
            "Names.Count",
            "name count does not fit in usize",
        )
    })?);

    for index in 0..summary.names.count {
        let path = format!("Names[{index}]");
        names.push(reader.read_fstring(&path)?);
        if summary.versions.is_at_least_ue4(UE4_NAME_HASHES_SERIALIZED) {
            reader.skip(4, &format!("{path}.Hashes"))?;
        }
    }
    Ok(names)
}

fn read_import_map(
    reader: &mut Reader<'_>,
    summary: &PackageSummary,
    names: &[String],
) -> Result<Vec<Import>, PackageError> {
    reader.seek(summary.imports.offset.get(), "Imports")?;
    let mut imports = Vec::with_capacity(usize::try_from(summary.imports.count).map_err(|_| {
        PackageError::new(
            PackageErrorKind::MalformedData,
            Some(summary.imports.offset.get()),
            "Imports.Count",
            "import count does not fit in usize",
        )
    })?);

    for index in 0..summary.imports.count {
        let path = format!("Imports[{index}]");
        let class_package = reader.read_name_ref(&format!("{path}.ClassPackage"))?;
        validate_name_ref(names, class_package, &format!("{path}.ClassPackage"))?;
        let class_name = reader.read_name_ref(&format!("{path}.ClassName"))?;
        validate_name_ref(names, class_name, &format!("{path}.ClassName"))?;
        let outer_index = PackageIndex::parse(reader.read_i32(&format!("{path}.OuterIndex"))?);
        let object_name = reader.read_name_ref(&format!("{path}.ObjectName"))?;
        validate_name_ref(names, object_name, &format!("{path}.ObjectName"))?;
        let package_name = (!summary
            .versions
            .package_flags
            .contains(PackageFlags::FILTER_EDITOR_ONLY)
            && summary
                .versions
                .is_at_least_ue4(UE4_NON_OUTER_PACKAGE_IMPORT))
        .then(|| {
            let name = reader.read_name_ref(&format!("{path}.PackageName"))?;
            validate_name_ref(names, name, &format!("{path}.PackageName"))?;
            Ok::<crate::archive::NameRef, PackageError>(name)
        })
        .transpose()?;
        let import_optional = summary
            .versions
            .is_at_least_ue5(UE5_OPTIONAL_RESOURCES)
            .then(|| read_archive_bool(reader, &format!("{path}.bImportOptional")))
            .transpose()?;

        imports.push(Import {
            class_package,
            class_name,
            outer_index,
            object_name,
            package_name,
            import_optional,
            object_path: ObjectPath(String::new()),
            class_path: ObjectPath(String::new()),
        });
    }
    Ok(imports)
}

fn read_export_map(
    reader: &mut Reader<'_>,
    summary: &PackageSummary,
    names: &[String],
) -> Result<Vec<Export>, PackageError> {
    reader.seek(summary.exports.offset.get(), "Exports")?;
    let mut exports = Vec::with_capacity(usize::try_from(summary.exports.count).map_err(|_| {
        PackageError::new(
            PackageErrorKind::MalformedData,
            Some(summary.exports.offset.get()),
            "Exports.Count",
            "export count does not fit in usize",
        )
    })?);

    for index in 0..summary.exports.count {
        let path = format!("Exports[{index}]");
        let class_index = PackageIndex::parse(reader.read_i32(&format!("{path}.ClassIndex"))?);
        let super_index = PackageIndex::parse(reader.read_i32(&format!("{path}.SuperIndex"))?);
        let template_index = summary
            .versions
            .is_at_least_ue4(UE4_TEMPLATE_INDEX_IN_COOKED_EXPORTS)
            .then(|| {
                Ok::<PackageIndex, PackageError>(PackageIndex::parse(
                    reader.read_i32(&format!("{path}.TemplateIndex"))?,
                ))
            })
            .transpose()?;
        let outer_index = PackageIndex::parse(reader.read_i32(&format!("{path}.OuterIndex"))?);
        let object_name = reader.read_name_ref(&format!("{path}.ObjectName"))?;
        validate_name_ref(names, object_name, &format!("{path}.ObjectName"))?;
        let object_flags = reader.read_u32(&format!("{path}.ObjectFlags"))?;
        let (serial_size, serial_offset) = read_export_serial_location(reader, summary, &path)?;
        let forced_export = read_archive_bool(reader, &format!("{path}.bForcedExport"))?;
        let not_for_client = read_archive_bool(reader, &format!("{path}.bNotForClient"))?;
        let not_for_server = read_archive_bool(reader, &format!("{path}.bNotForServer"))?;
        if !summary
            .versions
            .is_at_least_ue5(UE5_REMOVE_OBJECT_EXPORT_PACKAGE_GUID)
        {
            reader.read_guid(&format!("{path}.PackageGuid"))?;
        }
        let inherited_instance = summary
            .versions
            .is_at_least_ue5(UE5_TRACK_OBJECT_EXPORT_IS_INHERITED)
            .then(|| read_archive_bool(reader, &format!("{path}.bIsInheritedInstance")))
            .transpose()?;
        let package_flags = reader.read_u32(&format!("{path}.PackageFlags"))?;
        let not_always_loaded_for_editor_game = summary
            .versions
            .is_at_least_ue4(UE4_LOAD_FOR_EDITOR_GAME)
            .then(|| read_archive_bool(reader, &format!("{path}.bNotAlwaysLoadedForEditorGame")))
            .transpose()?;
        let is_asset = summary
            .versions
            .is_at_least_ue4(UE4_COOKED_ASSETS_IN_EDITOR_SUPPORT)
            .then(|| read_archive_bool(reader, &format!("{path}.bIsAsset")))
            .transpose()?;
        let generate_public_hash = summary
            .versions
            .is_at_least_ue5(UE5_OPTIONAL_RESOURCES)
            .then(|| read_archive_bool(reader, &format!("{path}.bGeneratePublicHash")))
            .transpose()?;
        if summary
            .versions
            .is_at_least_ue4(UE4_PRELOAD_DEPENDENCIES_IN_COOKED_EXPORTS)
        {
            reader.skip(20, &format!("{path}.PreloadDependencies"))?;
        }
        let (script_serialization_start_offset, script_serialization_end_offset) = if summary
            .versions
            .is_at_least_ue5(UE5_SCRIPT_SERIALIZATION_OFFSET)
        {
            (
                Some(read_non_negative_u64(
                    reader,
                    &format!("{path}.ScriptSerializationStartOffset"),
                )?),
                Some(read_non_negative_u64(
                    reader,
                    &format!("{path}.ScriptSerializationEndOffset"),
                )?),
            )
        } else {
            (None, None)
        };

        validate_export_span(serial_offset, serial_size, reader.span().len(), &path)?;

        exports.push(Export {
            class_index,
            super_index,
            template_index,
            outer_index,
            object_name,
            object_flags,
            serial_size,
            serial_offset,
            forced_export,
            not_for_client,
            not_for_server,
            inherited_instance,
            package_flags,
            not_always_loaded_for_editor_game,
            is_asset,
            generate_public_hash,
            script_serialization_start_offset,
            script_serialization_end_offset,
            object_path: ObjectPath(String::new()),
            class_path: None,
        });
    }
    Ok(exports)
}

fn read_export_serial_location(
    reader: &mut Reader<'_>,
    summary: &PackageSummary,
    path: &str,
) -> Result<(u64, FileOffset), PackageError> {
    if summary
        .versions
        .is_at_least_ue4(UE4_64BIT_EXPORTMAP_SERIALSIZES)
    {
        let serial_size = read_non_negative_u64(reader, &format!("{path}.SerialSize"))?;
        let serial_offset = read_non_negative_u64(reader, &format!("{path}.SerialOffset"))?;
        Ok((serial_size, FileOffset(serial_offset)))
    } else {
        let serial_size = read_non_negative_u32(reader, &format!("{path}.SerialSize"))?;
        let serial_offset = read_non_negative_u32(reader, &format!("{path}.SerialOffset"))?;
        Ok((u64::from(serial_size), FileOffset(u64::from(serial_offset))))
    }
}

fn read_archive_bool(reader: &mut Reader<'_>, path: &str) -> Result<bool, PackageError> {
    let offset = reader.tell();
    match reader.read_u32(path)? {
        0 => Ok(false),
        1 => Ok(true),
        value => Err(PackageError::new(
            PackageErrorKind::MalformedData,
            Some(offset),
            path,
            format!("serialized bool must be 0 or 1, got {value}"),
        )),
    }
}

fn validate_name_ref(
    names: &[String],
    name: crate::archive::NameRef,
    path: &str,
) -> Result<(), PackageError> {
    if usize::try_from(name.index().get())
        .ok()
        .is_some_and(|index| index < names.len())
    {
        Ok(())
    } else {
        Err(PackageError::new(
            PackageErrorKind::MalformedData,
            None,
            path,
            format!("name index {} is outside name map", name.index().get()),
        ))
    }
}

fn resolve_name_ref(
    names: &[String],
    name: crate::archive::NameRef,
) -> Result<String, PackageError> {
    validate_name_ref(names, name, "NameRef")?;
    let base = &names[usize::try_from(name.index().get()).expect("u32 fits in usize")];
    if name.number() == 0 {
        Ok(base.clone())
    } else {
        Ok(format!("{}_{}", base, name.number() - 1))
    }
}

fn resolve_import_class_path(
    import: &Import,
    names: &[String],
) -> Result<ObjectPath, PackageError> {
    let package = resolve_name_ref(names, import.class_package)?;
    let class = resolve_name_ref(names, import.class_name)?;
    Ok(normalize_object_path(ObjectPath(format!(
        "/{package}.{class}"
    ))))
}

fn normalize_object_path(path: ObjectPath) -> ObjectPath {
    if let Some(stripped) = path.0.strip_prefix("/None.") {
        ObjectPath(stripped.to_owned())
    } else if let Some(stripped) = path.0.strip_prefix("None.") {
        ObjectPath(stripped.to_owned())
    } else {
        path
    }
}

fn resolve_index_path(
    index: PackageIndex,
    package_name: &str,
    names: &[String],
    imports: &[Import],
    exports: &[Export],
    path: &str,
) -> Result<ObjectPath, PackageError> {
    match index {
        PackageIndex::Null => Ok(ObjectPath(package_name.to_owned())),
        PackageIndex::Import(import_index) => {
            let import = imports
                .get(usize::try_from(import_index).expect("u32 fits in usize"))
                .ok_or_else(|| {
                    PackageError::new(
                        PackageErrorKind::MalformedData,
                        None,
                        path,
                        format!("import index {import_index} is outside import map"),
                    )
                })?;
            let object_name = resolve_name_ref(names, import.object_name)?;
            match import.outer_index {
                PackageIndex::Null => {
                    if object_name == "None" {
                        return Ok(ObjectPath(String::new()));
                    }
                    if let Some(package_name_ref) = import.package_name {
                        let package = resolve_name_ref(names, package_name_ref)?;
                        Ok(ObjectPath(format!("/{package}.{object_name}")))
                    } else if object_name.starts_with('/') {
                        Ok(ObjectPath(object_name))
                    } else {
                        Ok(ObjectPath(format!("/{object_name}")))
                    }
                }
                outer => {
                    let outer =
                        resolve_index_path(outer, package_name, names, imports, exports, path)?;
                    if outer.as_str().is_empty() || outer.as_str() == "/None" {
                        Ok(ObjectPath(object_name))
                    } else {
                        Ok(ObjectPath(format!("{outer}.{object_name}")))
                    }
                }
            }
        }
        PackageIndex::Export(export_index) => {
            let export = exports
                .get(usize::try_from(export_index).expect("u32 fits in usize"))
                .ok_or_else(|| {
                    PackageError::new(
                        PackageErrorKind::MalformedData,
                        None,
                        path,
                        format!("export index {export_index} is outside export map"),
                    )
                })?;
            let object_name = resolve_name_ref(names, export.object_name)?;
            match export.outer_index {
                PackageIndex::Null => Ok(ObjectPath(format!("{package_name}.{object_name}"))),
                outer => {
                    let outer =
                        resolve_index_path(outer, package_name, names, imports, exports, path)?;
                    Ok(ObjectPath(format!("{outer}.{object_name}")))
                }
            }
        }
    }
}

fn validate_export_span(
    serial_offset: FileOffset,
    serial_size: u64,
    file_len: u64,
    path: &str,
) -> Result<(), PackageError> {
    let end = serial_offset
        .get()
        .checked_add(serial_size)
        .ok_or_else(|| {
            PackageError::new(
                PackageErrorKind::MalformedData,
                Some(serial_offset.get()),
                format!("{path}.SerialOffset"),
                format!(
                    "serial size {serial_size} overflows offset {}",
                    serial_offset.get()
                ),
            )
        })?;
    if end > file_len {
        return Err(PackageError::new(
            PackageErrorKind::MalformedData,
            Some(serial_offset.get()),
            format!("{path}.SerialOffset"),
            format!("export span ends at {end}, outside file length {file_len}"),
        ));
    }
    Ok(())
}

fn read_offset(reader: &mut Reader<'_>, path: &str) -> Result<FileOffset, PackageError> {
    let offset = reader.tell();
    let value = reader.read_i32(path)?;
    if value < 0 {
        return Err(PackageError::new(
            PackageErrorKind::MalformedData,
            Some(offset),
            path,
            format!("offset must be non-negative, got {value}"),
        ));
    }
    Ok(FileOffset(u64::from(
        u32::try_from(value).expect("value was checked as non-negative"),
    )))
}

fn read_optional_i32_offset(
    reader: &mut Reader<'_>,
    path: &str,
) -> Result<Option<FileOffset>, PackageError> {
    let offset = reader.tell();
    let value = reader.read_i32(path)?;
    match value {
        -1 => Ok(None),
        0.. => Ok(Some(FileOffset(u64::from(
            u32::try_from(value).expect("value was checked as non-negative"),
        )))),
        _ => Err(PackageError::new(
            PackageErrorKind::MalformedData,
            Some(offset),
            path,
            format!("optional offset must be -1 or non-negative, got {value}"),
        )),
    }
}

fn read_optional_offset(
    reader: &mut Reader<'_>,
    path: &str,
) -> Result<Option<FileOffset>, PackageError> {
    let offset = reader.tell();
    let value = reader.read_i64(path)?;
    match value {
        -1 => Ok(None),
        0.. => Ok(Some(FileOffset(
            u64::try_from(value).expect("value was checked as non-negative"),
        ))),
        _ => Err(PackageError::new(
            PackageErrorKind::MalformedData,
            Some(offset),
            path,
            format!("optional offset must be -1 or non-negative, got {value}"),
        )),
    }
}

fn read_non_negative_u32(reader: &mut Reader<'_>, path: &str) -> Result<u32, PackageError> {
    let offset = reader.tell();
    let value = reader.read_i32(path)?;
    u32::try_from(value).map_err(|_| {
        PackageError::new(
            PackageErrorKind::MalformedData,
            Some(offset),
            path,
            format!("value must be non-negative, got {value}"),
        )
    })
}

fn checked_non_negative_i32(value: i32, path: &str) -> Result<u32, PackageError> {
    u32::try_from(value).map_err(|_| {
        PackageError::new(
            PackageErrorKind::MalformedData,
            None,
            path,
            format!("value must be non-negative, got {value}"),
        )
    })
}

fn read_non_negative_u64(reader: &mut Reader<'_>, path: &str) -> Result<u64, PackageError> {
    let offset = reader.tell();
    let value = reader.read_i64(path)?;
    u64::try_from(value).map_err(|_| {
        PackageError::new(
            PackageErrorKind::MalformedData,
            Some(offset),
            path,
            format!("value must be non-negative, got {value}"),
        )
    })
}

fn read_engine_version(reader: &mut Reader<'_>, path: &str) -> Result<EngineVersion, PackageError> {
    Ok(EngineVersion {
        major: reader.read_u16(&format!("{path}.Major"))?,
        minor: reader.read_u16(&format!("{path}.Minor"))?,
        patch: reader.read_u16(&format!("{path}.Patch"))?,
        changelist: reader.read_u32(&format!("{path}.Changelist"))?,
        branch: reader.read_fstring(&format!("{path}.Branch"))?,
    })
}

fn io_hash_from_guid(guid: Guid) -> IoHash {
    let mut bytes = [0; IoHash::BYTE_LEN];
    bytes[0..4].copy_from_slice(&guid.a.to_le_bytes());
    bytes[4..8].copy_from_slice(&guid.b.to_le_bytes());
    bytes[8..12].copy_from_slice(&guid.c.to_le_bytes());
    bytes[12..16].copy_from_slice(&guid.d.to_le_bytes());
    IoHash::from_bytes(bytes)
}

fn validate_table_location(
    path: &str,
    table: TableLocation,
    file_len: u64,
) -> Result<(), PackageError> {
    if table.count > 0 && table.offset.get() >= file_len {
        return Err(PackageError::new(
            PackageErrorKind::MalformedData,
            Some(table.offset.get()),
            path,
            format!(
                "non-empty table starts at {}, outside file length {file_len}",
                table.offset.get()
            ),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn push_i32(bytes: &mut Vec<u8>, value: i32) {
        bytes.extend_from_slice(&value.to_le_bytes());
    }

    fn push_u32(bytes: &mut Vec<u8>, value: u32) {
        bytes.extend_from_slice(&value.to_le_bytes());
    }

    fn push_i64(bytes: &mut Vec<u8>, value: i64) {
        bytes.extend_from_slice(&value.to_le_bytes());
    }

    fn push_engine_version(bytes: &mut Vec<u8>) {
        bytes.extend_from_slice(&5_u16.to_le_bytes());
        bytes.extend_from_slice(&7_u16.to_le_bytes());
        bytes.extend_from_slice(&2_u16.to_le_bytes());
        push_u32(bytes, 123_456);
        push_i32(bytes, 0);
    }

    fn current_summary_fixture(package_flags: u32) -> Vec<u8> {
        let mut bytes = Vec::new();
        push_u32(&mut bytes, PACKAGE_FILE_TAG);
        push_i32(&mut bytes, -9);
        push_i32(&mut bytes, 864);
        push_i32(&mut bytes, VersionContext::LATEST_SUPPORTED_UE4);
        push_i32(&mut bytes, VersionContext::LATEST_SUPPORTED_UE5);
        push_i32(&mut bytes, 0);
        bytes.extend_from_slice(&[0xAB; IoHash::BYTE_LEN]);
        let total_header_size_offset = bytes.len();
        push_i32(&mut bytes, 0);

        push_i32(&mut bytes, 0); // CustomVersions
        push_i32(&mut bytes, 0); // PackageName
        push_u32(&mut bytes, package_flags);

        for _ in 0..2 {
            push_i32(&mut bytes, 0); // Names
        }
        for _ in 0..2 {
            push_i32(&mut bytes, 0); // SoftObjectPaths
        }
        push_i32(&mut bytes, 0); // LocalizationId
        for _ in 0..2 {
            push_i32(&mut bytes, 0); // GatherableTextData
        }
        for _ in 0..2 {
            push_i32(&mut bytes, 0); // Exports
        }
        for _ in 0..2 {
            push_i32(&mut bytes, 0); // Imports
        }
        for _ in 0..2 {
            push_i32(&mut bytes, 0); // CellExports
        }
        for _ in 0..2 {
            push_i32(&mut bytes, 0); // CellImports
        }
        push_i32(&mut bytes, 0); // MetaDataOffset
        push_i32(&mut bytes, 0); // DependsOffset
        for _ in 0..2 {
            push_i32(&mut bytes, 0); // SoftPackageReferences
        }
        push_i32(&mut bytes, 0); // SearchableNamesOffset
        push_i32(&mut bytes, 0); // ThumbnailTableOffset
        for _ in 0..2 {
            push_i32(&mut bytes, 0); // ImportTypeHierarchies
        }
        bytes.extend_from_slice(&[0; 16]); // PersistentGuid
        push_i32(&mut bytes, 0); // Generations
        push_engine_version(&mut bytes);
        push_engine_version(&mut bytes);
        push_u32(&mut bytes, 0); // CompressionFlags
        push_i32(&mut bytes, 0); // CompressedChunks
        push_u32(&mut bytes, 0); // PackageSource
        push_i32(&mut bytes, 0); // AdditionalPackagesToCook
        push_i32(&mut bytes, 0); // AssetRegistryDataOffset
        push_i64(&mut bytes, 0); // BulkDataStartOffset
        push_i32(&mut bytes, 0); // WorldTileInfoDataOffset
        push_i32(&mut bytes, 0); // ChunkIDs
        push_i32(&mut bytes, -1); // PreloadDependencyCount
        push_i32(&mut bytes, 0); // PreloadDependencyOffset
        push_i32(&mut bytes, 0); // NamesReferencedFromExportDataCount
        push_i64(&mut bytes, -1); // PayloadTocOffset
        push_i32(&mut bytes, -1); // DataResourceOffset

        let header_size = i32::try_from(bytes.len()).unwrap();
        bytes[total_header_size_offset..total_header_size_offset + 4]
            .copy_from_slice(&header_size.to_le_bytes());
        bytes
    }

    #[test]
    fn parses_current_ue5_summary_contract() {
        let bytes = current_summary_fixture(0);

        let summary = PackageSummary::parse(&bytes).unwrap();

        assert_eq!(summary.versions.legacy_file_version, -9);
        assert_eq!(summary.versions.ue5, 1018);
        assert_eq!(summary.total_header_size as usize, bytes.len());
        assert_eq!(summary.span.len() as usize, bytes.len());
        assert!(summary.soft_object_paths.is_some());
        assert!(summary.cell_exports.is_some());
        assert!(summary.import_type_hierarchies.is_some());
        assert_eq!(summary.payload_toc_offset, None);
        assert_eq!(summary.data_resource_offset, None);
    }

    #[test]
    fn rejects_out_of_contract_package_flags() {
        for flag in [PackageFlags::COOKED, PackageFlags::UNVERSIONED_PROPERTIES] {
            let error = PackageSummary::parse(&current_summary_fixture(flag)).unwrap_err();
            assert_eq!(error.kind(), PackageErrorKind::UnsupportedCapability);
            assert_eq!(error.path(), "Summary.PackageFlags");
        }
    }

    #[test]
    fn parses_existing_classic_ue5_asset_summary() {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(
            "../Samples/StarterContent/Content/StarterContent/Architecture/Floor_400x400.uasset",
        );
        let bytes = std::fs::read(path).unwrap();

        let summary = PackageSummary::parse(&bytes).unwrap();

        assert_eq!(summary.versions.legacy_file_version, -8);
        assert_eq!(summary.versions.ue4, 522);
        assert_eq!(summary.versions.ue5, 1006);
        assert!(summary.total_header_size > summary.span.len() as u32);
        assert!(summary.names.count > 0);
        assert!(summary.exports.count > 0);
        assert!(summary.names.offset.get() < bytes.len() as u64);
    }

    #[test]
    fn rejects_invalid_and_swapped_tags_distinctly() {
        let invalid = PackageSummary::parse(&0_u32.to_le_bytes()).unwrap_err();
        assert_eq!(invalid.kind(), PackageErrorKind::UnsupportedFormat);

        let swapped = PackageSummary::parse(&PACKAGE_FILE_TAG_SWAPPED.to_le_bytes()).unwrap_err();
        assert_eq!(swapped.kind(), PackageErrorKind::UnsupportedCapability);
    }

    #[test]
    fn rejects_future_legacy_version_before_parsing_changed_layout() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&PACKAGE_FILE_TAG.to_le_bytes());
        bytes.extend_from_slice(&(-10_i32).to_le_bytes());

        let error = PackageSummary::parse(&bytes).unwrap_err();
        assert_eq!(error.kind(), PackageErrorKind::UnsupportedVersion);
        assert_eq!(error.path(), "Summary.LegacyFileVersion");
    }

    #[test]
    fn truncated_summary_keeps_logical_field_context() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&PACKAGE_FILE_TAG.to_le_bytes());
        bytes.extend_from_slice(&(-8_i32).to_le_bytes());
        bytes.extend_from_slice(&864_i32.to_le_bytes());

        let error = PackageSummary::parse(&bytes).unwrap_err();
        assert_eq!(error.kind(), PackageErrorKind::MalformedData);
        assert_eq!(error.path(), "Summary.FileVersionUE4");
        assert_eq!(error.offset(), Some(12));
    }
}
