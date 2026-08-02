//! Bounded byte access and Unreal primitive wire decoding.

use std::{fmt, mem::size_of};

/// An absolute, half-open byte range in the source file.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Span {
    offset: u64,
    len: u64,
}

impl Span {
    /// Creates a span after checking that its end is representable.
    pub fn new(offset: u64, len: u64) -> Result<Self, ArchiveError> {
        offset.checked_add(len).ok_or_else(|| {
            ArchiveError::new(
                ArchiveErrorKind::IntegerOverflow,
                offset,
                "span",
                format!("span length {len} overflows its start offset"),
            )
        })?;
        Ok(Self { offset, len })
    }

    #[must_use]
    pub const fn offset(self) -> u64 {
        self.offset
    }

    #[must_use]
    pub const fn len(self) -> u64 {
        self.len
    }

    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.len == 0
    }

    #[must_use]
    pub const fn end(self) -> u64 {
        self.offset + self.len
    }
}

/// Resource limits applied before allocation or repeated parsing.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ArchiveLimits {
    pub max_array_elements: usize,
    pub max_string_code_units: usize,
    pub max_allocation_bytes: usize,
}

impl Default for ArchiveLimits {
    fn default() -> Self {
        Self {
            max_array_elements: 1_000_000,
            max_string_code_units: 16 * 1024 * 1024,
            max_allocation_bytes: 256 * 1024 * 1024,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ArchiveErrorKind {
    OutOfBounds,
    InvalidSeek,
    InvalidCount,
    AllocationLimit,
    MissingNullTerminator,
    InvalidString,
    InvalidNameReference,
    IntegerOverflow,
}

/// A parse failure with an absolute source offset and logical field path.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ArchiveError {
    kind: ArchiveErrorKind,
    offset: u64,
    path: String,
    detail: String,
}

impl ArchiveError {
    fn new(
        kind: ArchiveErrorKind,
        offset: u64,
        path: impl fmt::Display,
        detail: impl Into<String>,
    ) -> Self {
        Self {
            kind,
            offset,
            path: path.to_string(),
            detail: detail.into(),
        }
    }

    #[must_use]
    pub const fn kind(&self) -> ArchiveErrorKind {
        self.kind
    }

    #[must_use]
    pub const fn offset(&self) -> u64 {
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

impl fmt::Display for ArchiveError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{:?} at byte {} while reading {}: {}",
            self.kind, self.offset, self.path, self.detail
        )
    }
}

impl std::error::Error for ArchiveError {}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Guid {
    pub a: u32,
    pub b: u32,
    pub c: u32,
    pub d: u32,
}

impl Guid {
    #[must_use]
    pub const fn is_zero(self) -> bool {
        self.a == 0 && self.b == 0 && self.c == 0 && self.d == 0
    }
}

impl fmt::Display for Guid {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{:08x}-{:08x}-{:08x}-{:08x}",
            self.a, self.b, self.c, self.d
        )
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct IoHash([u8; Self::BYTE_LEN]);

impl IoHash {
    pub const BYTE_LEN: usize = 20;

    #[must_use]
    pub const fn from_bytes(bytes: [u8; Self::BYTE_LEN]) -> Self {
        Self(bytes)
    }

    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; Self::BYTE_LEN] {
        &self.0
    }
}

impl Default for IoHash {
    fn default() -> Self {
        Self([0; Self::BYTE_LEN])
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct NameIndex(u32);

impl NameIndex {
    #[must_use]
    pub const fn get(self) -> u32 {
        self.0
    }
}

/// A package-scoped classic `FName` reference.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct NameRef {
    index: NameIndex,
    number: u32,
}

impl NameRef {
    #[must_use]
    pub const fn index(self) -> NameIndex {
        self.index
    }

    #[must_use]
    pub const fn number(self) -> u32 {
        self.number
    }
}

/// Cursor over an immutable source region. Every operation is bounded by `span`.
#[derive(Clone, Debug)]
pub struct Reader<'a> {
    source: &'a [u8],
    span: Span,
    position: u64,
    limits: ArchiveLimits,
}

impl<'a> Reader<'a> {
    #[must_use]
    pub fn new(source: &'a [u8]) -> Self {
        Self::with_limits(source, ArchiveLimits::default())
    }

    #[must_use]
    pub fn with_limits(source: &'a [u8], limits: ArchiveLimits) -> Self {
        Self {
            source,
            span: Span {
                offset: 0,
                len: u64::try_from(source.len()).expect("usize always fits in u64"),
            },
            position: 0,
            limits,
        }
    }

