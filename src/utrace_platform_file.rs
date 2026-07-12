//! Bounded aggregation for Unreal `PlatformFile` (FileChannel) activity.

use std::collections::HashMap;

use crate::utrace::{
    PlatformFileActivityKind, PlatformFileActivitySample, PlatformFileDashboard,
    PlatformFileSummary,
};

const MAX_FILES: usize = 4_096;
const MAX_ACTIVITY_SAMPLES: usize = 40;
const MAX_OPEN_HANDLES: usize = 65_536;
const MAX_ACTIVE_OPS: usize = 65_536;
const INVALID_FILE_HANDLE: u64 = u64::MAX;

#[derive(Clone, Debug, Default)]
struct FileRec {
    path: String,
    opens: u64,
    open_failures: u64,
    reopens: u64,
    closes: u64,
    reads: u64,
    writes: u64,
    bytes_read: u64,
    bytes_written: u64,
    bytes_requested_read: u64,
    bytes_requested_write: u64,
}

#[derive(Clone, Copy, Debug)]
struct PendingOpen {
    file_index: u32,
    sample_index: Option<usize>,
    start_cycle: u64,
}

#[derive(Clone, Copy, Debug)]
struct PendingClose {
    file_index: u32,
    sample_index: Option<usize>,
    start_cycle: u64,
    file_handle: u64,
}

#[derive(Clone, Copy, Debug)]
struct ActiveOp {
    file_index: u32,
    sample_index: Option<usize>,
    start_cycle: u64,
}

#[derive(Clone, Copy, Debug)]
struct SampleStart {
    kind: PlatformFileActivityKind,
    file_index: Option<u32>,
    thread_id: u16,
    file_handle: Option<u64>,
    op_handle: Option<u64>,
    offset: Option<u64>,
    size: Option<u64>,
    start_cycle: u64,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct PlatformFileProvider {
    files: Vec<FileRec>,
    path_to_index: HashMap<String, u32>,
    unknown_file_index: Option<u32>,
    file_overflow: u64,
    open_handles: HashMap<u64, u32>,
    open_handle_overflow: u64,
    pending_opens: HashMap<u16, PendingOpen>,
    pending_reopens: HashMap<u16, PendingOpen>,
    pending_closes: HashMap<u16, PendingClose>,
    active_reads: HashMap<u64, ActiveOp>,
    active_writes: HashMap<u64, ActiveOp>,
    active_op_overflow: u64,
    opens: u64,
    open_failures: u64,
    reopens: u64,
    closes: u64,
    reads: u64,
    writes: u64,
    bytes_read: u64,
    bytes_written: u64,
    bytes_requested_read: u64,
    bytes_requested_write: u64,
    unpaired_ends: u64,
    unknown_handle_ops: u64,
    activity_samples: Vec<PlatformFileActivitySample>,
    activity_sample_overflow: u64,
}

impl PlatformFileProvider {
    pub(crate) fn begin_open(&mut self, thread_id: u16, cycle: u64, path: String) {
        let file_index = self.file_index_for_path(path);
        let sample_index = self.start_sample(SampleStart {
            kind: PlatformFileActivityKind::Open,
            file_index: Some(file_index),
            thread_id,
            file_handle: None,
            op_handle: None,
            offset: None,
            size: None,
            start_cycle: cycle,
        });
        self.pending_opens.insert(
            thread_id,
            PendingOpen {
                file_index,
                sample_index,
                start_cycle: cycle,
            },
        );
    }

    pub(crate) fn end_open(&mut self, thread_id: u16, cycle: u64, file_handle: u64) {
        let Some(pending) = self.pending_opens.remove(&thread_id) else {
            self.unpaired_ends = self.unpaired_ends.saturating_add(1);
            return;
        };
        let failed = file_handle == INVALID_FILE_HANDLE;
        if failed {
            self.opens = self.opens.saturating_add(1);
            self.open_failures = self.open_failures.saturating_add(1);
            if let Some(file) = self.files.get_mut(pending.file_index as usize) {
                file.opens = file.opens.saturating_add(1);
                file.open_failures = file.open_failures.saturating_add(1);
            }
        } else {
            self.opens = self.opens.saturating_add(1);
            if let Some(file) = self.files.get_mut(pending.file_index as usize) {
                file.opens = file.opens.saturating_add(1);
            }
            self.bind_open_handle(file_handle, pending.file_index);
        }
        self.finish_sample(
            pending.sample_index,
            cycle,
            pending.start_cycle,
            Some(file_handle),
            None,
            failed,
        );
    }

