//! Browser-facing, deliberately small WASM surface.
//!
//! The worker owns JavaScript timing and transport. This module accepts browser
//! supplied bytes, invokes the same parser entry points as native code, and
//! serializes stable JSON envelopes.

use crate::utrace_progress::{
    DashboardBootstrap, DashboardPatch, DecodeProgress, PROGRESS_PROTOCOL_VERSION,
};
use crate::utrace_session::MAX_INPUT_BYTES;
use serde::Serialize;
use wasm_bindgen::prelude::*;

#[cfg(feature = "wasm-uasset")]
const MAX_UASSET_BYTES: usize = 256 * 1024 * 1024;
#[cfg(feature = "wasm-uasset")]
const UASSET_SCHEMA_VERSION: u32 = 6;
const UTRACE_SCHEMA_VERSION: u32 = 2;

#[cfg(feature = "wasm-uasset")]
#[derive(Serialize)]
struct TableOutput {
    count: u32,
    offset: u64,
}

#[cfg(feature = "wasm-uasset")]
#[derive(Serialize)]
struct UassetOutput {
    schema_version: u32,
    status: &'static str,
    path: String,
    package: UassetPackageOutput,
    assets: Vec<serde_json::Value>,
}

#[cfg(feature = "wasm-uasset")]
#[derive(Serialize)]
struct UassetPackageOutput {
    name: String,
    version: VersionOutput,
    package_flags: u32,
    summary_size: u64,
    total_header_size: u32,
    names: TableOutput,
    imports: TableOutput,
    exports: TableOutput,
    #[serde(skip_serializing_if = "Option::is_none")]
    soft_object_paths: Option<SoftObjectPathsOutput>,
}

#[cfg(feature = "wasm-uasset")]
#[derive(Serialize)]
struct VersionOutput {
    legacy_file: i32,
    legacy_ue3: Option<i32>,
    ue4: i32,
    ue5: i32,
    licensee: i32,
}

#[cfg(feature = "wasm-uasset")]
#[derive(Serialize)]
struct SoftObjectPathsOutput {
    count: u32,
    parsed_count: usize,
}

#[derive(Serialize)]
struct UtraceOutput<T: Serialize> {
    schema_version: u32,
    status: &'static str,
    path: String,
    #[serde(flatten)]
    body: T,
}

#[derive(Serialize)]
struct TraceBody<T: Serialize> {
    #[serde(rename = "trace")]
    value: T,
}

#[derive(Serialize)]
struct InventoryBody<T: Serialize> {
    inventory: T,
}

#[derive(Serialize)]
struct DashboardBody<T: Serialize> {
    dashboard: T,
}

#[derive(Serialize)]
struct DashboardBundleBody<D: Serialize, I: Serialize> {
    dashboard: D,
    inventory: I,
}

#[derive(Serialize)]
struct TimelineBody<T: Serialize> {
    timeline: T,
}

#[derive(serde::Deserialize, Default)]
struct DashboardInput {
    max_frames: Option<usize>,
    frame: Option<u32>,
    timeline_limit: Option<usize>,
    gpu_frame: Option<u32>,
    gpu_timeline_limit: Option<usize>,
}

#[derive(serde::Deserialize, Default)]
struct TimelineQueryInput {
    start_cycle: Option<u64>,
    end_cycle: Option<u64>,
    thread: Option<u16>,
    search: Option<String>,
    limit: Option<usize>,
}

#[cfg(feature = "wasm-uasset")]
fn bounded_uasset(bytes: &[u8]) -> Result<(), JsValue> {
    if bytes.len() > MAX_UASSET_BYTES {
        return Err(JsValue::from_str(&format!(
            "uasset-inspect input is {} bytes; browser limit is {MAX_UASSET_BYTES} bytes",
            bytes.len(),
        )));
    }
    Ok(())
}

fn json(value: &impl Serialize) -> Result<String, JsValue> {
    serde_json::to_string(value).map_err(|error| JsValue::from_str(&error.to_string()))
}

fn dashboard_options(options_json: &str) -> Result<crate::utrace::DashboardOptions, JsValue> {
    let input: DashboardInput = serde_json::from_str(options_json)
        .map_err(|error| JsValue::from_str(&format!("invalid dashboard options: {error}")))?;
    Ok(crate::utrace::DashboardOptions {
        max_frames: input.max_frames,
        timeline_frame: input.frame,
        timeline_limit: input.timeline_limit,
        gpu_timeline_frame: input.gpu_frame,
        gpu_timeline_limit: input.gpu_timeline_limit,
    })
}

