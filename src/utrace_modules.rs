//! Bounded catalog for Unreal `Diagnostics.Module*` address ranges.

use std::collections::BTreeMap;

use crate::utrace::{
    EventTypeInfo, ModuleDashboard, ModuleEntry, ModuleFrameMap, ModuleFrameMapping,
    ModuleIdentity, SymbolFormat, TraceError, TraceErrorKind, event_data_size, optional_aux_text,
    parse_protocol5_aux, read_required_aux_bytes, read_u8_field, read_u32_field, read_u64_field,
};

const MAX_RETAINED_MODULES: usize = 4_096;
const MAX_IMAGE_ID_BYTES: usize = 64;

#[derive(Clone, Debug, Default)]
pub(crate) struct ModuleProvider {
    init_seen: bool,
    symbol_format: Option<SymbolFormat>,
    module_base_shift: u8,
    /// Default Insights uses when ModuleInit is absent from GetValue fallback.
    missing_init: bool,
    observed_loads: u64,
    observed_unloads: u64,
    retained: u64,
    dropped: u64,
    truncated: bool,
    duplicate_bases: u64,
    unload_without_load: u64,
    malformed: u64,
    /// Active modules keyed by reconstructed base address.
    active: BTreeMap<u64, LoadedModule>,
    /// Bounded retained history for dashboard (latest wins per base).
    retained_modules: BTreeMap<u64, LoadedModule>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct LoadedModule {
    name: String,
    base: u64,
    size: u32,
    image_id: Vec<u8>,
    unloaded: bool,
}

impl ModuleProvider {
    pub(crate) fn record_init(&mut self, format: SymbolFormat, module_base_shift: u8) {
        self.init_seen = true;
        self.symbol_format = Some(format);
        self.module_base_shift = module_base_shift;
    }

    pub(crate) fn record_load(
        &mut self,
        name: String,
        raw_base: u64,
        size: u32,
        image_id: Vec<u8>,
    ) -> Result<(), TraceError> {
        self.observed_loads = self.observed_loads.saturating_add(1);
        if !self.init_seen {
            self.missing_init = true;
            // Insights defaults GetValue("ModuleBaseShift", 16) only when reading the
            // field; without ModuleInit the provider is not created. We still map
            // ranges using shift 0 until init arrives, and flag missing_init.
        }

        let base = reconstruct_base(raw_base, self.module_base_shift)?;
        if size == 0 {
            self.malformed = self.malformed.saturating_add(1);
            return Ok(());
        }
        let end = base.checked_add(u64::from(size)).ok_or_else(|| {
            TraceError::new(
                TraceErrorKind::MalformedData,
                0,
                "Diagnostics.ModuleLoad",
                format!("module base {base:#x} + size {size} overflows u64"),
            )
        })?;
        let _ = end;

        let module = LoadedModule {
            name,
            base,
            size,
            image_id,
            unloaded: false,
        };

        let can_retain = self.retained_modules.contains_key(&base)
            || self.retained_modules.len() < MAX_RETAINED_MODULES;
        if !can_retain {
            self.dropped = self.dropped.saturating_add(1);
            self.truncated = true;
            return Ok(());
        }

        if self.active.insert(base, module.clone()).is_some() {
            self.duplicate_bases = self.duplicate_bases.saturating_add(1);
        }

        self.retained_modules.insert(base, module);
        self.retained = u64::try_from(self.retained_modules.len()).unwrap_or(u64::MAX);
        Ok(())
    }

    pub(crate) fn record_unload(&mut self, raw_base: u64) -> Result<(), TraceError> {
        self.observed_unloads = self.observed_unloads.saturating_add(1);
        let base = reconstruct_base(raw_base, self.module_base_shift)?;
        if let Some(module) = self.active.remove(&base) {
            if let Some(retained) = self.retained_modules.get_mut(&base) {
                retained.unloaded = true;
            } else {
                let mut unloaded = module;
                unloaded.unloaded = true;
                if self.retained_modules.len() < MAX_RETAINED_MODULES {
                    self.retained_modules.insert(base, unloaded);
                }
            }
        } else {
            self.unload_without_load = self.unload_without_load.saturating_add(1);
        }
        Ok(())
    }