    pub(crate) fn begin_reopen(&mut self, thread_id: u16, cycle: u64, old_file_handle: u64) {
        let file_index = self.file_index_for_handle(old_file_handle);
        let sample_index = self.start_sample(SampleStart {
            kind: PlatformFileActivityKind::ReOpen,
            file_index: Some(file_index),
            thread_id,
            file_handle: Some(old_file_handle),
            op_handle: None,
            offset: None,
            size: None,
            start_cycle: cycle,
        });
        self.pending_reopens.insert(
            thread_id,
            PendingOpen {
                file_index,
                sample_index,
                start_cycle: cycle,
            },
        );
    }

    pub(crate) fn end_reopen(&mut self, thread_id: u16, cycle: u64, new_file_handle: u64) {
        let Some(pending) = self.pending_reopens.remove(&thread_id) else {
            self.unpaired_ends = self.unpaired_ends.saturating_add(1);
            return;
        };
        let failed = new_file_handle == INVALID_FILE_HANDLE;
        if failed {
            self.reopens = self.reopens.saturating_add(1);
            if let Some(file) = self.files.get_mut(pending.file_index as usize) {
                file.reopens = file.reopens.saturating_add(1);
                file.open_failures = file.open_failures.saturating_add(1);
            }
            self.open_failures = self.open_failures.saturating_add(1);
        } else {
            self.reopens = self.reopens.saturating_add(1);
            if let Some(file) = self.files.get_mut(pending.file_index as usize) {
                file.reopens = file.reopens.saturating_add(1);
            }
            self.bind_open_handle(new_file_handle, pending.file_index);
        }
        self.finish_sample(
            pending.sample_index,
            cycle,
            pending.start_cycle,
            Some(new_file_handle),
            None,
            failed,
        );
    }

    pub(crate) fn begin_close(&mut self, thread_id: u16, cycle: u64, file_handle: u64) {
        let file_index = match self.open_handles.remove(&file_handle) {
            Some(index) => index,
            None => {
                self.unknown_handle_ops = self.unknown_handle_ops.saturating_add(1);
                self.unknown_file_index()
            }
        };
        let sample_index = self.start_sample(SampleStart {
            kind: PlatformFileActivityKind::Close,
            file_index: Some(file_index),
            thread_id,
            file_handle: Some(file_handle),
            op_handle: None,
            offset: None,
            size: None,
            start_cycle: cycle,
        });
        self.pending_closes.insert(
            thread_id,
            PendingClose {
                file_index,
                sample_index,
                start_cycle: cycle,
                file_handle,
            },
        );
    }

    pub(crate) fn end_close(&mut self, thread_id: u16, cycle: u64) {
        let Some(pending) = self.pending_closes.remove(&thread_id) else {
            self.unpaired_ends = self.unpaired_ends.saturating_add(1);
            return;
        };
        self.closes = self.closes.saturating_add(1);
        if let Some(file) = self.files.get_mut(pending.file_index as usize) {
            file.closes = file.closes.saturating_add(1);
        }
        self.finish_sample(
            pending.sample_index,
            cycle,
            pending.start_cycle,
            Some(pending.file_handle),
            None,
            false,
        );
    }

    pub(crate) fn begin_read(
        &mut self,
        thread_id: u16,
        cycle: u64,
        read_handle: u64,
        file_handle: u64,
        offset: u64,
        size: u64,
    ) {
        let file_index = self.file_index_for_handle_or_bind(file_handle);
        if let Some(file) = self.files.get_mut(file_index as usize) {
            file.reads = file.reads.saturating_add(1);
            file.bytes_requested_read = file.bytes_requested_read.saturating_add(size);
        }
        self.reads = self.reads.saturating_add(1);
        self.bytes_requested_read = self.bytes_requested_read.saturating_add(size);
        let sample_index = self.start_sample(SampleStart {
            kind: PlatformFileActivityKind::Read,
            file_index: Some(file_index),
            thread_id,
            file_handle: Some(file_handle),
            op_handle: Some(read_handle),
            offset: Some(offset),
            size: Some(size),
            start_cycle: cycle,
        });
        if self.active_reads.len() >= MAX_ACTIVE_OPS
            && !self.active_reads.contains_key(&read_handle)
        {
            self.active_op_overflow = self.active_op_overflow.saturating_add(1);
        } else {
            self.active_reads.insert(
                read_handle,
                ActiveOp {
                    file_index,
                    sample_index,
                    start_cycle: cycle,
                },
            );
        }
    }

