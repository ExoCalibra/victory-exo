//! `ds_swizzle_b32` rotate (0xC000..0xE000) and FFT (0xE000..=0xFFFF) modes (Marvisak/VIctory#606).

use super::{Bin, Lower, Why};
use crate::emit::bitcode::{Attr, V};

impl<'a> Lower<'a> {
    pub(super) fn ds_swizzle_fft_or_rotate(
        &mut self,
        c: usize,
        lane: V,
        offset: u32,
        fft: bool,
    ) -> Result<V, Why> {
        let k = |l: &mut Self, v: u32| l.s.m.i32(v);
        let (low, high) = (k(self, 0x1F), k(self, !0x1F));
        let mask = offset & 0x1F;
        let i = self.s.m.bin(c, Bin::And, lane, low);
        let j = if fft {
            let (top, m, i32t) = (k(self, 27), k(self, mask), self.s.t.i32);
            let down = k(self, mask.count_ones());
            let j = self.s.op(c, "unary.i32", Attr::ReadNone, i32t, 30, &[i]);
            let j = self.s.m.bin(c, Bin::LShr, j, top);
            let j = self.s.m.bin(c, Bin::LShr, j, down);
            let own = self.s.m.bin(c, Bin::And, i, m);
            self.s.m.bin(c, Bin::Or, j, own)
        } else {
            if mask != 0 {
                return Err("a masked ds_swizzle rotation");
            }
            let n = offset >> 5 & 0x1F;
            let n = if offset & 0x400 != 0 {
                n.wrapping_neg()
            } else {
                n
            };
            let n = k(self, n);
            let j = self.s.m.bin(c, Bin::Add, i, n);
            self.s.m.bin(c, Bin::And, j, low)
        };
        let group = self.s.m.bin(c, Bin::And, lane, high);
        Ok(self.s.m.bin(c, Bin::Or, group, j))
    }

    /// `-x` as the `neg` input modifier computes it: bit 31 flipped, a NaN's
    /// sign too (Marvisak/VIctory#2823).
    pub(super) fn fneg(&mut self, f: V) -> V {
        let x = self.as_int(f);
        let sign = self.s.m.i32(0x8000_0000);
        let x = self.s.m.bin(self.cur, Bin::Xor, x, sign);
        self.as_float(x)
    }
}
