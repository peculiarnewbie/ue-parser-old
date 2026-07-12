//! Optional symbolization seam for module-mapped callstack frames.

use std::collections::HashMap;
#[cfg(feature = "utrace-symbols")]
use std::path::{Path, PathBuf};

use crate::utrace::{
    MappedCallstackFrame, MappedFrameStatus, ModuleFrameMap, ModuleFrameMapping, ModuleIdentity,
};
use crate::utrace_callstacks::format_frame_address;
use crate::utrace_modules::ModuleProvider;

const MAX_SYMBOL_CACHE: usize = 16_384;

/// Result of resolving one relative address against a verified module identity.
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum SymbolResolveResult {
    Symbol {
        name: String,
        file: Option<String>,
        line: Option<u32>,
    },
    ModuleOnly,
    SymbolsMissing,
    IdentityMismatch,
    ResolverError(String),
}

/// Filesystem-backed resolver. Library parse stays free of this trait.
pub trait SymbolResolver {
    fn resolve(
        &mut self,
        module_name: &str,
        identity: Option<&ModuleIdentity>,
        relative_address: u64,
    ) -> SymbolResolveResult;
}

#[derive(Clone, Debug, Default)]
pub struct SymbolCache {
    entries: HashMap<CacheKey, SymbolResolveResult>,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct CacheKey {
    guid: String,
    age: u32,
    relative_address: u64,
}

impl SymbolCache {
    pub fn get(
        &self,
        identity: &ModuleIdentity,
        relative_address: u64,
    ) -> Option<&SymbolResolveResult> {
        self.entries.get(&CacheKey {
            guid: identity.guid.clone(),
            age: identity.age,
            relative_address,
        })
    }