    pub(crate) fn map_address(&self, address: u64) -> ModuleFrameMapping {
        map_matching_modules(
            self.active.values().filter(|module| !module.unloaded),
            address,
        )
        .or_else(|| {
            // Retained unloaded modules provide a best-effort mapping for historical PCs.
            map_matching_modules(self.retained_modules.values(), address)
        })
        .unwrap_or(ModuleFrameMapping::Unmapped)
    }

    pub(crate) fn dashboard(self) -> ModuleDashboard {
        let mut modules = self
            .retained_modules
            .into_values()
            .map(|module| ModuleEntry {
                name: module.name,
                base: format_address(module.base),
                size: module.size,
                image_id_hex: hex_encode(&module.image_id),
                identity: ModuleIdentity::from_image_id(&module.image_id),
                unloaded: module.unloaded,
            })
            .collect::<Vec<_>>();
        modules.sort_by(|left, right| left.name.cmp(&right.name).then(left.base.cmp(&right.base)));

        ModuleDashboard {
            init_seen: self.init_seen,
            missing_init: self.missing_init,
            symbol_format: self.symbol_format,
            module_base_shift: self.module_base_shift,
            observed_loads: self.observed_loads,
            observed_unloads: self.observed_unloads,
            retained: self.retained,
            dropped: self.dropped,
            truncated: self.truncated,
            duplicate_bases: self.duplicate_bases,
            unload_without_load: self.unload_without_load,
            malformed: self.malformed,
            modules,
        }
    }
}

fn map_matching_modules<'a>(
    modules: impl Iterator<Item = &'a LoadedModule>,
    address: u64,
) -> Option<ModuleFrameMapping> {
    let mut first: Option<&LoadedModule> = None;
    let mut candidates = Vec::new();
    for module in modules.filter(|module| {
        address >= module.base && address < module.base.saturating_add(u64::from(module.size))
    }) {
        if let Some(first_match) = first {
            if candidates.is_empty() {
                candidates.push(first_match.name.clone());
            }
            candidates.push(module.name.clone());
        } else {
            first = Some(module);
        }
    }
    if !candidates.is_empty() {
        return Some(ModuleFrameMapping::Ambiguous { candidates });
    }
    first.map(|module| {
        ModuleFrameMapping::Mapped(ModuleFrameMap {
            module: module.name.clone(),
            base: module.base,
            relative_address: address - module.base,
            identity: ModuleIdentity::from_image_id(&module.image_id),
        })
    })
}

pub(crate) fn decode_module_init(
    event: &EventTypeInfo,
    data: &[u8],
    base_offset: u64,
) -> Result<(SymbolFormat, u8), TraceError> {
    let aux = parse_protocol5_aux(data, event_data_size(event), base_offset)?;
    let format_raw = optional_aux_text(event, &aux, "SymbolFormat")?.unwrap_or_default();
    let format = SymbolFormat::parse(&format_raw);
    let shift = read_u8_field(event, data, "ModuleBaseShift", base_offset)?;
    Ok((format, shift))
}

pub(crate) fn decode_module_load(
    event: &EventTypeInfo,
    data: &[u8],
    base_offset: u64,
) -> Result<(String, u64, u32, Vec<u8>), TraceError> {
    let aux = parse_protocol5_aux(data, event_data_size(event), base_offset)?;
    let name = optional_aux_text(event, &aux, "Name")?.unwrap_or_default();
    let raw_base = read_u64_field(event, data, "Base", base_offset)?;
    let size = read_u32_field(event, data, "Size", base_offset)?;
    let image_id_bytes = read_required_aux_bytes(event, &aux, "ImageId")?;
    if image_id_bytes.len() > MAX_IMAGE_ID_BYTES {
        return Err(TraceError::new(
            TraceErrorKind::ResourceLimit,
            base_offset,
            "Diagnostics.ModuleLoad.ImageId",
            format!(
                "ImageId length {} exceeds limit {MAX_IMAGE_ID_BYTES}",
                image_id_bytes.len()
            ),
        ));
    }
    Ok((name, raw_base, size, image_id_bytes.to_vec()))
}

pub(crate) fn decode_module_unload(
    event: &EventTypeInfo,
    data: &[u8],
    base_offset: u64,
) -> Result<u64, TraceError> {
    read_u64_field(event, data, "Base", base_offset)
}