#[derive(serde::Deserialize)]
struct GpuTimelineQueryInput {
    frame_number: u32,
    limit: Option<usize>,
}

fn bounded_utrace(bytes: &[u8]) -> Result<(), JsValue> {
    if bytes.len() > MAX_INPUT_BYTES {
        return Err(JsValue::from_str(&format!(
            "UTrace input is {} bytes; browser limit is {MAX_INPUT_BYTES} bytes",
            bytes.len(),
        )));
    }
    Ok(())
}

fn inspect_utrace_output(filename: &str, bytes: &[u8]) -> Result<String, JsValue> {
    json(&UtraceOutput {
        schema_version: UTRACE_SCHEMA_VERSION,
        status: "ok",
        path: filename.to_owned(),
        body: TraceBody {
            value: crate::utrace::inspect(bytes)
                .map_err(|error| JsValue::from_str(&error.to_string()))?,
        },
    })
}

fn inventory_utrace_output(filename: &str, bytes: &[u8]) -> Result<String, JsValue> {
    json(&UtraceOutput {
        schema_version: UTRACE_SCHEMA_VERSION,
        status: "ok",
        path: filename.to_owned(),
        body: InventoryBody {
            inventory: crate::utrace::inventory(bytes)
                .map_err(|error| JsValue::from_str(&error.to_string()))?,
        },
    })
}

fn dashboard_utrace_output(
    filename: &str,
    bytes: &[u8],
    options_json: &str,
) -> Result<String, JsValue> {
    let dashboard = crate::utrace::dashboard_with_options(bytes, dashboard_options(options_json)?)
        .map_err(|error| JsValue::from_str(&error.to_string()))?;
    json(&UtraceOutput {
        schema_version: UTRACE_SCHEMA_VERSION,
        status: "ok",
        path: filename.to_owned(),
        body: DashboardBody { dashboard },
    })
}

fn dashboard_bundle_utrace_output(
    filename: &str,
    bytes: &[u8],
    options_json: &str,
) -> Result<String, JsValue> {
    let (dashboard, inventory) = crate::utrace::dashboard_and_inventory_with_options(
        bytes,
        dashboard_options(options_json)?,
    )
    .map_err(|error| JsValue::from_str(&error.to_string()))?;
    json(&UtraceOutput {
        schema_version: UTRACE_SCHEMA_VERSION,
        status: "ok",
        path: filename.to_owned(),
        body: DashboardBundleBody {
            dashboard,
            inventory,
        },
    })
}

/// Inspects a UTrace capture and returns the schema-versioned JSON envelope.
#[wasm_bindgen(js_name = inspectUtrace)]
pub fn inspect_utrace(filename: &str, bytes: &[u8]) -> Result<String, JsValue> {
    bounded_utrace(bytes)?;
    inspect_utrace_output(filename, bytes)
}

/// Produces the parser-oriented UTrace event inventory JSON envelope.
#[wasm_bindgen(js_name = inventoryUtrace)]
pub fn inventory_utrace(filename: &str, bytes: &[u8]) -> Result<String, JsValue> {
    bounded_utrace(bytes)?;
    inventory_utrace_output(filename, bytes)
}

/// Produces the UTrace dashboard JSON envelope using the requested bounded views.
#[wasm_bindgen(js_name = dashboardUtrace)]
pub fn dashboard_utrace(
    filename: &str,
    bytes: &[u8],
    options_json: &str,
) -> Result<String, JsValue> {
    bounded_utrace(bytes)?;
    dashboard_utrace_output(filename, bytes, options_json)
}

/// Produces dashboard and inventory projections in one UTrace packet pass.
#[wasm_bindgen(js_name = dashboardBundleUtrace)]
pub fn dashboard_bundle_utrace(
    filename: &str,
    bytes: &[u8],
    options_json: &str,
) -> Result<String, JsValue> {
    bounded_utrace(bytes)?;
    dashboard_bundle_utrace_output(filename, bytes, options_json)
}

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum WasmProgressEvent {
    Bootstrap {
        protocol_version: u32,
        sequence: u64,
        progress: DecodeProgress,
        bootstrap: DashboardBootstrap,
    },
    Snapshot {
        protocol_version: u32,
        sequence: u64,
        progress: DecodeProgress,
        patch: DashboardPatch,
    },
    Complete {
        protocol_version: u32,
        sequence: u64,
        progress: DecodeProgress,
        dashboard: Box<UtraceOutput<DashboardBody<crate::utrace::TraceDashboard>>>,
        inventory: Box<UtraceOutput<InventoryBody<crate::utrace::TraceInventory>>>,
        timeline_index: crate::utrace::CpuTimelineIndexInfo,
    },
}

