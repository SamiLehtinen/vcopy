use std::fs::File;
use std::io::{self, Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::sync::Arc;
use std::thread::{self, JoinHandle};

#[cfg(target_os = "linux")]
use crate::util::FADVISE_WINDOW_BYTES;

/// Sink abstraction for the decoupled copy pipeline.
///
/// Production code uses `std::fs::File`; tests use fault-injecting mocks that
/// exercise the exact same state machine (`write_all`, `advise_dontneed`, `set_len`).
pub(crate) trait PipelineSink: Write + Send + 'static {
    #[cfg(target_os = "linux")]
    fn advise_dontneed(&self, _offset: u64, _len: u64) {}

    fn set_len(&mut self, _len: u64) -> io::Result<()> {
        Ok(())
    }
}

impl PipelineSink for File {
    #[cfg(target_os = "linux")]
    fn advise_dontneed(&self, offset: u64, len: u64) {
        crate::util::advise_dontneed(self, offset, len);
    }

    fn set_len(&mut self, len: u64) -> io::Result<()> {
        File::set_len(self, len)
    }
}

/// Source abstraction for the decoupled copy pipeline.
pub(crate) trait PipelineSource: Read {
    #[cfg(target_os = "linux")]
    fn advise_dontneed(&self, _offset: u64, _len: u64) {}
}

impl PipelineSource for File {
    #[cfg(target_os = "linux")]
    fn advise_dontneed(&self, offset: u64, len: u64) {
        crate::util::advise_dontneed(self, offset, len);
    }
}

enum WriterMsg<W> {
    Start {
        dst: W,
        src_size: u64,
        preallocated: bool,
    },
    Chunk {
        buf: Vec<u8>,
        len: usize,
    },
    Finish,
    Shutdown,
}

enum WriterState<W> {
    Idle,
    Active {
        dst: W,
        src_size: u64,
        preallocated: bool,
        written: u64,
        #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
        dropped: u64,
        write_err: Option<io::Error>,
    },
}

/// Persistent single-writer 3-slot ring-buffer copy pipeline.
///
/// Design invariants:
/// - **Single-writer serialization:** Files are processed one at a time behind a
///   global mutex in `util.rs`, avoiding HDD/USB/SMR multi-stream seek thrashing.
/// - **Deadlock-free buffer ownership:** `free_pool` lives solely on the Reader,
///   while the Writer holds the sole `return_tx` sender. If the Writer unwinds on
///   panic, `return_tx` and `done_tx` are dropped immediately so every Reader
///   channel operation unblocks with `Disconnected` instead of hanging.
/// - **Close-before-error:** On `write_err`, the Writer drops `dst` before
///   sending `Err` on `done_tx`; on `read_err`, the Reader drops `dst` before
///   returning `Err`. Destination handles are therefore always closed before
///   `copy_atomic` unlinks `.tmp` staging files.
pub(crate) struct CopyPipeline<W: PipelineSink = File> {
    work_tx: Option<SyncSender<WriterMsg<W>>>,
    return_rx: Receiver<Vec<u8>>,
    done_rx: Receiver<io::Result<(W, u64)>>,
    free_pool: Vec<Vec<u8>>,
    abort: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
    ring_slots: usize,
    dead: bool,
}

impl<W: PipelineSink> CopyPipeline<W> {
    pub(crate) fn try_new(block_size: usize, ring_slots: usize) -> io::Result<Self> {
        assert!(block_size > 0, "block_size must be > 0");
        assert!(ring_slots > 0, "ring_slots must be > 0");

        let (work_tx, work_rx) = mpsc::sync_channel::<WriterMsg<W>>(ring_slots + 2);
        let (return_tx, return_rx) = mpsc::sync_channel::<Vec<u8>>(ring_slots);
        let (done_tx, done_rx) = mpsc::sync_channel::<io::Result<(W, u64)>>(1);
        let abort = Arc::new(AtomicBool::new(false));
        let worker_abort = Arc::clone(&abort);

        let handle = thread::Builder::new()
            .name("vcopy-writer".to_string())
            .spawn(move || {
                writer_loop(work_rx, return_tx, done_tx, worker_abort);
            })?;

        let mut free_pool = Vec::with_capacity(ring_slots);
        for _ in 0..ring_slots {
            free_pool.push(vec![0u8; block_size]);
        }

        Ok(Self {
            work_tx: Some(work_tx),
            return_rx,
            done_rx,
            free_pool,
            abort,
            handle: Some(handle),
            ring_slots,
            dead: false,
        })
    }

