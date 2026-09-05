//! Shared Protocol 5+ event framing for progressive transport and serial dispatch.
//! Layouts are resolved once per declaration; every payload still uses Reader bounds.

use std::collections::BTreeMap;

use crate::Reader;
use crate::utrace::{EventTypeInfo, TraceError, TraceErrorKind, event_data_size};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ParsedNormalEvent {
    pub(crate) uid: u16,
    pub(crate) offset: usize,
    pub(crate) total_end: usize,
    pub(crate) data_start: usize,
    pub(crate) data_end: usize,
    pub(crate) has_aux: bool,
    pub(crate) serial: Option<u32>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct NormalEventLayout {
    data_size: usize,
    maybe_has_aux: bool,
    no_sync: bool,
}

pub(crate) fn normal_event_layouts(
    registry: &BTreeMap<u16, &EventTypeInfo>,
) -> Vec<Option<NormalEventLayout>> {
    let max_uid = registry.keys().next_back().copied().unwrap_or(0);
    let mut layouts = vec![None; usize::from(max_uid) + 1];
    for (&uid, event) in registry {
        layouts[usize::from(uid)] = Some(NormalEventLayout::from(*event));
    }
    layouts
}

impl From<&EventTypeInfo> for NormalEventLayout {
    fn from(event: &EventTypeInfo) -> Self {
        Self {
            data_size: event_data_size(event),
            maybe_has_aux: event.flags.maybe_has_aux,
            no_sync: event.flags.no_sync,
        }
    }
}

pub(crate) fn decode_known_scope_cycle(uid: u16, data: &[u8]) -> Option<u64> {
    match uid {
        6 if data.len() == 8 => Some(u64::from_le_bytes(data.try_into().ok()?)),
        8 if data.len() == 7 => {
            let mut bytes = [0_u8; 8];
            bytes[..7].copy_from_slice(data);
            Some(u64::from_le_bytes(bytes))
        }
        _ => None,
    }
}

#[inline(always)]
pub(crate) fn parse_protocol5_normal_event(
    reader: &mut Reader<'_>,
    layouts: &[Option<NormalEventLayout>],
) -> Result<ParsedNormalEvent, TraceError> {
    const USER_UID: u16 = 16;
    let offset = usize::try_from(reader.tell()).unwrap();
    let first = reader.read_u8("Events.Uid")?;
    let raw_uid = if (first & 1) != 0 {
        let second = reader.read_u8("Events.Uid")?;
        u16::from(first) | (u16::from(second) << 8)
    } else {
        u16::from(first)
    };
    let uid = raw_uid >> 1;

    let (event_size, has_aux, serial) = if uid < USER_UID {
        let size = match uid {
            1 => {
                if reader.remaining() < 3 {
                    return Err(TraceError::new(
                        TraceErrorKind::MalformedData,
                        reader.tell(),
                        "Aux.Header",
                        "truncated aux header",
                    ));
                }
                let rest = reader.read_bytes(3, "Aux.Header")?;
                let pack = u32::from(first)
                    | (u32::from(rest[0]) << 8)
                    | (u32::from(rest[1]) << 16)
                    | (u32::from(rest[2]) << 24);
                usize::try_from(pack >> 13).unwrap()
            }
            6 | 7 => 8,
            8 | 9 => 7,
            _ => 0,
        };
        (size, false, None)
    } else {
        let Some(layout) = layouts.get(usize::from(uid)).copied().flatten() else {
            return Err(TraceError::new(
                TraceErrorKind::MalformedData,
                u64::try_from(offset).unwrap(),
                "Events.Uid",
                format!("unknown event uid {uid} in normal stream"),
            ));
        };
        let serial = if layout.no_sync {
            None
        } else {
            let bytes = reader.read_bytes(3, "Events.Serial")?;
            Some(u32::from(bytes[0]) | (u32::from(bytes[1]) << 8) | (u32::from(bytes[2]) << 16))
        };
        (layout.data_size, layout.maybe_has_aux, serial)
    };

    let data_start = usize::try_from(reader.tell()).unwrap();
    reader.skip(u64::try_from(event_size).unwrap(), "Events.Data")?;
    let data_end = usize::try_from(reader.tell()).unwrap();
    Ok(ParsedNormalEvent {
        uid,
        offset,
        total_end: data_end,
        data_start,
        data_end,
        has_aux,
        serial,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cached_layout_preserves_serial_and_rejects_every_truncation() {
        let mut layouts = vec![None; 17];
        layouts[16] = Some(NormalEventLayout {
            data_size: 4,
            maybe_has_aux: false,
            no_sync: false,
        });
        let wire = [32, 0xff, 0xff, 0xff, 1, 2, 3, 4];
        for end in 0..wire.len() {
            assert!(
                parse_protocol5_normal_event(&mut Reader::new(&wire[..end]), &layouts).is_err()
            );
        }
        let event = parse_protocol5_normal_event(&mut Reader::new(&wire), &layouts).unwrap();
        assert_eq!(event.serial, Some(0xff_ffff));
        assert_eq!(event.data_start, 4);
        assert_eq!(event.data_end, 8);
        assert!(parse_protocol5_normal_event(&mut Reader::new(&[34]), &layouts).is_err());
    }

    #[test]
    fn aux_payload_uses_its_own_bounded_wire_size() {
        let mut wire = (2_u32 | (3 << 13)).to_le_bytes().to_vec();
        wire.extend_from_slice(&[1, 2, 3]);
        for end in 0..wire.len() {
            assert!(parse_protocol5_normal_event(&mut Reader::new(&wire[..end]), &[]).is_err());
        }
        let event = parse_protocol5_normal_event(&mut Reader::new(&wire), &[]).unwrap();
        assert_eq!(event.uid, 1);
        assert_eq!(event.data_start, 4);
        assert_eq!(event.total_end, 7);
    }
}