    pub fn insert(
        &mut self,
        identity: &ModuleIdentity,
        relative_address: u64,
        result: SymbolResolveResult,
    ) {
        if self.entries.len() >= MAX_SYMBOL_CACHE
            && !self.entries.contains_key(&CacheKey {
                guid: identity.guid.clone(),
                age: identity.age,
                relative_address,
            })
        {
            return;
        }
        self.entries.insert(
            CacheKey {
                guid: identity.guid.clone(),
                age: identity.age,
                relative_address,
            },
            result,
        );
    }
}

/// Map a PC through the module catalog, optionally consulting a symbol resolver.
pub(crate) fn map_frame(
    modules: &ModuleProvider,
    address: u64,
    resolver: Option<&mut dyn SymbolResolver>,
    cache: &mut SymbolCache,
) -> MappedCallstackFrame {
    let address_hex = format_frame_address(address);
    match modules.map_address(address) {
        ModuleFrameMapping::Unmapped => MappedCallstackFrame {
            address: address_hex,
            module: None,
            relative_address: None,
            identity: None,
            symbol: None,
            file: None,
            line: None,
            status: MappedFrameStatus::Unmapped,
        },
        ModuleFrameMapping::Ambiguous { .. } => MappedCallstackFrame {
            address: address_hex,
            module: None,
            relative_address: None,
            identity: None,
            symbol: None,
            file: None,
            line: None,
            status: MappedFrameStatus::Ambiguous,
        },
        ModuleFrameMapping::Mapped(mapped) => {
            resolve_mapped_frame(address_hex, mapped, resolver, cache)
        }
    }
}

fn resolve_mapped_frame(
    address_hex: String,
    mapped: ModuleFrameMap,
    resolver: Option<&mut dyn SymbolResolver>,
    cache: &mut SymbolCache,
) -> MappedCallstackFrame {
    let relative_hex = format_frame_address(mapped.relative_address);
    let identity = mapped.identity.clone();
    let Some(resolver) = resolver else {
        return MappedCallstackFrame {
            address: address_hex,
            module: Some(mapped.module),
            relative_address: Some(relative_hex),
            identity,
            symbol: None,
            file: None,
            line: None,
            status: MappedFrameStatus::ModuleOffset,
        };
    };

    let cached = mapped
        .identity
        .as_ref()
        .and_then(|identity| cache.get(identity, mapped.relative_address).cloned());
    let result = cached.unwrap_or_else(|| {
        let resolved = resolver.resolve(
            &mapped.module,
            mapped.identity.as_ref(),
            mapped.relative_address,
        );
        if let Some(identity) = mapped.identity.as_ref() {
            cache.insert(identity, mapped.relative_address, resolved.clone());
        }
        resolved
    });

    match result {
        SymbolResolveResult::Symbol { name, file, line } => MappedCallstackFrame {
            address: address_hex,
            module: Some(mapped.module),
            relative_address: Some(relative_hex),
            identity,
            symbol: Some(name),
            file,
            line,
            status: MappedFrameStatus::Symbol,
        },
        SymbolResolveResult::ModuleOnly => MappedCallstackFrame {
            address: address_hex,
            module: Some(mapped.module),
            relative_address: Some(relative_hex),
            identity,
            symbol: None,
            file: None,
            line: None,
            status: MappedFrameStatus::ModuleOffset,
        },
        SymbolResolveResult::SymbolsMissing => MappedCallstackFrame {
            address: address_hex,
            module: Some(mapped.module),
            relative_address: Some(relative_hex),
            identity,
            symbol: None,
            file: None,
            line: None,
            status: MappedFrameStatus::SymbolsMissing,
        },
        SymbolResolveResult::IdentityMismatch => MappedCallstackFrame {
            address: address_hex,
            module: Some(mapped.module),
            relative_address: Some(relative_hex),
            identity,
            symbol: None,
            file: None,
            line: None,
            status: MappedFrameStatus::IdentityMismatch,
        },
        SymbolResolveResult::ResolverError(_) => MappedCallstackFrame {
            address: address_hex,
            module: Some(mapped.module),
            relative_address: Some(relative_hex),
            identity,
            symbol: None,
            file: None,
            line: None,
            status: MappedFrameStatus::ResolverError,
        },
    }
}

/// Apply a symbol resolver to already module-mapped callstack frames.
/// Raw `frames` hex addresses are never modified.
pub fn enrich_callstacks_with_symbols(
    callstacks: &mut crate::utrace::CallstackDashboard,
    resolver: &mut dyn SymbolResolver,
) {
    for stack in &mut callstacks.stacks {
        for frame in &mut stack.mapped_frames {
            if !matches!(
                frame.status,
                MappedFrameStatus::ModuleOffset | MappedFrameStatus::SymbolsMissing
            ) {
                continue;
            }
            let Some(module) = frame.module.clone() else {
                continue;
            };
            let Some(relative_hex) = frame.relative_address.as_deref() else {
                continue;
            };
            let Ok(relative) = u64::from_str_radix(relative_hex.trim_start_matches("0x"), 16)
            else {
                continue;
            };
            match resolver.resolve(&module, frame.identity.as_ref(), relative) {
                SymbolResolveResult::Symbol { name, file, line } => {
                    frame.symbol = Some(name);
                    frame.file = file;
                    frame.line = line;
                    frame.status = MappedFrameStatus::Symbol;
                }
                SymbolResolveResult::IdentityMismatch => {
                    frame.status = MappedFrameStatus::IdentityMismatch;
                }
                SymbolResolveResult::SymbolsMissing => {
                    frame.status = MappedFrameStatus::SymbolsMissing;
                }
                SymbolResolveResult::ResolverError(_) => {
                    frame.status = MappedFrameStatus::ResolverError;
                }
                SymbolResolveResult::ModuleOnly => {
                    frame.status = MappedFrameStatus::ModuleOffset;
                }
            }
        }
    }
}

/// Windows PDB resolver using `pdb-addr2line`, gated behind `utrace-symbols`.
#[cfg(feature = "utrace-symbols")]
pub struct PdbSymbolResolver {
    symbol_paths: Vec<PathBuf>,
    cache: SymbolCache,
}

#[cfg(feature = "utrace-symbols")]
impl PdbSymbolResolver {
    pub fn new(symbol_paths: Vec<PathBuf>) -> Self {
        Self {
            symbol_paths,
            cache: SymbolCache::default(),
        }
    }

    fn find_pdb(&self, module_name: &str) -> Option<PathBuf> {
        let file_name = Path::new(module_name)
            .file_stem()
            .and_then(|stem| stem.to_str())
            .map(|stem| format!("{stem}.pdb"))?;
        for root in &self.symbol_paths {
            let candidate = root.join(&file_name);
            if candidate.is_file() {
                return Some(candidate);
            }
            // Also try basename of the module next to the path root.
            let by_module = root.join(
                Path::new(module_name)
                    .file_name()
                    .map(|name| {
                        let mut pdb = PathBuf::from(name);
                        pdb.set_extension("pdb");
                        pdb
                    })
                    .unwrap_or_else(|| PathBuf::from(&file_name)),
            );
            if by_module.is_file() {
                return Some(by_module);
            }
        }
        None
    }

