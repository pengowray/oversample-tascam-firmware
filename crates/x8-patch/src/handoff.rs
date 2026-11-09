//! Passing work between two tasks without a lock: a [`Mailbox`] that holds the latest command
//! from one task, and a [`Ring`] of results from the other. The patch's card reader and DSP task
//! use them (src/patch/reader.rs and playback.rs). Both work on one core or several, because each
//! side writes only its own counter.

use core::sync::atomic::{AtomicU32, Ordering::Acquire, Ordering::Relaxed, Ordering::Release, fence};

/// The latest command from one writer, as `N` words. A newer command replaces one not read yet,
/// so the reader always acts on the latest. The count is odd while the writer is changing the
/// words, so a reader that runs in the middle sees no command and looks again later.
pub struct Mailbox<const N: usize> {
    count: AtomicU32,
    words: [AtomicU32; N],
}

impl<const N: usize> Mailbox<N> {
    pub const fn new() -> Self {
        Self { count: AtomicU32::new(0), words: [const { AtomicU32::new(0) }; N] }
    }

    /// Post a command (the writer only). Returns its number, which counts up from 1.
    pub fn post(&self, words: [u32; N]) -> u32 {
        let count = self.count.load(Relaxed);
        self.count.store(count.wrapping_add(1), Relaxed);
        fence(Release);
        for (slot, word) in self.words.iter().zip(words) {
            slot.store(word, Relaxed);
        }
        let done = count.wrapping_add(2);
        self.count.store(done, Release);
        done / 2
    }

    /// The number of the latest command posted, whether or not it has been read.
    pub fn latest(&self) -> u32 {
        self.count.load(Acquire).div_ceil(2)
    }

    /// The latest command and its number, if it is newer than `seen` and the writer is not
    /// halfway through it.
    pub fn read(&self, seen: u32) -> Option<(u32, [u32; N])> {
        let before = self.count.load(Acquire);
        if before % 2 == 1 || before / 2 == seen {
            return None;
        }
        let words = core::array::from_fn(|i| self.words[i].load(Relaxed));
        fence(Acquire);
        (self.count.load(Relaxed) == before).then_some((before / 2, words))
    }
}

impl<const N: usize> Default for Mailbox<N> {
    fn default() -> Self {
        Self::new()
    }
}

/// A ring of `len` items from one writer to one reader. The slots can be anywhere, including
/// memory the cache does not cover; the two counters keep counting past `len`.
pub struct Ring<'a, T: Copy> {
    slots: *mut T,
    len: u32,
    written: &'a AtomicU32,
    read: &'a AtomicU32,
}

impl<'a, T: Copy> Ring<'a, T> {
    /// `len` must be a power of two, so a slot's place stays the same when the counters wrap.
    ///
    /// # Safety
    /// `slots` must point to `len` slots for `T`, used by nothing else, and `written` and `read`
    /// must be used by no other ring. Only one context may push, and only one may pop.
    pub const unsafe fn new(slots: *mut T, len: u32, written: &'a AtomicU32, read: &'a AtomicU32) -> Self {
        assert!(len.is_power_of_two());
        Self { slots, len, written, read }
    }

    /// Items waiting.
    pub fn waiting(&self) -> u32 {
        self.written.load(Acquire).wrapping_sub(self.read.load(Acquire))
    }

    /// Add an item (the writer only). False if the ring is full.
    pub fn push(&self, item: T) -> bool {
        let written = self.written.load(Relaxed);
        if written.wrapping_sub(self.read.load(Acquire)) >= self.len {
            return false;
        }
        // SAFETY: the slot is inside the ring, and the reader does not read it until the count
        // below includes it.
        unsafe { self.slots.add((written % self.len) as usize).write(item) };
        self.written.store(written.wrapping_add(1), Release);
        true
    }

    /// Take the oldest item (the reader only).
    pub fn pop(&self) -> Option<T> {
        let read = self.read.load(Relaxed);
        if self.written.load(Acquire) == read {
            return None;
        }
        // SAFETY: the writer wrote this slot before counting it, and does not write it again until
        // the count below frees it.
        let item = unsafe { self.slots.add((read % self.len) as usize).read() };
        self.read.store(read.wrapping_add(1), Release);
        Some(item)
    }

    /// Drop every item waiting (the reader only).
    pub fn clear(&self) {
        self.read.store(self.written.load(Acquire), Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mailbox_gives_the_latest_command_once() {
        let mailbox = Mailbox::<2>::new();
        assert_eq!(mailbox.read(0), None);
        assert_eq!(mailbox.post([1, 10]), 1);
        assert_eq!(mailbox.post([2, 20]), 2);
        assert_eq!(mailbox.latest(), 2);
        assert_eq!(mailbox.read(0), Some((2, [2, 20])));
        assert_eq!(mailbox.read(2), None);
        assert_eq!(mailbox.post([3, 30]), 3);
        assert_eq!(mailbox.read(2), Some((3, [3, 30])));
    }

    #[test]
    fn mailbox_hides_a_command_being_written() {
        let mailbox = Mailbox::<1>::new();
        mailbox.post([5]);
        // As if the writer were stopped after marking the count odd.
        mailbox.count.store(3, Relaxed);
        assert_eq!(mailbox.read(0), None);
        assert_eq!(mailbox.latest(), 2);
    }

    #[test]
    fn ring_keeps_order_and_refuses_when_full() {
        let mut slots = [0u32; 4];
        // The counters wrap partway through.
        let (written, read) = (AtomicU32::new(u32::MAX - 1), AtomicU32::new(u32::MAX - 1));
        // SAFETY: the slots and counters are this ring's alone.
        let ring = unsafe { Ring::new(slots.as_mut_ptr(), 4, &written, &read) };
        assert!(ring.push(1) && ring.push(2) && ring.push(3) && ring.push(4));
        assert!(!ring.push(5));
        assert_eq!(ring.waiting(), 4);
        assert_eq!(ring.pop(), Some(1));
        assert!(ring.push(5));
        assert_eq!(
            [ring.pop(), ring.pop(), ring.pop(), ring.pop(), ring.pop()],
            [Some(2), Some(3), Some(4), Some(5), None]
        );
        ring.push(6);
        ring.clear();
        assert_eq!(ring.pop(), None);
    }
}