    pub(crate) fn is_dead(&self) -> bool {
        self.dead
    }

    #[cfg(test)]
    pub(crate) fn free_pool_len(&self) -> usize {
        self.free_pool.len()
    }

    pub(crate) fn copy_file<R: PipelineSource + ?Sized>(
        &mut self,
        reader: &mut R,
        dst: W,
        src_size: u64,
        preallocated: bool,
    ) -> io::Result<(W, u64)> {
        if self.dead {
            return Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "copy pipeline writer is no longer running",
            ));
        }

        // Drain any leftover returned buffers defensively before starting.
        while let Ok(buf) = self.return_rx.try_recv() {
            self.free_pool.push(buf);
        }

        self.abort.store(false, Ordering::Relaxed);
        let work_tx = match self.work_tx.as_ref() {
            Some(tx) => tx,
            None => {
                self.dead = true;
                return Err(io::Error::new(
                    io::ErrorKind::BrokenPipe,
                    "copy pipeline work channel closed",
                ));
            }
        };

        if work_tx
            .send(WriterMsg::Start {
                dst,
                src_size,
                preallocated,
            })
            .is_err()
        {
            self.dead = true;
            return Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "copy pipeline writer terminated before start",
            ));
        }

        let mut read_err: Option<io::Error> = None;
        let mut read_total: u64 = 0;
        #[cfg(target_os = "linux")]
        let mut read_dropped: u64 = 0;

        while !self.abort.load(Ordering::Relaxed) {
            let mut buf = if let Some(b) = self.free_pool.pop() {
                b
            } else {
                match self.return_rx.recv() {
                    Ok(b) => b,
                    Err(_) => {
                        self.dead = true;
                        read_err = Some(io::Error::new(
                            io::ErrorKind::BrokenPipe,
                            "copy pipeline writer terminated while returning buffer",
                        ));
                        break;
                    }
                }
            };

            let read_res = loop {
                match reader.read(&mut buf) {
                    Ok(n) => break Ok(n),
                    Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                    Err(e) => break Err(e),
                }
            };

            match read_res {
                Ok(0) => {
                    self.free_pool.push(buf);
                    break;
                }
                Ok(n) => {
                    read_total += n as u64;
                    #[cfg(target_os = "linux")]
                    while read_total - read_dropped >= FADVISE_WINDOW_BYTES {
                        reader.advise_dontneed(read_dropped, FADVISE_WINDOW_BYTES);
                        read_dropped += FADVISE_WINDOW_BYTES;
                    }
                    if let Err(mpsc::SendError(WriterMsg::Chunk { buf: returned, .. })) =
                        work_tx.send(WriterMsg::Chunk { buf, len: n })
                    {
                        self.free_pool.push(returned);
                        self.dead = true;
                        read_err = Some(io::Error::new(
                            io::ErrorKind::BrokenPipe,
                            "copy pipeline writer disconnected during chunk send",
                        ));
                        break;
                    }
                }
                Err(e) => {
                    self.free_pool.push(buf);
                    read_err = Some(e);
                    self.abort.store(true, Ordering::Relaxed);
                    break;
                }
            }
        }

        #[cfg(target_os = "linux")]
        if read_err.is_none() && read_total > read_dropped {
            reader.advise_dontneed(read_dropped, read_total - read_dropped);
        }
        #[cfg(not(target_os = "linux"))]
        let _ = read_total;

        if work_tx.send(WriterMsg::Finish).is_err() {
            self.dead = true;
        }

        let write_res = match self.done_rx.recv() {
            Ok(res) => res,
            Err(_) => {
                self.dead = true;
                Err(io::Error::new(
                    io::ErrorKind::BrokenPipe,
                    "copy pipeline writer terminated before completion",
                ))
            }
        };

        while let Ok(buf) = self.return_rx.try_recv() {
            self.free_pool.push(buf);
        }

        if !self.dead {
            debug_assert_eq!(
                self.free_pool.len(),
                self.ring_slots,
                "ring buffer slot conservation violated"
            );
        }

        if let Some(err) = read_err {
            // Explicitly drop any returned sink handle before returning Err so the
            // caller can unlink the staging file cleanly.
            drop(write_res);
            return Err(err);
        }

        write_res
    }
}