    fn pdb_identity_matches(path: &Path, expected: &ModuleIdentity) -> Result<bool, String> {
        use pdb_addr2line::pdb as pdb_crate;
        let file = std::fs::File::open(path).map_err(|error| error.to_string())?;
        let mut pdb = pdb_crate::PDB::open(file).map_err(|error| error.to_string())?;
        let info = pdb.pdb_information().map_err(|error| error.to_string())?;
        let guid = format!("{}", info.guid).to_ascii_lowercase();
        Ok(guid == expected.guid.to_ascii_lowercase() && info.age == expected.age)
    }
}

#[cfg(feature = "utrace-symbols")]
impl SymbolResolver for PdbSymbolResolver {
    fn resolve(
        &mut self,
        module_name: &str,
        identity: Option<&ModuleIdentity>,
        relative_address: u64,
    ) -> SymbolResolveResult {
        if let Some(identity) = identity {
            if let Some(cached) = self.cache.get(identity, relative_address) {
                return cached.clone();
            }
        }

        let Some(pdb_path) = self.find_pdb(module_name) else {
            return SymbolResolveResult::SymbolsMissing;
        };

        if let Some(identity) = identity {
            match self::PdbSymbolResolver::pdb_identity_matches(&pdb_path, identity) {
                Ok(true) => {}
                Ok(false) => return SymbolResolveResult::IdentityMismatch,
                Err(error) => return SymbolResolveResult::ResolverError(error),
            }
        }

        match resolve_pdb_address(&pdb_path, relative_address) {
            Ok(Some((name, file, line))) => {
                let result = SymbolResolveResult::Symbol { name, file, line };
                if let Some(identity) = identity {
                    self.cache
                        .insert(identity, relative_address, result.clone());
                }
                result
            }
            Ok(None) => SymbolResolveResult::ModuleOnly,
            Err(error) => SymbolResolveResult::ResolverError(error),
        }
    }
}

#[cfg(feature = "utrace-symbols")]
type ResolvedSymbol = (String, Option<String>, Option<u32>);

#[cfg(feature = "utrace-symbols")]
fn resolve_pdb_address(
    path: &Path,
    relative_address: u64,
) -> Result<Option<ResolvedSymbol>, String> {
    use pdb_addr2line::pdb as pdb_crate;
    let file = std::fs::File::open(path).map_err(|error| error.to_string())?;
    let pdb = pdb_crate::PDB::open(file).map_err(|error| error.to_string())?;
    let context_data =
        pdb_addr2line::ContextPdbData::try_from_pdb(pdb).map_err(|error| error.to_string())?;
    let context = context_data
        .make_context()
        .map_err(|error| error.to_string())?;
    let Ok(relative) = u32::try_from(relative_address) else {
        return Ok(None);
    };
    let Some(frames) = context
        .find_frames(relative)
        .map_err(|error| error.to_string())?
    else {
        return Ok(None);
    };
    let Some(frame) = frames.frames.last() else {
        return Ok(None);
    };
    let name = frame
        .function
        .clone()
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| format!("0x{relative_address:x}"));
    let file = frame.file.as_ref().map(|file| file.to_string());
    let line = frame.line;
    Ok(Some((name, file, line)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::utrace::{ModuleIdentity, SymbolFormat};
    use std::path::PathBuf;
    use std::process::Command;

    #[test]
    fn module_offset_mapping_without_resolver() {
        let mut modules = ModuleProvider::default();
        modules.record_init(SymbolFormat::Pdb, 0);
        modules
            .record_load("Game.exe".to_owned(), 0x1000, 0x100, vec![0; 20])
            .unwrap();
        let mut cache = SymbolCache::default();
        let mapped = map_frame(&modules, 0x1042, None, &mut cache);
        assert_eq!(mapped.status, MappedFrameStatus::ModuleOffset);
        assert_eq!(mapped.module.as_deref(), Some("Game.exe"));
        assert_eq!(mapped.relative_address.as_deref(), Some("0x42"));
        assert_eq!(mapped.address, "0x1042");
    }

    #[cfg(all(feature = "utrace-symbols", windows))]
    fn build_probe_pdb() -> PathBuf {
        use std::sync::OnceLock;
        static PROBE_PDB: OnceLock<PathBuf> = OnceLock::new();
        PROBE_PDB
            .get_or_init(|| {
                let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
                let src = manifest.join("tests/fixtures/symbols/probe.rs");
                let out_dir = manifest.join("target/symbol-probe");
                std::fs::create_dir_all(&out_dir).unwrap();
                let exe = out_dir.join("probe.exe");
                let status = Command::new("rustc")
                    .arg(&src)
                    .arg("-o")
                    .arg(&exe)
                    .arg("-C")
                    .arg("debuginfo=2")
                    .arg("-C")
                    .arg("opt-level=0")
                    .status()
                    .expect("rustc must be available to build the symbol probe");
                assert!(status.success(), "rustc failed to build symbol probe");
                let pdb = out_dir.join("probe.pdb");
                assert!(pdb.is_file(), "expected probe.pdb next to probe.exe");
                pdb
            })
            .clone()
    }

    #[cfg(all(feature = "utrace-symbols", windows))]
    fn probe_identity(pdb_path: &std::path::Path) -> ModuleIdentity {
        use pdb_addr2line::pdb as pdb_crate;
        let file = std::fs::File::open(pdb_path).unwrap();
        let mut pdb = pdb_crate::PDB::open(file).unwrap();
        let info = pdb.pdb_information().unwrap();
        ModuleIdentity {
            guid: format!("{}", info.guid).to_ascii_lowercase(),
            age: info.age,
        }
    }

    #[cfg(all(feature = "utrace-symbols", windows))]
    fn probe_marker_rva(pdb_path: &std::path::Path) -> u32 {
        use pdb_addr2line::pdb as pdb_crate;
        let file = std::fs::File::open(pdb_path).unwrap();
        let pdb = pdb_crate::PDB::open(file).unwrap();
        let context_data = pdb_addr2line::ContextPdbData::try_from_pdb(pdb).unwrap();
        let context = context_data.make_context().unwrap();
        context
            .functions()
            .find(|function| {
                function
                    .name
                    .as_deref()
                    .is_some_and(|name| name.contains("ue_parser_probe_marker"))
            })
            .map(|function| function.start_rva)
            .expect("probe PDB must contain ue_parser_probe_marker")
    }

    #[cfg(all(feature = "utrace-symbols", windows))]
    #[test]
    fn pdb_resolver_resolves_known_probe_symbol() {
        let pdb = build_probe_pdb();
        let identity = probe_identity(&pdb);
        let rva = u64::from(probe_marker_rva(&pdb));
        let mut resolver = PdbSymbolResolver::new(vec![pdb.parent().unwrap().to_path_buf()]);
        let result = resolver.resolve("probe.exe", Some(&identity), rva);
        match result {
            SymbolResolveResult::Symbol { name, .. } => {
                assert!(
                    name.contains("ue_parser_probe_marker"),
                    "unexpected symbol name {name}"
                );
            }
            other => panic!("expected Symbol, got {other:?}"),
        }
    }

    #[cfg(all(feature = "utrace-symbols", windows))]
    #[test]
    fn pdb_resolver_rejects_identity_mismatch() {
        let pdb = build_probe_pdb();
        let mut identity = probe_identity(&pdb);
        identity.age = identity.age.wrapping_add(99);
        let mut resolver = PdbSymbolResolver::new(vec![pdb.parent().unwrap().to_path_buf()]);
        let result = resolver.resolve("probe.exe", Some(&identity), 0x1000);
        assert_eq!(result, SymbolResolveResult::IdentityMismatch);
    }

    #[cfg(all(feature = "utrace-symbols", windows))]
    #[test]
    fn pdb_resolver_reports_symbols_missing() {
        let mut resolver = PdbSymbolResolver::new(vec![PathBuf::from("target/symbol-probe-empty")]);
        let result = resolver.resolve(
            "missing.exe",
            Some(&ModuleIdentity {
                guid: "00000000-0000-0000-0000-000000000000".to_owned(),
                age: 1,
            }),
            0x10,
        );
        assert_eq!(result, SymbolResolveResult::SymbolsMissing);
    }
}