#[wasm_bindgen]
pub struct ProgressiveUtraceSession {
    inner: Option<crate::utrace::ProgressiveDashboardSession>,
    filename: String,
    total_bytes: u64,
    sequence: u64,
    chunk_count: u64,
    bootstrap_emitted: bool,
    last_frame_revision: u64,
    timeline_index: Option<crate::utrace::CpuTimelineMemoryIndex>,
    gpu_timeline_index: Option<crate::utrace::GpuTimelineMemoryIndex>,
}

#[wasm_bindgen]
impl ProgressiveUtraceSession {
    #[wasm_bindgen(constructor)]
    pub fn new(filename: String, total_bytes: f64, options_json: &str) -> Result<Self, JsValue> {
        if !total_bytes.is_finite()
            || total_bytes < 0.0
            || total_bytes.fract() != 0.0
            || total_bytes > 9_007_199_254_740_991.0
        {
            return Err(JsValue::from_str("invalid progressive total byte count"));
        }
        if total_bytes > MAX_INPUT_BYTES as f64 {
            return Err(JsValue::from_str(&format!(
                "progressive UTrace input exceeds browser limit of {MAX_INPUT_BYTES} bytes"
            )));
        }
        let options = dashboard_options(options_json)?;
        Ok(Self {
            inner: Some(crate::utrace::ProgressiveDashboardSession::new(options)),
            filename,
            total_bytes: total_bytes as u64,
            sequence: 0,
            chunk_count: 0,
            bootstrap_emitted: false,
            last_frame_revision: 0,
            timeline_index: None,
            gpu_timeline_index: None,
        })
    }

    pub fn push_chunk(&mut self, bytes: &[u8]) -> Result<String, JsValue> {
        let session = self
            .inner
            .as_mut()
            .ok_or_else(|| JsValue::from_str("session already finished"))?;
        session
            .push_chunk(bytes)
            .map_err(|error| JsValue::from_str(&error.to_string()))?;
        self.chunk_count += 1;
        let mut events = Vec::new();
        if !self.bootstrap_emitted {
            if let Some((progress, bootstrap)) = session.bootstrap(Some(self.total_bytes)) {
                events.push(WasmProgressEvent::Bootstrap {
                    protocol_version: PROGRESS_PROTOCOL_VERSION,
                    sequence: self.sequence,
                    progress,
                    bootstrap,
                });
                self.sequence += 1;
                self.bootstrap_emitted = true;
            }
        }
        let (progress, patch) = session.frame_patch(Some(self.total_bytes));
        let frame_revision = session.frame_revision();
        if frame_revision != self.last_frame_revision {
            self.last_frame_revision = frame_revision;
            events.push(WasmProgressEvent::Snapshot {
                protocol_version: PROGRESS_PROTOCOL_VERSION,
                sequence: self.sequence,
                progress,
                patch,
            });
            self.sequence += 1;
        } else if self.chunk_count % 16 == 0 {
            let (progress, patch) = session.transport_patch(Some(self.total_bytes));
            events.push(WasmProgressEvent::Snapshot {
                protocol_version: PROGRESS_PROTOCOL_VERSION,
                sequence: self.sequence,
                progress,
                patch,
            });
            self.sequence += 1;
        }
        json(&events)
    }

    pub fn finish(&mut self) -> Result<String, JsValue> {
        let session = self
            .inner
            .take()
            .ok_or_else(|| JsValue::from_str("session already finished"))?;
        let progress = session.complete_progress(Some(self.total_bytes));
        let (dashboard, inventory, timeline_index, gpu_timeline_index) = session
            .finish_with_inventory_and_memory_timeline_index()
            .map_err(|error| JsValue::from_str(&error.to_string()))?;
        let timeline_index_info = timeline_index.info().clone();
        self.timeline_index = Some(timeline_index);
        self.gpu_timeline_index = Some(gpu_timeline_index);
        let event = WasmProgressEvent::Complete {
            protocol_version: PROGRESS_PROTOCOL_VERSION,
            sequence: self.sequence,
            progress,
            dashboard: Box::new(UtraceOutput {
                schema_version: UTRACE_SCHEMA_VERSION,
                status: "ok",
                path: self.filename.clone(),
                body: DashboardBody { dashboard },
            }),
            inventory: Box::new(UtraceOutput {
                schema_version: UTRACE_SCHEMA_VERSION,
                status: "ok",
                path: self.filename.clone(),
                body: InventoryBody { inventory },
            }),
            timeline_index: timeline_index_info,
        };
        json(&event)
    }