impl<W: PipelineSink> Drop for CopyPipeline<W> {
    fn drop(&mut self) {
        if let Some(tx) = self.work_tx.take() {
            let _ = tx.try_send(WriterMsg::Shutdown);
            drop(tx);
        }
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

fn writer_loop<W: PipelineSink>(
    work_rx: Receiver<WriterMsg<W>>,
    return_tx: SyncSender<Vec<u8>>,
    done_tx: SyncSender<io::Result<(W, u64)>>,
    abort: Arc<AtomicBool>,
) {
    let mut state = WriterState::Idle;

    while let Ok(msg) = work_rx.recv() {
        match msg {
            WriterMsg::Start {
                dst,
                src_size,
                preallocated,
            } => {
                state = WriterState::Active {
                    dst,
                    src_size,
                    preallocated,
                    written: 0,
                    dropped: 0,
                    write_err: None,
                };
            }
            WriterMsg::Chunk { buf, len } => {
                if let WriterState::Active {
                    dst,
                    written,
                    dropped,
                    write_err,
                    ..
                } = &mut state
                {
                    if write_err.is_none() && len > 0 && !abort.load(Ordering::Relaxed) {
                        match dst.write_all(&buf[..len]) {
                            Ok(()) => {
                                *written += len as u64;
                                #[cfg(target_os = "linux")]
                                while *written - *dropped >= FADVISE_WINDOW_BYTES {
                                    dst.advise_dontneed(*dropped, FADVISE_WINDOW_BYTES);
                                    *dropped += FADVISE_WINDOW_BYTES;
                                }
                                #[cfg(not(target_os = "linux"))]
                                let _ = dropped;
                            }
                            Err(e) => {
                                *write_err = Some(e);
                                abort.store(true, Ordering::Relaxed);
                            }
                        }
                    }
                }
                let _ = return_tx.send(buf);
            }
            WriterMsg::Finish => {
                let finished = std::mem::replace(&mut state, WriterState::Idle);
                if let WriterState::Active {
                    mut dst,
                    src_size,
                    preallocated,
                    written,
                    dropped,
                    mut write_err,
                } = finished
                {
                    if write_err.is_none() && !abort.load(Ordering::Relaxed) {
                        if preallocated && written < src_size {
                            if let Err(e) = dst.set_len(written) {
                                write_err = Some(e);
                            }
                        }
                        #[cfg(target_os = "linux")]
                        if write_err.is_none() && written > dropped {
                            dst.advise_dontneed(dropped, written - dropped);
                        }
                        #[cfg(not(target_os = "linux"))]
                        let _ = dropped;
                    }

                    let result = match write_err {
                        Some(e) => {
                            drop(dst);
                            Err(e)
                        }
                        None => Ok((dst, written)),
                    };
                    if done_tx.send(result).is_err() {
                        break;
                    }
                }
            }
            WriterMsg::Shutdown => break,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    fn pattern_bytes(len: usize) -> Vec<u8> {
        let mut data = vec![0u8; len];
        for (i, b) in data.iter_mut().enumerate() {
            *b = ((i.wrapping_mul(31) ^ (i >> 8)) & 0xff) as u8;
        }
        data
    }

    struct MockSource {
        data: Vec<u8>,
        pos: usize,
        max_read: usize,
        eintr_every: usize,
        fail_after_reads: Option<usize>,
        reads: usize,
    }

    impl MockSource {
        fn new(data: Vec<u8>) -> Self {
            Self {
                data,
                pos: 0,
                max_read: usize::MAX,
                eintr_every: 0,
                fail_after_reads: None,
                reads: 0,
            }
        }
    }

    impl Read for MockSource {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            self.reads += 1;
            if self.eintr_every > 0 && self.reads.is_multiple_of(self.eintr_every) {
                return Err(io::Error::new(io::ErrorKind::Interrupted, "mock eintr"));
            }
            if let Some(limit) = self.fail_after_reads {
                if self.reads >= limit {
                    return Err(io::Error::other("mock read failure"));
                }
            }
            if self.pos >= self.data.len() {
                return Ok(0);
            }
            let n = (self.data.len() - self.pos)
                .min(buf.len())
                .min(self.max_read);
            buf[..n].copy_from_slice(&self.data[self.pos..self.pos + n]);
            self.pos += n;
            Ok(n)
        }
    }

    impl PipelineSource for MockSource {}

    struct MockSink {
        out: Vec<u8>,
        max_write: usize,
        eintr_every: usize,
        fail_after_writes: Option<usize>,
        panic_after_writes: Option<usize>,
        fail_set_len: bool,
        set_len_calls: Arc<AtomicUsize>,
        writes: usize,
    }

    impl MockSink {
        fn new() -> Self {
            Self {
                out: Vec::new(),
                max_write: usize::MAX,
                eintr_every: 0,
                fail_after_writes: None,
                panic_after_writes: None,
                fail_set_len: false,
                set_len_calls: Arc::new(AtomicUsize::new(0)),
                writes: 0,
            }
        }
    }

    impl Write for MockSink {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.writes += 1;
            if let Some(p) = self.panic_after_writes {
                if self.writes >= p {
                    panic!("intentional mock writer panic");
                }
            }
            if self.eintr_every > 0 && self.writes.is_multiple_of(self.eintr_every) {
                return Err(io::Error::new(
                    io::ErrorKind::Interrupted,
                    "mock write eintr",
                ));
            }
            if let Some(limit) = self.fail_after_writes {
                if self.writes >= limit {
                    return Err(io::Error::other("mock ENOSPC / EIO"));
                }
            }
            let n = buf.len().min(self.max_write);
            self.out.extend_from_slice(&buf[..n]);
            Ok(n)
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl PipelineSink for MockSink {
        fn set_len(&mut self, len: u64) -> io::Result<()> {
            self.set_len_calls.fetch_add(1, Ordering::SeqCst);
            if self.fail_set_len {
                return Err(io::Error::other("mock set_len failure"));
            }
            self.out.resize(len as usize, 0);
            Ok(())
        }
    }

    #[test]
    fn test_pipeline_boundary_sizes_and_slot_conservation() {
        const BLOCK: usize = 64 * 1024;
        let mut pipeline = CopyPipeline::<MockSink>::try_new(BLOCK, 3).unwrap();
        let sizes = [
            0,
            1,
            BLOCK - 1,
            BLOCK,
            BLOCK + 1,
            3 * BLOCK,
            3 * BLOCK + 17,
            10 * BLOCK + 123,
        ];

        for size in sizes {
            let data = pattern_bytes(size);
            let mut src = MockSource::new(data.clone());
            let dst = MockSink::new();
            let (out_sink, written) = pipeline
                .copy_file(&mut src, dst, size as u64, size > 0)
                .unwrap();
            assert_eq!(written, size as u64);
            assert_eq!(out_sink.out, data);
            assert_eq!(out_sink.set_len_calls.load(Ordering::SeqCst), 0);
            assert_eq!(pipeline.free_pool_len(), 3);
            assert!(!pipeline.is_dead());
        }
    }

    #[test]
    fn test_pipeline_irregular_short_reads_partial_writes_and_eintr() {
        const BLOCK: usize = 64 * 1024;
        let mut pipeline = CopyPipeline::<MockSink>::try_new(BLOCK, 3).unwrap();
        let data = pattern_bytes(4 * BLOCK + 519);

        for &read_chunk in &[13usize, 4097usize] {
            let mut src = MockSource::new(data.clone());
            src.max_read = read_chunk;
            src.eintr_every = 7;

            let mut dst = MockSink::new();
            dst.max_write = 313;
            dst.eintr_every = 11;

            let (out_sink, written) = pipeline
                .copy_file(&mut src, dst, data.len() as u64, true)
                .unwrap();
            assert_eq!(written, data.len() as u64);
            assert_eq!(out_sink.out, data);
            assert_eq!(pipeline.free_pool_len(), 3);
        }
    }

    #[test]
    fn test_pipeline_fault_injection_and_post_error_reuse() {
        const BLOCK: usize = 64 * 1024;
        let mut pipeline = CopyPipeline::<MockSink>::try_new(BLOCK, 3).unwrap();
        let data = pattern_bytes(10 * BLOCK + 77);

        for &fail_at in &[1usize, 2usize, 8usize] {
            // Writer failure at exact write call
            let mut src = MockSource::new(data.clone());
            let mut dst = MockSink::new();
            dst.fail_after_writes = Some(fail_at);
            let err = pipeline
                .copy_file(&mut src, dst, data.len() as u64, true)
                .err()
                .expect("writer failure must return Err");
            assert_eq!(err.kind(), io::ErrorKind::Other);
            assert_eq!(pipeline.free_pool_len(), 3);
            assert!(!pipeline.is_dead());

            // Immediate valid copy on the same pipeline must succeed
            let mut good_src = MockSource::new(data.clone());
            let good_dst = MockSink::new();
            let (out_sink, written) = pipeline
                .copy_file(&mut good_src, good_dst, data.len() as u64, true)
                .unwrap();
            assert_eq!(written, data.len() as u64);
            assert_eq!(out_sink.out, data);
            assert_eq!(pipeline.free_pool_len(), 3);

            // Reader failure at exact read call
            let mut bad_src = MockSource::new(data.clone());
            bad_src.fail_after_reads = Some(fail_at);
            let good_dst = MockSink::new();
            let err = pipeline
                .copy_file(&mut bad_src, good_dst, data.len() as u64, true)
                .err()
                .expect("reader failure must return Err");
            assert_eq!(err.kind(), io::ErrorKind::Other);
            assert_eq!(pipeline.free_pool_len(), 3);
            assert!(!pipeline.is_dead());

            // Immediate valid copy on the same pipeline must succeed
            let mut good_src2 = MockSource::new(data.clone());
            let good_dst2 = MockSink::new();
            let (out_sink2, written2) = pipeline
                .copy_file(&mut good_src2, good_dst2, data.len() as u64, true)
                .unwrap();
            assert_eq!(written2, data.len() as u64);
            assert_eq!(out_sink2.out, data);
            assert_eq!(pipeline.free_pool_len(), 3);
        }
    }

    #[test]
    fn test_pipeline_writer_panic_recovery_no_deadlock() {
        const BLOCK: usize = 64 * 1024;
        let mut pipeline = CopyPipeline::<MockSink>::try_new(BLOCK, 3).unwrap();
        let data = pattern_bytes(6 * BLOCK + 11);

        let mut src = MockSource::new(data.clone());
        let mut dst = MockSink::new();
        dst.panic_after_writes = Some(2);

        let err = pipeline
            .copy_file(&mut src, dst, data.len() as u64, true)
            .err()
            .expect("writer panic must unblock reader and return Err");
        assert_eq!(err.kind(), io::ErrorKind::BrokenPipe);
        assert!(pipeline.is_dead());

        // Subsequent calls on dead pipeline must fail fast without hanging
        let mut src2 = MockSource::new(data);
        let dst2 = MockSink::new();
        let err2 = pipeline.copy_file(&mut src2, dst2, 0, false).err().unwrap();
        assert_eq!(err2.kind(), io::ErrorKind::BrokenPipe);
    }

    #[test]
    fn test_pipeline_shrink_trim_and_set_len_error() {
        const BLOCK: usize = 64 * 1024;
        let mut pipeline = CopyPipeline::<MockSink>::try_new(BLOCK, 3).unwrap();
        let data = pattern_bytes(2 * BLOCK + 333);

        // Preallocated to larger src_size -> set_len called once with actual bytes
        let mut src = MockSource::new(data.clone());
        let dst = MockSink::new();
        let calls = Arc::clone(&dst.set_len_calls);
        let (out_sink, written) = pipeline
            .copy_file(&mut src, dst, (5 * BLOCK) as u64, true)
            .unwrap();
        assert_eq!(written, data.len() as u64);
        assert_eq!(out_sink.out, data);
        assert_eq!(calls.load(Ordering::SeqCst), 1);

        // set_len error propagates cleanly
        let mut src2 = MockSource::new(data.clone());
        let mut dst2 = MockSink::new();
        dst2.fail_set_len = true;
        assert!(pipeline
            .copy_file(&mut src2, dst2, (5 * BLOCK) as u64, true)
            .is_err());
    }

    #[test]
    fn test_pipeline_50_sequential_mixed_copies() {
        const BLOCK: usize = 64 * 1024;
        let mut pipeline = CopyPipeline::<MockSink>::try_new(BLOCK, 3).unwrap();
        for i in 0..55usize {
            let size = (i * 19_381) % (5 * BLOCK + 999);
            let data = pattern_bytes(size);
            let mut src = MockSource::new(data.clone());
            let dst = MockSink::new();
            let (out_sink, written) = pipeline
                .copy_file(&mut src, dst, size as u64, size > 0)
                .unwrap();
            assert_eq!(written, size as u64);
            assert_eq!(out_sink.out, data);
            assert_eq!(pipeline.free_pool_len(), 3);
        }
    }

    #[test]
    fn test_pipeline_reader_advances_while_writer_busy() {
        use std::time::Duration;

        struct OverlapSource {
            data: Vec<u8>,
            pos: usize,
            reads: usize,
            writer_in_write1: Arc<AtomicBool>,
            overlap_observed: Arc<AtomicBool>,
            in_write1_rx: Option<Receiver<()>>,
            chunk2_tx: Option<mpsc::SyncSender<()>>,
        }

        impl Read for OverlapSource {
            fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
                if self.pos >= self.data.len() {
                    return Ok(0);
                }
                self.reads += 1;
                if self.reads == 2 {
                    if let Some(rx) = self.in_write1_rx.take() {
                        rx.recv_timeout(Duration::from_secs(3))
                            .expect("writer never entered write #1 while reader advanced");
                    }
                    if self.writer_in_write1.load(Ordering::SeqCst) {
                        self.overlap_observed.store(true, Ordering::SeqCst);
                    }
                    if let Some(tx) = self.chunk2_tx.take() {
                        let _ = tx.send(());
                    }
                }
                let n = (self.data.len() - self.pos).min(buf.len());
                buf[..n].copy_from_slice(&self.data[self.pos..self.pos + n]);
                self.pos += n;
                Ok(n)
            }
        }

        impl PipelineSource for OverlapSource {}

        struct OverlapSink {
            out: Vec<u8>,
            writes: usize,
            writer_in_write1: Arc<AtomicBool>,
            in_write1_tx: Option<mpsc::SyncSender<()>>,
            chunk2_rx: Option<Receiver<()>>,
        }

        impl Write for OverlapSink {
            fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
                self.writes += 1;
                if self.writes == 1 {
                    self.writer_in_write1.store(true, Ordering::SeqCst);
                    if let Some(tx) = self.in_write1_tx.take() {
                        let _ = tx.send(());
                    }
                    // Wait for Reader to read chunk 2 *while* Writer is still in write #1.
                    // Sequential execution would time out here; decoupled pipeline unblocks immediately.
                    if let Some(rx) = self.chunk2_rx.take() {
                        rx.recv_timeout(Duration::from_secs(3))
                            .expect("reader did not advance to chunk 2 while writer was busy");
                    }
                    self.writer_in_write1.store(false, Ordering::SeqCst);
                }
                self.out.extend_from_slice(buf);
                Ok(buf.len())
            }

            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }

        impl PipelineSink for OverlapSink {}

        const BLOCK: usize = 64 * 1024;
        let mut pipeline = CopyPipeline::<OverlapSink>::try_new(BLOCK, 3).unwrap();
        let data = pattern_bytes(4 * BLOCK + 123);

        let writer_in_write1 = Arc::new(AtomicBool::new(false));
        let overlap_observed = Arc::new(AtomicBool::new(false));
        let (in_write1_tx, in_write1_rx) = mpsc::sync_channel(1);
        let (chunk2_tx, chunk2_rx) = mpsc::sync_channel(1);

        let mut src = OverlapSource {
            data: data.clone(),
            pos: 0,
            reads: 0,
            writer_in_write1: Arc::clone(&writer_in_write1),
            overlap_observed: Arc::clone(&overlap_observed),
            in_write1_rx: Some(in_write1_rx),
            chunk2_tx: Some(chunk2_tx),
        };
        let dst = OverlapSink {
            out: Vec::new(),
            writes: 0,
            writer_in_write1,
            in_write1_tx: Some(in_write1_tx),
            chunk2_rx: Some(chunk2_rx),
        };

        let (out_sink, written) = pipeline
            .copy_file(&mut src, dst, data.len() as u64, false)
            .unwrap();
        assert!(overlap_observed.load(Ordering::SeqCst));
        assert_eq!(written, data.len() as u64);
        assert_eq!(out_sink.out, data);
        assert_eq!(pipeline.free_pool_len(), 3);
    }
}