/// Insights `FModuleAnalyzer::GetBaseAddress`.
pub(crate) fn reconstruct_base(raw_base: u64, module_base_shift: u8) -> Result<u64, TraceError> {
    if module_base_shift == 0 {
        return Ok(raw_base);
    }
    if raw_base > u64::from(u32::MAX) {
        return Err(TraceError::new(
            TraceErrorKind::MalformedData,
            0,
            "Diagnostics.ModuleLoad.Base",
            format!(
                "shifted base encoding expects a 32-bit Base, got {raw_base:#x} with shift {module_base_shift}"
            ),
        ));
    }
    let shifted = u64::from(raw_base as u32);
    let shift = u32::from(module_base_shift);
    if shift >= 64 || shifted > (u64::MAX >> shift) {
        return Err(TraceError::new(
            TraceErrorKind::MalformedData,
            0,
            "Diagnostics.ModuleLoad.Base",
            format!("base {raw_base:#x} << {module_base_shift} overflows u64"),
        ));
    }
    Ok(shifted << shift)
}

fn format_address(address: u64) -> String {
    format!("0x{address:x}")
}

fn hex_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

impl ModuleIdentity {
    pub(crate) fn from_image_id(image_id: &[u8]) -> Option<Self> {
        if image_id.len() != 20 {
            return None;
        }
        let guid = microsoft_guid_string(&image_id[..16])?;
        let age = u32::from_le_bytes(image_id[16..20].try_into().ok()?);
        Some(Self { guid, age })
    }
}

