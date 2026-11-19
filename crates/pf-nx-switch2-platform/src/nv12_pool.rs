//! Recycle CPU-side packed NV12/P010 staging allocations across presented frames.
//!
//! The SDK decoder owns its output planes. A frame must keep an independent,
//! fully copied buffer until the display has finished uploading it. This pool
//! only reuses the allocations *after* the frame is dropped; it does not skip
//! that ownership boundary or hand a decoder-owned pointer to Ganesh.
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

const MAX_CACHED_BUFFERS: usize = 2;
const MAX_CACHED_BYTES: usize = 32 * 1024 * 1024;

#[derive(Default)]
pub(crate) struct Nv12Pool {
    cached: Mutex<Vec<Vec<u8>>>,
    hits: AtomicU64,
    allocations: AtomicU64,
}

impl Nv12Pool {
    pub(crate) fn take(&self, needed: usize) -> Vec<u8> {
        let mut cached = self.cached.lock().unwrap_or_else(|e| e.into_inner());
        // Choose the smallest fitting allocation so a temporary 1440p frame
        // does not force all later 1080p frames to retain its larger capacity.
        if let Some((index, _)) = cached.iter().enumerate()
            .filter(|(_, b)| b.capacity() >= needed)
            .min_by_key(|(_, b)| b.capacity())
        {
            let mut buffer = cached.swap_remove(index);
            buffer.clear();
            self.hits.fetch_add(1, Ordering::Relaxed);
            return buffer;
        }
        drop(cached);
        self.allocations.fetch_add(1, Ordering::Relaxed);
        Vec::with_capacity(needed)
    }

    pub(crate) fn give_back(&self, mut buffer: Vec<u8>) {
        buffer.clear();
        let bytes = buffer.capacity();
        if bytes == 0 || bytes > MAX_CACHED_BYTES { return; }
        let mut cached = self.cached.lock().unwrap_or_else(|e| e.into_inner());
        let in_pool = cached.iter().map(Vec::capacity).sum::<usize>();
        if cached.len() < MAX_CACHED_BUFFERS && in_pool + bytes <= MAX_CACHED_BYTES {
            cached.push(buffer);
        }
        // Else release the allocation immediately. There is never an
        // unbounded queue of old 4K/P010 frames hiding behind the renderer.
    }

    pub(crate) fn counts(&self) -> (u64, u64) {
        (self.hits.load(Ordering::Relaxed), self.allocations.load(Ordering::Relaxed))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn reuses_freed_buffers_without_reusing_live_buffers() {
        let pool = Nv12Pool::default();
        let live = pool.take(1024);
        let first_address = live.as_ptr();
        let concurrent = pool.take(1024);
        assert_ne!(first_address, concurrent.as_ptr());
        assert_eq!(pool.counts(), (0, 2));
        pool.give_back(live);
        let reused = pool.take(900);
        assert_eq!(reused.as_ptr(), first_address);
        assert_eq!(reused.len(), 0);
        assert_eq!(pool.counts(), (1, 2));
        pool.give_back(concurrent);
        pool.give_back(reused);
    }

    #[test]
    fn does_not_cache_large_or_unbounded_allocations() {
        let pool = Nv12Pool::default();
        for _ in 0..6 { pool.give_back(vec![0; 4 * 1024 * 1024]); }
        assert!(pool.cached.lock().unwrap().len() <= MAX_CACHED_BUFFERS);
        pool.give_back(vec![0; MAX_CACHED_BYTES + 1]);
        assert!(pool.cached.lock().unwrap().iter().all(|b| b.capacity() <= MAX_CACHED_BYTES));
    }

    #[test]
    fn can_release_frames_from_another_thread() {
        let pool = Arc::new(Nv12Pool::default());
        let bytes = pool.take(8192);
        let cloned = Arc::clone(&pool);
        std::thread::spawn(move || cloned.give_back(bytes)).join().unwrap();
        assert!(pool.take(8192).capacity() >= 8192);
        assert_eq!(pool.counts(), (1, 1));
    }
}
