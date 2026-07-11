//! Bounded catalog for Unreal `Memory.CallstackSpec` raw program-counter stacks.

use std::collections::BTreeMap;

use crate::Reader;
use crate::utrace::{
    CallstackDashboard, CallstackEntry, CallstackResolution, EventTypeInfo, TraceError,
    TraceErrorKind, event_data_size, parse_protocol5_aux, read_required_aux_bytes, read_u32_field,
};

/// Insights semantic maximum frames retained per stack.
pub(crate) const MAX_FRAMES_PER_STACK: usize = 255;
const MAX_RETAINED_STACKS: usize = 4_096;
const MAX_RETAINED_FRAMES_TOTAL: usize = 65_536;
const FRAME_SIZE: usize = size_of::<u64>();

/// Numeric callstack catalog id. `0` means no recorded stack.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub(crate) struct CallstackId(pub(crate) u32);

impl CallstackId {
    pub(crate) const fn is_none(self) -> bool {
        self.0 == 0
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct DecodedCallstackSpec {
    pub(crate) id: CallstackId,
    pub(crate) frames: Vec<u64>,
    pub(crate) declared_frame_count: u64,
    pub(crate) frames_truncated: bool,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct CallstackProvider {
    observed: u64,
    retained: u64,
    dropped: u64,
    truncated: bool,
    duplicate_ids: u64,
    malformed: u64,
    id_zero: u64,
    total_frames_retained: usize,
    stacks: BTreeMap<u32, RetainedCallstack>,
    unresolved_references: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct RetainedCallstack {
    frames: Vec<u64>,
    frames_truncated: bool,
}

impl CallstackProvider {
    pub(crate) fn record(&mut self, decoded: DecodedCallstackSpec) {
        self.observed = self.observed.saturating_add(1);

        if decoded.id.is_none() {
            self.id_zero = self.id_zero.saturating_add(1);
            return;
        }

        if self.stacks.contains_key(&decoded.id.0) {
            self.duplicate_ids = self.duplicate_ids.saturating_add(1);
            return;
        }

        let frame_count = decoded.frames.len();
        if self.stacks.len() >= MAX_RETAINED_STACKS
            || self.total_frames_retained.saturating_add(frame_count) > MAX_RETAINED_FRAMES_TOTAL
        {
            self.dropped = self.dropped.saturating_add(1);
            self.truncated = true;
            return;
        }

        self.total_frames_retained = self.total_frames_retained.saturating_add(frame_count);
        self.retained = self.retained.saturating_add(1);
        self.stacks.insert(
            decoded.id.0,
            RetainedCallstack {
                frames: decoded.frames,
                frames_truncated: decoded.frames_truncated,
            },
        );
    }

    pub(crate) fn resolve(&self, id: CallstackId) -> CallstackResolution {
        if id.is_none() {
            return CallstackResolution::None;
        }
        if self.stacks.contains_key(&id.0) {
            return CallstackResolution::Resolved;
        }
        if self.truncated {
            CallstackResolution::CatalogTruncated
        } else {
            CallstackResolution::Missing
        }
    }

    pub(crate) fn note_unresolved_reference(&mut self) {
        self.unresolved_references = self.unresolved_references.saturating_add(1);
    }

    pub(crate) fn dashboard_mapped(
        self,
        mut map_frame: impl FnMut(u64) -> Option<crate::utrace::MappedCallstackFrame>,
    ) -> CallstackDashboard {
        let mut stacks = self
            .stacks
            .into_iter()
            .map(|(id, stack)| {
                let mut frames = Vec::with_capacity(stack.frames.len());
                let mut mapped_frames = Vec::with_capacity(stack.frames.len());
                for address in stack.frames {
                    frames.push(format_frame_address(address));
                    if let Some(mapped) = map_frame(address) {
                        mapped_frames.push(mapped);
                    }
                }
                CallstackEntry {
                    id,
                    frame_count: u64::try_from(frames.len()).unwrap_or(u64::MAX),
                    frames_truncated: stack.frames_truncated,
                    frames,
                    mapped_frames,
                }
            })
            .collect::<Vec<_>>();
        stacks.sort_by_key(|entry| entry.id);

        CallstackDashboard {
            observed: self.observed,
            retained: self.retained,
            dropped: self.dropped,
            truncated: self.truncated,
            duplicate_ids: self.duplicate_ids,
            malformed: self.malformed,
            id_zero: self.id_zero,
            total_frames_retained: u64::try_from(self.total_frames_retained).unwrap_or(u64::MAX),
            unresolved_references: self.unresolved_references,
            stacks,
        }
    }
}

pub(crate) fn decode_callstack_spec(
    event: &EventTypeInfo,
    data: &[u8],
    base_offset: u64,
) -> Result<DecodedCallstackSpec, TraceError> {
    let id = CallstackId(read_u32_field(event, data, "CallstackId", base_offset)?);
    let aux = parse_protocol5_aux(data, event_data_size(event), base_offset)?;
    let frames_bytes = read_required_aux_bytes(event, &aux, "Frames")?;
    let (frames, declared_frame_count, frames_truncated) =
        decode_callstack_frames(frames_bytes, base_offset)?;
    Ok(DecodedCallstackSpec {
        id,
        frames,
        declared_frame_count,
        frames_truncated,
    })
}

pub(crate) fn decode_callstack_frames(
    frames_bytes: &[u8],
    base_offset: u64,
) -> Result<(Vec<u64>, u64, bool), TraceError> {
    if frames_bytes.len() % FRAME_SIZE != 0 {
        return Err(TraceError::new(
            TraceErrorKind::MalformedData,
            base_offset,
            "Memory.CallstackSpec.Frames",
            format!(
                "{} byte Frames array is not a whole number of u64 values",
                frames_bytes.len()
            ),
        ));
    }

    let declared_count = frames_bytes.len() / FRAME_SIZE;
    let declared_frame_count = u64::try_from(declared_count).unwrap_or(u64::MAX);
    let retain_count = declared_count.min(MAX_FRAMES_PER_STACK);
    let frames_truncated = declared_count > MAX_FRAMES_PER_STACK;

    let mut reader = Reader::new(frames_bytes);
    let mut frames = Vec::with_capacity(retain_count);
    for index in 0..retain_count {
        let path = format!("Memory.CallstackSpec.Frames[{index}]");
        let mut element = reader.take_bounded(u64::try_from(FRAME_SIZE).unwrap(), &path)?;
        frames.push(element.read_u64(&path)?);
    }

    Ok((frames, declared_frame_count, frames_truncated))
}

pub(crate) fn format_frame_address(address: u64) -> String {
    format!("0x{address:x}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::utrace::{EventFlags, FieldFamily, FieldInfo};

    fn callstack_event() -> EventTypeInfo {
        EventTypeInfo {
            uid: 90,
            logger: "Memory".to_owned(),
            event: "CallstackSpec".to_owned(),
            flags: EventFlags {
                important: true,
                maybe_has_aux: true,
                no_sync: true,
                definition: false,
            },
            fields: vec![
                FieldInfo {
                    name: "CallstackId".to_owned(),
                    offset: 0,
                    size: 4,
                    family: FieldFamily::Regular,
                    type_name: "uint32".to_owned(),
                    ref_uid: None,
                },
                FieldInfo {
                    name: "Frames".to_owned(),
                    offset: 4,
                    size: 0,
                    family: FieldFamily::Regular,
                    type_name: "array".to_owned(),
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

    fn frames_payload(addresses: &[u64]) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(addresses.len() * FRAME_SIZE);
        for address in addresses {
            bytes.extend_from_slice(&address.to_le_bytes());
        }
        bytes
    }

    fn spec_payload(id: u32, addresses: &[u64]) -> Vec<u8> {
        let mut data = Vec::new();
        data.extend_from_slice(&id.to_le_bytes());
        data.extend_from_slice(&aux(1, &frames_payload(addresses)));
        data.push(3);
        data
    }

    #[test]
    fn decodes_empty_frame_array() {
        let event = callstack_event();
        let data = spec_payload(7, &[]);
        let decoded = decode_callstack_spec(&event, &data, 0).unwrap();
        assert_eq!(decoded.id, CallstackId(7));
        assert!(decoded.frames.is_empty());
        assert_eq!(decoded.declared_frame_count, 0);
        assert!(!decoded.frames_truncated);
    }

    #[test]
    fn decodes_single_frame() {
        let event = callstack_event();
        let data = spec_payload(3, &[0x1000]);
        let decoded = decode_callstack_spec(&event, &data, 0).unwrap();
        assert_eq!(decoded.frames, vec![0x1000]);
        assert_eq!(decoded.declared_frame_count, 1);
    }

    #[test]
    fn retains_exactly_255_frames() {
        let addresses = (1..=255_u64).collect::<Vec<_>>();
        let (frames, declared, truncated) =
            decode_callstack_frames(&frames_payload(&addresses), 0).unwrap();
        assert_eq!(frames.len(), 255);
        assert_eq!(declared, 255);
        assert!(!truncated);
        assert_eq!(frames[0], 1);
        assert_eq!(frames[254], 255);
    }

    #[test]
    fn truncates_256th_frame_to_insights_cap() {
        let addresses = (1..=256_u64).collect::<Vec<_>>();
        let (frames, declared, truncated) =
            decode_callstack_frames(&frames_payload(&addresses), 0).unwrap();
        assert_eq!(frames.len(), 255);
        assert_eq!(declared, 256);
        assert!(truncated);
        assert_eq!(frames[254], 255);
    }

    #[test]
    fn rejects_truncated_frame_payload() {
        let error = decode_callstack_frames(&[1, 2, 3], 0).unwrap_err();
        assert_eq!(error.kind(), TraceErrorKind::MalformedData);
        assert!(error.detail().contains("whole number of u64"));
    }

    #[test]
    fn absurd_frame_count_does_not_allocate_beyond_cap() {
        // 10_000 declared u64s would be 80 KiB of addresses; retain only 255.
        let addresses = vec![0xdead_u64; 10_000];
        let (frames, declared, truncated) =
            decode_callstack_frames(&frames_payload(&addresses), 0).unwrap();
        assert_eq!(frames.len(), MAX_FRAMES_PER_STACK);
        assert_eq!(declared, 10_000);
        assert!(truncated);
        assert_eq!(frames.capacity(), MAX_FRAMES_PER_STACK);
    }

    #[test]
    fn id_zero_is_observed_but_not_retained() {
        let mut provider = CallstackProvider::default();
        provider.record(DecodedCallstackSpec {
            id: CallstackId(0),
            frames: vec![1],
            declared_frame_count: 1,
            frames_truncated: false,
        });
        let dashboard = provider.dashboard_mapped(|_| None);
        assert_eq!(dashboard.observed, 1);
        assert_eq!(dashboard.id_zero, 1);
        assert_eq!(dashboard.retained, 0);
        assert!(dashboard.stacks.is_empty());
    }

    #[test]
    fn duplicate_ids_keep_first_entry() {
        let mut provider = CallstackProvider::default();
        provider.record(DecodedCallstackSpec {
            id: CallstackId(9),
            frames: vec![0x111],
            declared_frame_count: 1,
            frames_truncated: false,
        });
        provider.record(DecodedCallstackSpec {
            id: CallstackId(9),
            frames: vec![0x222],
            declared_frame_count: 1,
            frames_truncated: false,
        });
        let dashboard = provider.dashboard_mapped(|_| None);
        assert_eq!(dashboard.duplicate_ids, 1);
        assert_eq!(dashboard.retained, 1);
        assert_eq!(dashboard.stacks[0].frames, vec!["0x111".to_owned()]);
    }

    #[test]
    fn catalog_cap_drops_additional_stacks() {
        let mut provider = CallstackProvider::default();
        for id in 1..=(MAX_RETAINED_STACKS as u32 + 1) {
            provider.record(DecodedCallstackSpec {
                id: CallstackId(id),
                frames: vec![u64::from(id)],
                declared_frame_count: 1,
                frames_truncated: false,
            });
        }
        let dashboard = provider.dashboard_mapped(|_| None);
        assert_eq!(
            dashboard.retained,
            u64::try_from(MAX_RETAINED_STACKS).unwrap()
        );
        assert_eq!(dashboard.dropped, 1);
        assert!(dashboard.truncated);
    }

    #[test]
    fn total_frame_cap_drops_oversized_catalog_growth() {
        let mut provider = CallstackProvider::default();
        let frames_per_stack = 64_usize;
        let stacks_before_cap = MAX_RETAINED_FRAMES_TOTAL / frames_per_stack;
        for id in 1..=(stacks_before_cap as u32) {
            provider.record(DecodedCallstackSpec {
                id: CallstackId(id),
                frames: vec![1; frames_per_stack],
                declared_frame_count: u64::try_from(frames_per_stack).unwrap(),
                frames_truncated: false,
            });
        }
        provider.record(DecodedCallstackSpec {
            id: CallstackId(u32::MAX),
            frames: vec![1; frames_per_stack],
            declared_frame_count: u64::try_from(frames_per_stack).unwrap(),
            frames_truncated: false,
        });
        let dashboard = provider.dashboard_mapped(|_| None);
        assert_eq!(
            dashboard.retained,
            u64::try_from(stacks_before_cap).unwrap()
        );
        assert_eq!(dashboard.dropped, 1);
        assert!(dashboard.truncated);
        assert_eq!(
            dashboard.total_frames_retained,
            u64::try_from(stacks_before_cap * frames_per_stack).unwrap()
        );
    }

    #[test]
    fn formats_addresses_as_hex_strings() {
        let mut provider = CallstackProvider::default();
        provider.record(DecodedCallstackSpec {
            id: CallstackId(1),
            frames: vec![0xdead_beef_u64, 0x10],
            declared_frame_count: 2,
            frames_truncated: false,
        });
        let dashboard = provider.dashboard_mapped(|_| None);
        assert_eq!(
            dashboard.stacks[0].frames,
            vec!["0xdeadbeef".to_owned(), "0x10".to_owned()]
        );
    }

    #[test]
    fn resolve_distinguishes_none_resolved_missing_and_truncated() {
        let mut provider = CallstackProvider::default();
        provider.record(DecodedCallstackSpec {
            id: CallstackId(5),
            frames: vec![1],
            declared_frame_count: 1,
            frames_truncated: false,
        });
        assert_eq!(provider.resolve(CallstackId(0)), CallstackResolution::None);
        assert_eq!(
            provider.resolve(CallstackId(5)),
            CallstackResolution::Resolved
        );
        assert_eq!(
            provider.resolve(CallstackId(6)),
            CallstackResolution::Missing
        );

        provider.truncated = true;
        assert_eq!(
            provider.resolve(CallstackId(6)),
            CallstackResolution::CatalogTruncated
        );
    }
}