/// Convert Microsoft in-memory GUID bytes to canonical UUID string.
fn microsoft_guid_string(bytes: &[u8]) -> Option<String> {
    if bytes.len() != 16 {
        return None;
    }
    let data1 = u32::from_le_bytes(bytes[0..4].try_into().ok()?);
    let data2 = u16::from_le_bytes(bytes[4..6].try_into().ok()?);
    let data3 = u16::from_le_bytes(bytes[6..8].try_into().ok()?);
    Some(format!(
        "{data1:08x}-{data2:04x}-{data3:04x}-{bytes_mid}-{bytes_tail}",
        bytes_mid = hex_encode(&bytes[8..10]),
        bytes_tail = hex_encode(&bytes[10..16]),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::utrace::{EventFlags, FieldFamily, FieldInfo};

    fn module_init_event() -> EventTypeInfo {
        EventTypeInfo {
            uid: 80,
            logger: "Diagnostics".to_owned(),
            event: "ModuleInit".to_owned(),
            flags: EventFlags {
                important: true,
                maybe_has_aux: true,
                no_sync: true,
                definition: false,
            },
            fields: vec![
                FieldInfo {
                    name: "SymbolFormat".to_owned(),
                    offset: 0,
                    size: 0,
                    family: FieldFamily::Regular,
                    type_name: "ansi_string".to_owned(),
                    ref_uid: None,
                },
                FieldInfo {
                    name: "ModuleBaseShift".to_owned(),
                    offset: 0,
                    size: 1,
                    family: FieldFamily::Regular,
                    type_name: "uint8".to_owned(),
                    ref_uid: None,
                },
            ],
        }
    }

    fn aux(field_index: u8, payload: &[u8]) -> Vec<u8> {
        let pack =
            1_u32 | (u32::from(field_index) << 8) | (u32::try_from(payload.len()).unwrap() << 13);
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&pack.to_le_bytes());
        bytes.extend_from_slice(payload);
        bytes
    }

    #[test]
    fn reconstruct_base_unshifted_uses_u64() {
        assert_eq!(
            reconstruct_base(0x7ff0_0000_0000, 0).unwrap(),
            0x7ff0_0000_0000
        );
    }

    #[test]
    fn reconstruct_base_applies_shift_to_u32() {
        assert_eq!(reconstruct_base(0x7ff0, 12).unwrap(), 0x7ff0 << 12);
    }

    #[test]
    fn reconstruct_base_rejects_overflow() {
        let error = reconstruct_base(u64::from(u32::MAX), 63).unwrap_err();
        assert_eq!(error.kind(), TraceErrorKind::MalformedData);
    }

    #[test]
    fn maps_address_inside_module_range() {
        let mut provider = ModuleProvider::default();
        provider.record_init(SymbolFormat::Pdb, 0);
        provider
            .record_load("Game.exe".to_owned(), 0x1000, 0x100, vec![0; 20])
            .unwrap();
        match provider.map_address(0x1040) {
            ModuleFrameMapping::Mapped(mapped) => {
                assert_eq!(mapped.module, "Game.exe");
                assert_eq!(mapped.relative_address, 0x40);
            }
            other => panic!("expected mapped, got {other:?}"),
        }
        assert!(matches!(
            provider.map_address(0x1100),
            ModuleFrameMapping::Unmapped
        ));
        assert!(matches!(
            provider.map_address(0xfff),
            ModuleFrameMapping::Unmapped
        ));
    }

    #[test]
    fn overlapping_ranges_are_ambiguous() {
        let mut provider = ModuleProvider::default();
        provider.record_init(SymbolFormat::Pdb, 0);
        provider
            .record_load("A.dll".to_owned(), 0x1000, 0x200, vec![1; 20])
            .unwrap();
        // Force overlap in active map by inserting a second range that overlaps via direct state.
        provider.active.insert(
            0x1100,
            LoadedModule {
                name: "B.dll".to_owned(),
                base: 0x1100,
                size: 0x200,
                image_id: vec![2; 20],
                unloaded: false,
            },
        );
        match provider.map_address(0x1150) {
            ModuleFrameMapping::Ambiguous { candidates } => {
                assert!(candidates.contains(&"A.dll".to_owned()));
                assert!(candidates.contains(&"B.dll".to_owned()));
            }
            other => panic!("expected ambiguous, got {other:?}"),
        }
    }

    #[test]
    fn unload_and_reload_same_base() {
        let mut provider = ModuleProvider::default();
        provider.record_init(SymbolFormat::Pdb, 0);
        provider
            .record_load("A.dll".to_owned(), 0x2000, 0x10, vec![0; 20])
            .unwrap();
        provider.record_unload(0x2000).unwrap();
        provider
            .record_load("A2.dll".to_owned(), 0x2000, 0x10, vec![0; 20])
            .unwrap();
        match provider.map_address(0x2004) {
            ModuleFrameMapping::Mapped(mapped) => assert_eq!(mapped.module, "A2.dll"),
            other => panic!("expected remapped module, got {other:?}"),
        }
    }

    #[test]
    fn active_module_catalog_is_bounded_with_retained_history() {
        let mut provider = ModuleProvider::default();
        provider.record_init(SymbolFormat::Pdb, 0);
        for index in 0..=MAX_RETAINED_MODULES {
            provider
                .record_load(
                    format!("Module{index}.dll"),
                    0x1000 + u64::try_from(index).unwrap() * 0x1000,
                    0x100,
                    vec![0; 20],
                )
                .unwrap();
        }

        assert_eq!(provider.active.len(), MAX_RETAINED_MODULES);
        assert_eq!(provider.retained_modules.len(), MAX_RETAINED_MODULES);
        assert_eq!(provider.dropped, 1);
        assert!(provider.truncated);
    }

    #[test]
    fn missing_init_is_flagged() {
        let mut provider = ModuleProvider::default();
        provider
            .record_load("Early.dll".to_owned(), 0x3000, 0x10, vec![0; 20])
            .unwrap();
        assert!(provider.missing_init);
    }

    #[test]
    fn parses_windows_image_id_identity() {
        let image_id: Vec<u8> = (0..20)
            .map(|i| {
                let s = "7fce1bf5888c1e43ada14a8298916b8c01000000";
                u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).unwrap()
            })
            .collect();
        let identity = ModuleIdentity::from_image_id(&image_id).unwrap();
        assert_eq!(identity.guid, "f51bce7f-8c88-431e-ada1-4a8298916b8c");
        assert_eq!(identity.age, 1);
    }

    #[test]
    fn decodes_module_init_wire() {
        let event = module_init_event();
        let mut data = vec![0_u8];
        data.extend_from_slice(&aux(0, b"pdb"));
        data.push(3);
        let (format, shift) = decode_module_init(&event, &data, 0).unwrap();
        assert_eq!(format, SymbolFormat::Pdb);
        assert_eq!(shift, 0);
    }
}
