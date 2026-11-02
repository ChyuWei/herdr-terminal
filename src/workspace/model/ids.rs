//! Stable workspace ids that cannot collide between two screens.
//!
//! A workspace id is a `u64` whose high 32 bits name the screen that allocated
//! it and whose low 32 bits are a per-screen counter. The default screen keeps
//! namespace 1 (so the example in the document schema, `4294967297`, is
//! `space 1`); every other screen id gets a 32-bit FNV-1a namespace. Restored
//! ids keep their values; every id ever seen is remembered so an id is never
//! reused even after its pane or tab is closed.

use std::collections::BTreeSet;

const DEFAULT_NAMESPACE: u64 = 1;

/// Allocates stable workspace ids for one screen.
#[derive(Clone, Debug)]
pub struct IdAllocator {
    namespace: u64,
    next: u32,
    used: BTreeSet<u64>,
}

impl IdAllocator {
    pub fn new(screen: &str) -> IdAllocator {
        IdAllocator {
            namespace: namespace(screen),
            next: 1,
            used: BTreeSet::new(),
        }
    }

    /// A fresh id in this screen's namespace.
    pub fn alloc(&mut self) -> u64 {
        loop {
            let low = self.next;
            self.next = self.next.wrapping_add(1);
            if low == 0 {
                continue;
            }
            let id = (self.namespace << 32) | u64::from(low);
            if self.used.insert(id) {
                return id;
            }
        }
    }

    /// Reserves an id found in a document. Restored ids keep their values and
    /// only push the counter past the high-water mark of this namespace.
    pub fn mark(&mut self, id: u64) {
        if id >> 32 == self.namespace {
            let low = id as u32;
            if low >= self.next {
                self.next = low.saturating_add(1);
            }
        }
        self.used.insert(id);
    }

    pub fn is_used(&self, id: u64) -> bool {
        self.used.contains(&id)
    }
}

fn namespace(screen: &str) -> u64 {
    if screen == "default" {
        return DEFAULT_NAMESPACE;
    }
    match fnv1a32(screen.as_bytes()) {
        // Keep the default namespace reserved and the counter from starting at
        // zero (zero is the empty tab's `active` placeholder, never an id).
        0 => 2,
        1 => 3,
        hash => u64::from(hash),
    }
}

fn fnv1a32(bytes: &[u8]) -> u32 {
    let mut hash = 2_166_136_261u32;
    for byte in bytes {
        hash ^= u32::from(*byte);
        hash = hash.wrapping_mul(16_777_619);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn two_screens_allocate_in_different_namespaces() {
        let mut one = IdAllocator::new("one");
        let mut two = IdAllocator::new("two");
        let a = one.alloc();
        let b = two.alloc();
        assert_ne!(a >> 32, b >> 32);
        assert_eq!(a >> 32, IdAllocator::new("one").namespace);
        // The same screen keeps a stable namespace across processes.
        assert_eq!(a >> 32, IdAllocator::new("one").namespace);

        let mut third = IdAllocator::new("third");
        let mut ids = BTreeSet::new();
        for _ in 0..32 {
            assert!(ids.insert(one.alloc()));
            assert!(ids.insert(two.alloc()));
            assert!(ids.insert(third.alloc()));
        }
    }

    #[test]
    fn the_default_namespace_matches_the_document_example() {
        let mut ids = IdAllocator::new("default");
        assert_eq!(ids.alloc(), 4_294_967_297);
        assert_eq!(ids.alloc(), 4_294_967_298);
    }

    #[test]
    fn restored_ids_are_not_reused() {
        let mut ids = IdAllocator::new("default");
        ids.mark(4_294_967_299);
        assert_eq!(ids.alloc(), 4_294_967_300);
        assert!(ids.is_used(4_294_967_299));
        assert!(!ids.is_used(4_294_967_298));
    }
}