    pub(crate) fn end_read(&mut self, cycle: u64, read_handle: u64, size_read: u64) {
        let Some(active) = self.active_reads.remove(&read_handle) else {
            self.unpaired_ends = self.unpaired_ends.saturating_add(1);
            return;
        };
        self.bytes_read = self.bytes_read.saturating_add(size_read);
        if let Some(file) = self.files.get_mut(active.file_index as usize) {
            file.bytes_read = file.bytes_read.saturating_add(size_read);
        }
        self.finish_sample(
            active.sample_index,
            cycle,
            active.start_cycle,
            None,
            Some(size_read),
            false,
        );
    }

    pub(crate) fn begin_write(
        &mut self,
        thread_id: u16,
        cycle: u64,
        write_handle: u64,
        file_handle: u64,
        offset: u64,
        size: u64,
    ) {
        let file_index = self.file_index_for_handle_or_bind(file_handle);
        if let Some(file) = self.files.get_mut(file_index as usize) {
            file.writes = file.writes.saturating_add(1);
            file.bytes_requested_write = file.bytes_requested_write.saturating_add(size);
        }
        self.writes = self.writes.saturating_add(1);
        self.bytes_requested_write = self.bytes_requested_write.saturating_add(size);
        let sample_index = self.start_sample(SampleStart {
            kind: PlatformFileActivityKind::Write,
            file_index: Some(file_index),
            thread_id,
            file_handle: Some(file_handle),
            op_handle: Some(write_handle),
            offset: Some(offset),
            size: Some(size),
            start_cycle: cycle,
        });
        if self.active_writes.len() >= MAX_ACTIVE_OPS
            && !self.active_writes.contains_key(&write_handle)
        {
            self.active_op_overflow = self.active_op_overflow.saturating_add(1);
        } else {
            self.active_writes.insert(
                write_handle,
                ActiveOp {
                    file_index,
                    sample_index,
                    start_cycle: cycle,
                },
            );
        }
    }

    pub(crate) fn end_write(&mut self, cycle: u64, write_handle: u64, size_written: u64) {
        let Some(active) = self.active_writes.remove(&write_handle) else {
            self.unpaired_ends = self.unpaired_ends.saturating_add(1);
            return;
        };
        self.bytes_written = self.bytes_written.saturating_add(size_written);
        if let Some(file) = self.files.get_mut(active.file_index as usize) {
            file.bytes_written = file.bytes_written.saturating_add(size_written);
        }
        self.finish_sample(
            active.sample_index,
            cycle,
            active.start_cycle,
            None,
            Some(size_written),
            false,
        );
    }

    pub(crate) fn dashboard(self) -> PlatformFileDashboard {
        let mut files = self
            .files
            .into_iter()
            .map(|file| PlatformFileSummary {
                path: file.path,
                opens: file.opens,
                open_failures: file.open_failures,
                reopens: file.reopens,
                closes: file.closes,
                reads: file.reads,
                writes: file.writes,
                bytes_read: file.bytes_read,
                bytes_written: file.bytes_written,
                bytes_requested_read: file.bytes_requested_read,
                bytes_requested_write: file.bytes_requested_write,
            })
            .collect::<Vec<_>>();
        files.sort_by(|left, right| {
            let left_ops = left
                .reads
                .saturating_add(left.writes)
                .saturating_add(left.opens);
            let right_ops = right
                .reads
                .saturating_add(right.writes)
                .saturating_add(right.opens);
            right_ops
                .cmp(&left_ops)
                .then_with(|| right.bytes_read.cmp(&left.bytes_read))
                .then_with(|| left.path.cmp(&right.path))
        });
        let file_count = u64::try_from(files.len()).unwrap_or(u64::MAX);
        files.truncate(64);

        PlatformFileDashboard {
            file_count,
            file_overflow: self.file_overflow,
            files,
            opens: self.opens,
            open_failures: self.open_failures,
            reopens: self.reopens,
            closes: self.closes,
            reads: self.reads,
            writes: self.writes,
            bytes_read: self.bytes_read,
            bytes_written: self.bytes_written,
            bytes_requested_read: self.bytes_requested_read,
            bytes_requested_write: self.bytes_requested_write,
            unpaired_ends: self.unpaired_ends,
            unknown_handle_ops: self.unknown_handle_ops,
            open_handle_overflow: self.open_handle_overflow,
            active_op_overflow: self.active_op_overflow,
            activity_sample_overflow: self.activity_sample_overflow,
            activity_samples: self.activity_samples,
        }
    }