    #[must_use]
    pub const fn span(&self) -> Span {
        self.span
    }

    /// Returns the absolute cursor offset.
    #[must_use]
    pub const fn tell(&self) -> u64 {
        self.position
    }

    #[must_use]
    pub const fn remaining(&self) -> u64 {
        self.span.end() - self.position
    }

    pub fn seek(
        &mut self,
        absolute_offset: u64,
        path: &(impl fmt::Display + ?Sized),
    ) -> Result<(), ArchiveError> {
        if absolute_offset < self.span.offset || absolute_offset > self.span.end() {
            return Err(ArchiveError::new(
                ArchiveErrorKind::InvalidSeek,
                self.position,
                path,
                format!(
                    "target {absolute_offset} is outside bounded span {}..{}",
                    self.span.offset,
                    self.span.end()
                ),
            ));
        }
        self.position = absolute_offset;
        Ok(())
    }

    pub fn skip(
        &mut self,
        byte_count: u64,
        path: &(impl fmt::Display + ?Sized),
    ) -> Result<(), ArchiveError> {
        let target = self.position.checked_add(byte_count).ok_or_else(|| {
            ArchiveError::new(
                ArchiveErrorKind::IntegerOverflow,
                self.position,
                path,
                format!("skip length {byte_count} overflows the cursor"),
            )
        })?;
        self.seek(target, path)
    }

    /// Creates an independently positioned child reader inside this reader's region.
    pub fn bounded(
        &self,
        span: Span,
        path: &(impl fmt::Display + ?Sized),
    ) -> Result<Self, ArchiveError> {
        if span.offset < self.span.offset || span.end() > self.span.end() {
            return Err(ArchiveError::new(
                ArchiveErrorKind::OutOfBounds,
                span.offset,
                path,
                format!(
                    "child span {}..{} is outside parent span {}..{}",
                    span.offset,
                    span.end(),
                    self.span.offset,
                    self.span.end()
                ),
            ));
        }
        Ok(Self {
            source: self.source,
            span,
            position: span.offset,
            limits: self.limits,
        })
    }

    /// Takes the next `byte_count` bytes as a child reader and advances this reader.
    pub fn take_bounded(
        &mut self,
        byte_count: u64,
        path: &(impl fmt::Display + ?Sized),
    ) -> Result<Self, ArchiveError> {
        let span = Span::new(self.position, byte_count)?;
        let child = self.bounded(span, path)?;
        self.position = span.end();
        Ok(child)
    }

    pub fn read_bytes(
        &mut self,
        byte_count: usize,
        path: &(impl fmt::Display + ?Sized),
    ) -> Result<&'a [u8], ArchiveError> {
        let byte_count_u64 = u64::try_from(byte_count).map_err(|_| {
            ArchiveError::new(
                ArchiveErrorKind::IntegerOverflow,
                self.position,
                path,
                format!("byte count {byte_count} does not fit in u64"),
            )
        })?;
        let end = self.position.checked_add(byte_count_u64).ok_or_else(|| {
            ArchiveError::new(
                ArchiveErrorKind::IntegerOverflow,
                self.position,
                path,
                format!("read length {byte_count} overflows the cursor"),
            )
        })?;
        if end > self.span.end() {
            return Err(ArchiveError::new(
                ArchiveErrorKind::OutOfBounds,
                self.position,
                path,
                format!(
                    "requested {byte_count} bytes with only {} remaining",
                    self.remaining()
                ),
            ));
        }

