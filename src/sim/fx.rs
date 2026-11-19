//! Fixed-seed FxHash maps for the sim's hot lookups: much faster than the
//! default SipHash on small integer keys, and iteration order is the same in
//! every process (the default hasher is randomly seeded per process).

use std::hash::{BuildHasherDefault, Hasher};

#[derive(Default, Clone, Copy)]
pub struct Fx(u64);

impl Fx {
    #[inline]
    fn add(&mut self, w: u64) {
        self.0 = (self.0.rotate_left(5) ^ w).wrapping_mul(0x51_7c_c1_b7_27_22_0a_95);
    }
}

impl Hasher for Fx {
    #[inline]
    fn write(&mut self, bytes: &[u8]) {
        for c in bytes.chunks(8) {
            let mut b = [0u8; 8];
            b[..c.len()].copy_from_slice(c);
            self.add(u64::from_le_bytes(b));
        }
    }
    #[inline]
    fn write_u8(&mut self, i: u8) { self.add(i as u64) }
    #[inline]
    fn write_u16(&mut self, i: u16) { self.add(i as u64) }
    #[inline]
    fn write_u32(&mut self, i: u32) { self.add(i as u64) }
    #[inline]
    fn write_u64(&mut self, i: u64) { self.add(i) }
    #[inline]
    fn write_usize(&mut self, i: usize) { self.add(i as u64) }
    #[inline]
    fn finish(&self) -> u64 { self.0 }
}

pub type HashMap<K, V> = std::collections::HashMap<K, V, BuildHasherDefault<Fx>>;
pub type HashSet<K> = std::collections::HashSet<K, BuildHasherDefault<Fx>>;

#[cfg(test)]
mod tests {
    #[test]
    fn iteration_order_is_the_same_for_every_map() {
        // the default hasher would give each map its own random order
        let a: super::HashSet<usize> = (0..500).map(|i| i * 7919).collect();
        let b: super::HashSet<usize> = (0..500).rev().map(|i| i * 7919).collect();
        assert!(a.iter().eq(b.iter()));
    }
}