    fn file_index_for_path(&mut self, path: String) -> u32 {
        if let Some(index) = self.path_to_index.get(&path).copied() {
            return index;
        }
        let reserved_slots = usize::from(self.unknown_file_index.is_none());
        if self.files.len() >= MAX_FILES.saturating_sub(reserved_slots) {
            self.file_overflow = self.file_overflow.saturating_add(1);
            return self.unknown_file_index();
        }
        let index = u32::try_from(self.files.len()).expect("file catalog within u32");
        self.path_to_index.insert(path.clone(), index);
        self.files.push(FileRec {
            path,
            ..FileRec::default()
        });
        index
    }

    fn unknown_file_index(&mut self) -> u32 {
        if let Some(index) = self.unknown_file_index {
            return index;
        }
        debug_assert!(
            self.files.len() < MAX_FILES,
            "unknown slot must be reserved"
        );
        let index = u32::try_from(self.files.len()).expect("file catalog within u32");
        self.unknown_file_index = Some(index);
        self.files.push(FileRec {
            path: String::new(),
            ..FileRec::default()
        });
        index
    }

    fn file_index_for_handle(&mut self, file_handle: u64) -> u32 {
        if let Some(index) = self.open_handles.get(&file_handle).copied() {
            return index;
        }
        self.unknown_handle_ops = self.unknown_handle_ops.saturating_add(1);
        self.unknown_file_index()
    }

    fn file_index_for_handle_or_bind(&mut self, file_handle: u64) -> u32 {
        if let Some(index) = self.open_handles.get(&file_handle).copied() {
            return index;
        }
        self.unknown_handle_ops = self.unknown_handle_ops.saturating_add(1);
        let index = self.unknown_file_index();
        self.bind_open_handle(file_handle, index);
        index
    }

    fn bind_open_handle(&mut self, file_handle: u64, file_index: u32) {
        if self.open_handles.len() >= MAX_OPEN_HANDLES
            && !self.open_handles.contains_key(&file_handle)
        {
            self.open_handle_overflow = self.open_handle_overflow.saturating_add(1);
            return;
        }
        self.open_handles.insert(file_handle, file_index);
    }

    fn start_sample(&mut self, start: SampleStart) -> Option<usize> {
        if self.activity_samples.len() >= MAX_ACTIVITY_SAMPLES {
            self.activity_sample_overflow = self.activity_sample_overflow.saturating_add(1);
            return None;
        }
        let path = start
            .file_index
            .and_then(|index| self.files.get(index as usize).map(|f| f.path.clone()));
        let index = self.activity_samples.len();
        self.activity_samples.push(PlatformFileActivitySample {
            kind: start.kind,
            path,
            thread_id: start.thread_id,
            file_handle: start.file_handle,
            op_handle: start.op_handle,
            offset: start.offset,
            size: start.size,
            actual_size: None,
            start_cycle: start.start_cycle,
            end_cycle: None,
            duration_cycles: None,
            failed: false,
        });
        Some(index)
    }