    pub fn analyzing(&mut self) -> Result<String, JsValue> {
        let session = self
            .inner
            .as_ref()
            .ok_or_else(|| JsValue::from_str("session already finished"))?;
        let (progress, patch) = session.analyzing_patch(Some(self.total_bytes));
        let event = WasmProgressEvent::Snapshot {
            protocol_version: PROGRESS_PROTOCOL_VERSION,
            sequence: self.sequence,
            progress,
            patch,
        };
        self.sequence += 1;
        json(&event)
    }

    pub fn query_timeline(&self, options_json: &str) -> Result<String, JsValue> {
        let input: TimelineQueryInput = serde_json::from_str(options_json)
            .map_err(|error| JsValue::from_str(&format!("invalid timeline query: {error}")))?;
        let timeline = self
            .timeline_index
            .as_ref()
            .ok_or_else(|| JsValue::from_str("timeline index is not ready"))?
            .query(&crate::utrace::CpuTimelineQuery {
                start_cycle: input.start_cycle,
                end_cycle: input.end_cycle,
                thread_id: input.thread,
                search: input.search,
                limit: input.limit,
            })
            .map_err(|error| JsValue::from_str(&error.to_string()))?;
        json(&UtraceOutput {
            schema_version: UTRACE_SCHEMA_VERSION,
            status: "ok",
            path: self.filename.clone(),
            body: TimelineBody { timeline },
        })
    }

    pub fn query_gpu_timeline(&self, options_json: &str) -> Result<String, JsValue> {
        let input: GpuTimelineQueryInput = serde_json::from_str(options_json)
            .map_err(|error| JsValue::from_str(&format!("invalid GPU timeline query: {error}")))?;
        let timeline = self
            .gpu_timeline_index
            .as_ref()
            .ok_or_else(|| JsValue::from_str("GPU timeline index is not ready"))?
            .query(input.frame_number, input.limit);
        json(&UtraceOutput {
            schema_version: UTRACE_SCHEMA_VERSION,
            status: "ok",
            path: self.filename.clone(),
            body: TimelineBody { timeline },
        })
    }
}

/// Compatibility adapter used by the in-repository browser UI.
///
/// New consumers should use the named UTrace exports above. Keeping this
/// adapter behind `wasm-uasset` lets the published UTrace package avoid a
/// string-dispatched public surface while preserving the current web UI ABI.
#[cfg(feature = "wasm-uasset")]
#[wasm_bindgen]
pub fn parse(
    kind: &str,
    filename: &str,
    bytes: &[u8],
    options_json: &str,
) -> Result<String, JsValue> {
    match kind {
        "uasset-inspect" => {
            bounded_uasset(bytes)?;
            let package = uasset_parser::Package::parse(bytes)
                .map_err(|error| JsValue::from_str(&error.to_string()))?;
            let summary = &package.summary;
            json(&UassetOutput {
                schema_version: UASSET_SCHEMA_VERSION,
                status: "ok",
                path: filename.to_owned(),
                package: UassetPackageOutput {
                    name: summary.package_name.clone(),
                    version: VersionOutput {
                        legacy_file: summary.versions.legacy_file_version,
                        legacy_ue3: summary.versions.legacy_ue3,
                        ue4: summary.versions.ue4,
                        ue5: summary.versions.ue5,
                        licensee: summary.versions.licensee,
                    },
                    package_flags: summary.versions.package_flags.bits(),
                    summary_size: summary.span.len(),
                    total_header_size: summary.total_header_size,
                    names: TableOutput {
                        count: summary.names.count,
                        offset: summary.names.offset.get(),
                    },
                    imports: TableOutput {
                        count: summary.imports.count,
                        offset: summary.imports.offset.get(),
                    },
                    exports: TableOutput {
                        count: summary.exports.count,
                        offset: summary.exports.offset.get(),
                    },
                    soft_object_paths: summary.soft_object_paths.map(|table| {
                        SoftObjectPathsOutput {
                            count: table.count,
                            parsed_count: package.soft_object_paths.len(),
                        }
                    }),
                },
                // Export adapters remain in the native CLI contract for now.
                // The UI explicitly labels this browser result as summary-only.
                assets: Vec::new(),
            })
        }
        "utrace-inspect" => inspect_utrace_output(filename, bytes),
        "utrace-inventory" => inventory_utrace_output(filename, bytes),
        "utrace-dashboard" => dashboard_utrace_output(filename, bytes, options_json),
        "utrace-dashboard-bundle" => dashboard_bundle_utrace_output(filename, bytes, options_json),
        _ => Err(JsValue::from_str("unsupported browser parser operation")),
    }
}