        let start_index = usize::try_from(self.position).map_err(|_| {
            ArchiveError::new(
                ArchiveErrorKind::IntegerOverflow,
                self.position,
                path,
                "source offset does not fit in usize",
            )
        })?;
        let end_index = usize::try_from(end).map_err(|_| {
            ArchiveError::new(
                ArchiveErrorKind::IntegerOverflow,
                self.position,
                path,
                "source end offset does not fit in usize",
            )
        })?;
        self.position = end;
        Ok(&self.source[start_index..end_index])
    }

    #[inline]
    pub fn read_u8(&mut self, path: &(impl fmt::Display + ?Sized)) -> Result<u8, ArchiveError> {
        Ok(self.read_bytes(1, path)?[0])
    }

    pub fn read_i8(&mut self, path: &(impl fmt::Display + ?Sized)) -> Result<i8, ArchiveError> {
        Ok(i8::from_le_bytes([self.read_u8(path)?]))
    }

    #[inline]
    pub fn read_u16(&mut self, path: &(impl fmt::Display + ?Sized)) -> Result<u16, ArchiveError> {
        Ok(u16::from_le_bytes(self.read_array(path)?))
    }

    pub fn read_i16(&mut self, path: &(impl fmt::Display + ?Sized)) -> Result<i16, ArchiveError> {
        Ok(i16::from_le_bytes(self.read_array(path)?))
    }

    #[inline]
    pub fn read_u32(&mut self, path: &(impl fmt::Display + ?Sized)) -> Result<u32, ArchiveError> {
        Ok(u32::from_le_bytes(self.read_array(path)?))
    }

    pub fn read_i32(&mut self, path: &(impl fmt::Display + ?Sized)) -> Result<i32, ArchiveError> {
        Ok(i32::from_le_bytes(self.read_array(path)?))
    }

    #[inline]
    pub fn read_u64(&mut self, path: &(impl fmt::Display + ?Sized)) -> Result<u64, ArchiveError> {
        Ok(u64::from_le_bytes(self.read_array(path)?))
    }

    pub fn read_i64(&mut self, path: &(impl fmt::Display + ?Sized)) -> Result<i64, ArchiveError> {
        Ok(i64::from_le_bytes(self.read_array(path)?))
    }

    pub fn read_f32(&mut self, path: &(impl fmt::Display + ?Sized)) -> Result<f32, ArchiveError> {
        Ok(f32::from_le_bytes(self.read_array(path)?))
    }

    pub fn read_f64(&mut self, path: &(impl fmt::Display + ?Sized)) -> Result<f64, ArchiveError> {
        Ok(f64::from_le_bytes(self.read_array(path)?))
    }

    pub fn read_guid(&mut self, path: &(impl fmt::Display + ?Sized)) -> Result<Guid, ArchiveError> {
        Ok(Guid {
            a: self.read_u32(&format_args!("{path}.A"))?,
            b: self.read_u32(&format_args!("{path}.B"))?,
            c: self.read_u32(&format_args!("{path}.C"))?,
            d: self.read_u32(&format_args!("{path}.D"))?,
        })
    }

    pub fn read_io_hash(
        &mut self,
        path: &(impl fmt::Display + ?Sized),
    ) -> Result<IoHash, ArchiveError> {
        Ok(IoHash(self.read_array(path)?))
    }

    pub fn read_name_ref(
        &mut self,
        path: &(impl fmt::Display + ?Sized),
    ) -> Result<NameRef, ArchiveError> {
        let index_offset = self.position;
        let index = self.read_i32(&format_args!("{path}.NameIndex"))?;
        if index < 0 {
            return Err(ArchiveError::new(
                ArchiveErrorKind::InvalidNameReference,
                index_offset,
                format_args!("{path}.NameIndex"),
                format!("name index must be non-negative, got {index}"),
            ));
        }

        let number_offset = self.position;
        let number = self.read_i32(&format_args!("{path}.Number"))?;
        if number < 0 {
            return Err(ArchiveError::new(
                ArchiveErrorKind::InvalidNameReference,
                number_offset,
                format_args!("{path}.Number"),
                format!("name number must be non-negative, got {number}"),
            ));
        }

        Ok(NameRef {
            index: NameIndex(u32::try_from(index).expect("index was checked as non-negative")),
            number: u32::try_from(number).expect("number was checked as non-negative"),
        })
    }

    /// Reads `FSoftObjectPath` wire format: asset-path `FString` plus optional subpath `FString`.
    pub fn read_soft_object_path(
        &mut self,
        path: &(impl fmt::Display + ?Sized),
    ) -> Result<String, ArchiveError> {
        let asset_path = self.read_fstring(&format_args!("{path}.AssetPath"))?;
        if self.remaining() == 0 {
            return Ok(asset_path);
        }

        let sub_path = self.read_fstring(&format_args!("{path}.SubPath"))?;
        Ok(Self::format_soft_object_path(&asset_path, &sub_path))
    }

    /// Reads Unreal's signed-length, null-terminated serialized `FString`.
    pub fn read_fstring(
        &mut self,
        path: &(impl fmt::Display + ?Sized),
    ) -> Result<String, ArchiveError> {
        let length_offset = self.position;
        let signed_length = self.read_i32(&format_args!("{path}.Length"))?;
        match signed_length {
            0 => Ok(String::new()),
            1.. => self.read_ansi_string(
                u32::try_from(signed_length).expect("positive i32 always fits in u32"),
                length_offset,
                path,
                true,
            ),
            i32::MIN => Err(ArchiveError::new(
                ArchiveErrorKind::InvalidCount,
                length_offset,
                format_args!("{path}.Length"),
                "wide string length cannot be i32::MIN",
            )),
            _ => self.read_wide_string(signed_length.unsigned_abs(), length_offset, path, true),
        }
    }

    /// Reads an `FSoftObjectPath` subpath string from the package summary table.
    ///
    /// Unlike a standard `FString`, this `FUtf8String`/workaround framing
    /// (`FSoftObjectPath::SerializePathWithoutFixup`, UE `SoftObjectPath.cpp`)
    /// stores an `i32` code-unit count followed by exactly that many units with
    /// **no guaranteed null terminator** — older saves stripped trailing nulls
    /// entirely. Read exactly `count` units and trim trailing nulls, which
    /// tolerates both the unterminated and null-terminated forms.
    pub(crate) fn read_soft_object_subpath(
        &mut self,
        path: &(impl fmt::Display + ?Sized),
    ) -> Result<String, ArchiveError> {
        let length_offset = self.position;
        let signed_length = self.read_i32(&format_args!("{path}.Length"))?;
        match signed_length {
            0 => Ok(String::new()),
            1.. => self.read_ansi_string(
                u32::try_from(signed_length).expect("positive i32 always fits in u32"),
                length_offset,
                path,
                false,
            ),
            i32::MIN => Err(ArchiveError::new(
                ArchiveErrorKind::InvalidCount,
                length_offset,
                format_args!("{path}.Length"),
                "wide string length cannot be i32::MIN",
            )),
            _ => self.read_wide_string(signed_length.unsigned_abs(), length_offset, path, false),
        }
    }

    /// Reads an Unreal `TArray` count and invokes `read_element` for each item.
    pub fn read_tarray<T>(
        &mut self,
        path: &(impl fmt::Display + ?Sized),
        minimum_element_size: usize,
        mut read_element: impl FnMut(&mut Self, usize) -> Result<T, ArchiveError>,
    ) -> Result<Vec<T>, ArchiveError> {
        let count_offset = self.position;
        let count = self.read_count(&format_args!("{path}.Count"))?;
        let capacity = self.checked_vec_capacity_at::<T>(
            count,
            minimum_element_size,
            count_offset,
            &format_args!("{path}.Count"),
        )?;

        let mut values = Vec::with_capacity(capacity);
        for index in 0..count {
            values.push(read_element(self, index)?);
        }
        Ok(values)
    }

    pub fn checked_vec_capacity<T>(
        &self,
        count: usize,
        minimum_element_size: usize,
        path: &(impl fmt::Display + ?Sized),
    ) -> Result<usize, ArchiveError> {
        self.checked_vec_capacity_at::<T>(count, minimum_element_size, self.position, path)
    }

    fn checked_vec_capacity_at<T>(
        &self,
        count: usize,
        minimum_element_size: usize,
        offset: u64,
        path: &(impl fmt::Display + ?Sized),
    ) -> Result<usize, ArchiveError> {
        if count > self.limits.max_array_elements {
            return Err(ArchiveError::new(
                ArchiveErrorKind::InvalidCount,
                offset,
                path,
                format!(
                    "count {count} exceeds element limit {}",
                    self.limits.max_array_elements
                ),
            ));
        }
        self.validate_allocation(
            count,
            minimum_element_size.max(size_of::<T>()),
            offset,
            path,
        )?;
        self.validate_remaining(count, minimum_element_size, offset, path)?;
        Ok(count)
    }

    pub(crate) fn read_count(
        &mut self,
        path: &(impl fmt::Display + ?Sized),
    ) -> Result<usize, ArchiveError> {
        let offset = self.position;
        let count = self.read_i32(path)?;
        if count < 0 {
            return Err(ArchiveError::new(
                ArchiveErrorKind::InvalidCount,
                offset,
                path,
                format!("count must be non-negative, got {count}"),
            ));
        }
        let count = usize::try_from(count).expect("non-negative i32 always fits in usize");
        if count > self.limits.max_array_elements {
            return Err(ArchiveError::new(
                ArchiveErrorKind::InvalidCount,
                offset,
                path,
                format!(
                    "count {count} exceeds element limit {}",
                    self.limits.max_array_elements
                ),
            ));
        }
        Ok(count)
    }

    fn validate_allocation(
        &self,
        count: usize,
        element_size: usize,
        offset: u64,
        path: &(impl fmt::Display + ?Sized),
    ) -> Result<(), ArchiveError> {
        let allocation = count.checked_mul(element_size).ok_or_else(|| {
            ArchiveError::new(
                ArchiveErrorKind::IntegerOverflow,
                offset,
                path,
                format!("allocation for {count} elements of {element_size} bytes overflows"),
            )
        })?;
        if allocation > self.limits.max_allocation_bytes {
            return Err(ArchiveError::new(
                ArchiveErrorKind::AllocationLimit,
                offset,
                path,
                format!(
                    "minimum allocation {allocation} exceeds byte limit {}",
                    self.limits.max_allocation_bytes
                ),
            ));
        }
        Ok(())
    }

    fn validate_remaining(
        &self,
        count: usize,
        minimum_element_size: usize,
        offset: u64,
        path: &(impl fmt::Display + ?Sized),
    ) -> Result<(), ArchiveError> {
        let minimum_bytes = count.checked_mul(minimum_element_size).ok_or_else(|| {
            ArchiveError::new(
                ArchiveErrorKind::IntegerOverflow,
                offset,
                path,
                format!(
                    "serialized size for {count} elements of {minimum_element_size} bytes overflows"
                ),
            )
        })?;
        let minimum_bytes = u64::try_from(minimum_bytes).map_err(|_| {
            ArchiveError::new(
                ArchiveErrorKind::IntegerOverflow,
                offset,
                path,
                "minimum serialized size does not fit in u64",
            )
        })?;
        if minimum_bytes > self.remaining() {
            return Err(ArchiveError::new(
                ArchiveErrorKind::OutOfBounds,
                offset,
                path,
                format!(
                    "minimum serialized size {minimum_bytes} exceeds remaining bytes {}",
                    self.remaining()
                ),
            ));
        }
        Ok(())
    }

    fn format_soft_object_path(asset_path: &str, sub_path: &str) -> String {
        if sub_path.is_empty() {
            asset_path.to_owned()
        } else {
            format!("{asset_path}:{sub_path}")
        }
    }

    fn read_ansi_string(
        &mut self,
        code_units: u32,
        length_offset: u64,
        path: &(impl fmt::Display + ?Sized),
        require_terminator: bool,
    ) -> Result<String, ArchiveError> {
        let code_units = self.validate_string_length(code_units, 1, length_offset, path)?;
        let bytes = self.read_bytes(code_units, &format_args!("{path}.Data"))?;

        // Unreal's serialized ANSI form is an 8-bit code-unit string. This
        // one-to-one mapping is lossless for byte values and avoids assuming UTF-8.
        let to_string = |content: &[u8]| content.iter().map(|byte| char::from(*byte)).collect();

        if !require_terminator {
            let end = bytes
                .iter()
                .rposition(|&byte| byte != 0)
                .map_or(0, |i| i + 1);
            return Ok(to_string(&bytes[..end]));
        }

        let Some((&terminator, content)) = bytes.split_last() else {
            return Err(ArchiveError::new(
                ArchiveErrorKind::MissingNullTerminator,
                self.position,
                path,
                "non-empty ANSI FString has no payload",
            ));
        };
        if terminator != 0 {
            return Err(ArchiveError::new(
                ArchiveErrorKind::MissingNullTerminator,
                self.position - 1,
                path,
                "ANSI FString does not end in a null byte",
            ));
        }
        Ok(to_string(content))
    }

    fn read_wide_string(
        &mut self,
        code_units: u32,
        length_offset: u64,
        path: &(impl fmt::Display + ?Sized),
        require_terminator: bool,
    ) -> Result<String, ArchiveError> {
        let capacity = self.validate_string_length(code_units, 2, length_offset, path)?;
        let mut units = Vec::with_capacity(capacity);
        for index in 0..capacity {
            units.push(self.read_u16(&format_args!("{path}.Data[{index}]"))?);
        }
        let decode = |content: &[u16]| {
            String::from_utf16(content).map_err(|error| {
                ArchiveError::new(
                    ArchiveErrorKind::InvalidString,
                    length_offset,
                    path,
                    format!("wide FString is invalid UTF-16: {error}"),
                )
            })
        };

        if !require_terminator {
            let end = units
                .iter()
                .rposition(|&unit| unit != 0)
                .map_or(0, |i| i + 1);
            return decode(&units[..end]);
        }

        let Some((&terminator, content)) = units.split_last() else {
            return Err(ArchiveError::new(
                ArchiveErrorKind::MissingNullTerminator,
                self.position,
                path,
                "non-empty wide FString has no payload",
            ));
        };
        if terminator != 0 {
            return Err(ArchiveError::new(
                ArchiveErrorKind::MissingNullTerminator,
                self.position - 2,
                path,
                "wide FString does not end in a null code unit",
            ));
        }
        decode(content)
    }

    fn validate_string_length(
        &self,
        code_units: u32,
        bytes_per_unit: usize,
        offset: u64,
        path: &(impl fmt::Display + ?Sized),
    ) -> Result<usize, ArchiveError> {
        let code_units = usize::try_from(code_units).map_err(|_| {
            ArchiveError::new(
                ArchiveErrorKind::IntegerOverflow,
                offset,
                format_args!("{path}.Length"),
                "string length does not fit in usize",
            )
        })?;
        if code_units == 0 || code_units > self.limits.max_string_code_units {
            return Err(ArchiveError::new(
                ArchiveErrorKind::InvalidCount,
                offset,
                format_args!("{path}.Length"),
                format!(
                    "string code-unit count {code_units} is outside 1..={}",
                    self.limits.max_string_code_units
                ),
            ));
        }
        self.validate_allocation(
            code_units,
            bytes_per_unit,
            offset,
            &format_args!("{path}.Length"),
        )?;
        self.validate_remaining(
            code_units,
            bytes_per_unit,
            offset,
            &format_args!("{path}.Length"),
        )?;
        Ok(code_units)
    }

    fn read_array<const N: usize>(
        &mut self,
        path: &(impl fmt::Display + ?Sized),
    ) -> Result<[u8; N], ArchiveError> {
        self.read_bytes(N, path)?
            .try_into()
            .map_err(|_| unreachable!("read_bytes returned the requested length"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_little_endian_primitives() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&(-7_i8).to_le_bytes());
        bytes.extend_from_slice(&(-0x1234_i16).to_le_bytes());
        bytes.extend_from_slice(&0x1234_u16.to_le_bytes());
        bytes.extend_from_slice(&(-123_i32).to_le_bytes());
        bytes.extend_from_slice(&0x0123_4567_89ab_cdef_u64.to_le_bytes());
        bytes.extend_from_slice(&(-0x0123_4567_89ab_cdef_i64).to_le_bytes());
        bytes.extend_from_slice(&1.5_f32.to_le_bytes());
        bytes.extend_from_slice(&(-2.25_f64).to_le_bytes());

        let mut reader = Reader::new(&bytes);
        assert_eq!(reader.read_i8("I8").unwrap(), -7);
        assert_eq!(reader.read_i16("I16").unwrap(), -0x1234);
        assert_eq!(reader.read_u16("U16").unwrap(), 0x1234);
        assert_eq!(reader.read_i32("I32").unwrap(), -123);
        assert_eq!(reader.read_u64("U64").unwrap(), 0x0123_4567_89ab_cdef);
        assert_eq!(reader.read_i64("I64").unwrap(), -0x0123_4567_89ab_cdef);
        assert_eq!(reader.read_f32("F32").unwrap(), 1.5);
        assert_eq!(reader.read_f64("F64").unwrap(), -2.25);
        assert_eq!(reader.remaining(), 0);
    }

    #[test]
    fn truncated_read_reports_offset_and_field() {
        let mut reader = Reader::new(&[1, 2, 3]);
        let error = reader.read_u32("Summary.Tag").unwrap_err();

        assert_eq!(error.kind(), ArchiveErrorKind::OutOfBounds);
        assert_eq!(error.offset(), 0);
        assert_eq!(error.path(), "Summary.Tag");
        assert_eq!(reader.tell(), 0);
    }

    #[test]
    fn child_cannot_read_outside_its_span() {
        let source = [0, 1, 2, 3, 4, 5];
        let root = Reader::new(&source);
        let mut child = root.bounded(Span::new(2, 2).unwrap(), "Export[0]").unwrap();

        assert_eq!(child.read_u16("Export[0].Value").unwrap(), 0x0302);
        let error = child.read_u8("Export[0].Overflow").unwrap_err();
        assert_eq!(error.kind(), ArchiveErrorKind::OutOfBounds);
        assert_eq!(error.offset(), 4);
    }

    #[test]
    fn take_bounded_advances_parent_without_sharing_cursor() {
        let source = [1, 2, 3, 4];
        let mut root = Reader::new(&source);
        let mut child = root.take_bounded(2, "Payload").unwrap();

        assert_eq!(root.tell(), 2);
        assert_eq!(child.read_u16("Payload.Value").unwrap(), 0x0201);
        assert_eq!(root.read_u16("Tail").unwrap(), 0x0403);
    }

    #[test]
    fn checked_seek_and_skip_preserve_cursor_on_failure() {
        let mut reader = Reader::new(&[0; 4]);
        reader.skip(3, "Padding").unwrap();
        let error = reader.skip(2, "Overflow").unwrap_err();

        assert_eq!(error.kind(), ArchiveErrorKind::InvalidSeek);
        assert_eq!(reader.tell(), 3);
    }

    #[test]
    fn rejects_cursor_and_child_span_boundary_violations() {
        let source = [0; 8];
        let root = Reader::new(&source);
        let mut child = root.bounded(Span::new(2, 4).unwrap(), "Child").unwrap();

        let below = child.seek(1, "Below").unwrap_err();
        assert_eq!(below.kind(), ArchiveErrorKind::InvalidSeek);
        assert_eq!(child.tell(), 2);

        let overflow = child.skip(u64::MAX, "Overflow").unwrap_err();
        assert_eq!(overflow.kind(), ArchiveErrorKind::IntegerOverflow);
        assert_eq!(child.tell(), 2);

        let outside = child
            .bounded(Span::new(1, 2).unwrap(), "Outside")
            .unwrap_err();
        assert_eq!(outside.kind(), ArchiveErrorKind::OutOfBounds);
    }

    #[test]
    fn reads_guid_hash_and_name_reference() {
        let mut bytes = Vec::new();
        for value in [1_u32, 2, 3, 4] {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        bytes.extend(0_u8..20);
        bytes.extend_from_slice(&7_i32.to_le_bytes());
        bytes.extend_from_slice(&2_i32.to_le_bytes());

        let mut reader = Reader::new(&bytes);
        assert_eq!(
            reader.read_guid("Guid").unwrap(),
            Guid {
                a: 1,
                b: 2,
                c: 3,
                d: 4
            }
        );
        assert_eq!(
            reader.read_io_hash("Hash").unwrap().as_bytes(),
            &std::array::from_fn::<_, 20, _>(|index| u8::try_from(index).unwrap())
        );
        let name = reader.read_name_ref("Name").unwrap();
        assert_eq!(name.index().get(), 7);
        assert_eq!(name.number(), 2);
    }

    #[test]
    fn rejects_negative_name_components() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&(-1_i32).to_le_bytes());
        bytes.extend_from_slice(&0_i32.to_le_bytes());

        let error = Reader::new(&bytes).read_name_ref("Name").unwrap_err();
        assert_eq!(error.kind(), ArchiveErrorKind::InvalidNameReference);
        assert_eq!(error.path(), "Name.NameIndex");

        let mut bytes = Vec::new();
        bytes.extend_from_slice(&0_i32.to_le_bytes());
        bytes.extend_from_slice(&(-1_i32).to_le_bytes());
        let error = Reader::new(&bytes).read_name_ref("Name").unwrap_err();
        assert_eq!(error.kind(), ArchiveErrorKind::InvalidNameReference);
        assert_eq!(error.path(), "Name.Number");
    }

    #[test]
    fn reads_empty_ansi_and_wide_fstrings() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&0_i32.to_le_bytes());
        bytes.extend_from_slice(&6_i32.to_le_bytes());
        bytes.extend_from_slice(b"Hello\0");
        bytes.extend_from_slice(&(-2_i32).to_le_bytes());
        for unit in "猫\0".encode_utf16() {
            bytes.extend_from_slice(&unit.to_le_bytes());
        }

        let mut reader = Reader::new(&bytes);
        assert_eq!(reader.read_fstring("Empty").unwrap(), "");
        assert_eq!(reader.read_fstring("Ansi").unwrap(), "Hello");
        assert_eq!(reader.read_fstring("Wide").unwrap(), "猫");
    }

    #[test]
    fn rejects_missing_string_terminator() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&4_i32.to_le_bytes());
        bytes.extend_from_slice(b"nope");

        let error = Reader::new(&bytes).read_fstring("Text").unwrap_err();
        assert_eq!(error.kind(), ArchiveErrorKind::MissingNullTerminator);
        assert_eq!(error.offset(), 7);
    }

    #[test]
    fn rejects_minimum_signed_string_lengths() {
        let bytes = i32::MIN.to_le_bytes();

        let fstring_error = Reader::new(&bytes).read_fstring("Text").unwrap_err();
        assert_eq!(fstring_error.kind(), ArchiveErrorKind::InvalidCount);
        assert_eq!(fstring_error.path(), "Text.Length");

        let subpath_error = Reader::new(&bytes)
            .read_soft_object_subpath("SubPath")
            .unwrap_err();
        assert_eq!(subpath_error.kind(), ArchiveErrorKind::InvalidCount);
        assert_eq!(subpath_error.path(), "SubPath.Length");
    }

    #[test]
    fn rejects_span_end_overflow() {
        let error = Span::new(u64::MAX, 1).unwrap_err();
        assert_eq!(error.kind(), ArchiveErrorKind::IntegerOverflow);
        assert_eq!(error.path(), "span");
    }

    #[test]
    fn rejects_invalid_utf16() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&(-2_i32).to_le_bytes());
        bytes.extend_from_slice(&0xD800_u16.to_le_bytes());
        bytes.extend_from_slice(&0_u16.to_le_bytes());

        let error = Reader::new(&bytes).read_fstring("Text").unwrap_err();
        assert_eq!(error.kind(), ArchiveErrorKind::InvalidString);
    }

    #[test]
    fn reads_soft_object_subpath_without_trailing_null() {
        // Real `FSoftObjectPath` summary entries store the subpath as an
        // `FUtf8String`/workaround string whose `i32` count covers exactly the
        // content bytes, with no null terminator (e.g. Blueprint
        // `:UserConstructionScript`). A strict FString reader rejects these and
        // killed the whole package parse; the tolerant reader must accept them.
        let subpath = b"UserConstructionScript";
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&(subpath.len() as i32).to_le_bytes());
        bytes.extend_from_slice(subpath);
        // A second, empty entry confirms the cursor stopped exactly after 22 bytes.
        bytes.extend_from_slice(&0_i32.to_le_bytes());

        let mut reader = Reader::new(&bytes);
        assert_eq!(
            reader.read_soft_object_subpath("Sub").unwrap(),
            "UserConstructionScript"
        );
        assert_eq!(reader.read_soft_object_subpath("Empty").unwrap(), "");
        assert_eq!(reader.remaining(), 0);

        // The strict reader must still reject the same unterminated bytes, so we
        // did not weaken normal FString parsing.
        let strict = Reader::new(&bytes).read_fstring("Sub").unwrap_err();
        assert_eq!(strict.kind(), ArchiveErrorKind::MissingNullTerminator);
    }

    #[test]
    fn reads_soft_object_subpath_tolerating_trailing_nulls() {
        // Newer saves include the null in the count; trimming trailing nulls
        // yields the same string from either framing. Also exercises the wide form.
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&11_i32.to_le_bytes());
        bytes.extend_from_slice(b"EventGraph\0");
        bytes.extend_from_slice(&(-2_i32).to_le_bytes());
        for unit in "猫\0".encode_utf16() {
            bytes.extend_from_slice(&unit.to_le_bytes());
        }

        let mut reader = Reader::new(&bytes);
        assert_eq!(
            reader.read_soft_object_subpath("Ansi").unwrap(),
            "EventGraph"
        );
        assert_eq!(reader.read_soft_object_subpath("Wide").unwrap(), "猫");
        assert_eq!(reader.remaining(), 0);
    }

    #[test]
    fn reads_generic_tarray() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&3_i32.to_le_bytes());
        for value in [10_u16, 20, 30] {
            bytes.extend_from_slice(&value.to_le_bytes());
        }

        let values = Reader::new(&bytes)
            .read_tarray("Values", size_of::<u16>(), |reader, index| {
                reader.read_u16(&format!("Values[{index}]"))
            })
            .unwrap();
        assert_eq!(values, [10, 20, 30]);
    }

    #[test]
    fn rejects_negative_and_excessive_array_counts_before_allocation() {
        let limits = ArchiveLimits {
            max_array_elements: 2,
            ..ArchiveLimits::default()
        };

        let negative = (-1_i32).to_le_bytes();
        let error = Reader::with_limits(&negative, limits)
            .read_tarray::<u8>("Values", 1, |reader, _| reader.read_u8("Value"))
            .unwrap_err();
        assert_eq!(error.kind(), ArchiveErrorKind::InvalidCount);

        let excessive = 3_i32.to_le_bytes();
        let error = Reader::with_limits(&excessive, limits)
            .read_tarray::<u8>("Values", 1, |reader, _| reader.read_u8("Value"))
            .unwrap_err();
        assert_eq!(error.kind(), ArchiveErrorKind::InvalidCount);
    }

    #[test]
    fn rejects_array_minimum_allocation_over_limit() {
        let limits = ArchiveLimits {
            max_array_elements: 10,
            max_allocation_bytes: 7,
            ..ArchiveLimits::default()
        };
        let bytes = 2_i32.to_le_bytes();

        let error = Reader::with_limits(&bytes, limits)
            .read_tarray::<u32>("Values", 4, |reader, _| reader.read_u32("Value"))
            .unwrap_err();
        assert_eq!(error.kind(), ArchiveErrorKind::AllocationLimit);
    }
}