    fn finish_sample(
        &mut self,
        sample_index: Option<usize>,
        end_cycle: u64,
        start_cycle: u64,
        file_handle: Option<u64>,
        actual_size: Option<u64>,
        failed: bool,
    ) {
        let Some(index) = sample_index else {
            return;
        };
        let Some(sample) = self.activity_samples.get_mut(index) else {
            return;
        };
        sample.end_cycle = Some(end_cycle);
        sample.duration_cycles = Some(end_cycle.saturating_sub(start_cycle));
        sample.failed = failed;
        if let Some(file_handle) = file_handle {
            sample.file_handle = Some(file_handle);
        }
        if let Some(actual_size) = actual_size {
            sample.actual_size = Some(actual_size);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pairs_open_read_close_and_failed_open() {
        let mut provider = PlatformFileProvider::default();
        provider.begin_open(7, 10, "/Game/A.uasset".to_owned());
        provider.end_open(7, 20, 0x100);
        provider.begin_read(7, 30, 0x200, 0x100, 0, 64);
        provider.end_read(40, 0x200, 64);
        provider.begin_close(7, 50, 0x100);
        provider.end_close(7, 55);

        provider.begin_open(8, 60, "/Game/Missing.uasset".to_owned());
        provider.end_open(8, 70, INVALID_FILE_HANDLE);

        let dashboard = provider.dashboard();
        assert_eq!(dashboard.opens, 2);
        assert_eq!(dashboard.open_failures, 1);
        assert_eq!(dashboard.reads, 1);
        assert_eq!(dashboard.closes, 1);
        assert_eq!(dashboard.bytes_read, 64);
        assert_eq!(dashboard.bytes_requested_read, 64);
        assert_eq!(dashboard.file_count, 2);
        assert!(
            dashboard
                .files
                .iter()
                .any(|file| file.path == "/Game/A.uasset"
                    && file.reads == 1
                    && file.bytes_read == 64)
        );
        assert!(
            dashboard
                .files
                .iter()
                .any(|file| file.path == "/Game/Missing.uasset" && file.open_failures == 1)
        );
        assert_eq!(dashboard.activity_samples.len(), 4);
        assert_eq!(
            dashboard.activity_samples[0].kind,
            PlatformFileActivityKind::Open
        );
        assert_eq!(dashboard.activity_samples[0].duration_cycles, Some(10));
        assert_eq!(
            dashboard.activity_samples[1].kind,
            PlatformFileActivityKind::Read
        );
        assert_eq!(dashboard.activity_samples[1].actual_size, Some(64));
        assert_eq!(
            dashboard.activity_samples[2].kind,
            PlatformFileActivityKind::Close
        );
        assert!(dashboard.activity_samples[3].failed);
        assert_eq!(
            dashboard.activity_samples[3].path.as_deref(),
            Some("/Game/Missing.uasset")
        );
    }

    #[test]
    fn counts_unpaired_ends_and_unknown_handles() {
        let mut provider = PlatformFileProvider::default();
        provider.end_read(10, 0x99, 8);
        provider.begin_read(1, 20, 0x1, 0x42, 0, 16);
        provider.end_read(30, 0x1, 16);
        let dashboard = provider.dashboard();
        assert_eq!(dashboard.unpaired_ends, 1);
        assert_eq!(dashboard.unknown_handle_ops, 1);
        assert_eq!(dashboard.reads, 1);
        assert_eq!(dashboard.bytes_read, 16);
    }

    #[test]
    fn caps_activity_samples() {
        let mut provider = PlatformFileProvider::default();
        for index in 0..(MAX_ACTIVITY_SAMPLES as u64 + 3) {
            let handle = 0x1000 + index;
            provider.begin_open(1, index * 10, format!("/tmp/{index}"));
            provider.end_open(1, index * 10 + 1, handle);
        }
        let dashboard = provider.dashboard();
        assert_eq!(dashboard.activity_samples.len(), MAX_ACTIVITY_SAMPLES);
        assert_eq!(dashboard.activity_sample_overflow, 3);
        assert_eq!(dashboard.opens, MAX_ACTIVITY_SAMPLES as u64 + 3);
    }

    #[test]
    fn full_file_catalog_uses_a_reserved_unknown_bucket() {
        let mut provider = PlatformFileProvider::default();
        for index in 0..MAX_FILES {
            provider.file_index_for_path(format!("/known/{index}"));
        }

        let unknown = provider.file_index_for_handle(0xdead);
        assert_ne!(
            unknown, 0,
            "overflow activity must not alias the first file"
        );
        assert_eq!(provider.files[unknown as usize].path, "");
        assert_eq!(provider.files.len(), MAX_FILES);
        assert_eq!(provider.file_overflow, 1);
    }
}
