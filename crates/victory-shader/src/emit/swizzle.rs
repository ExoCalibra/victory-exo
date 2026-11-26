//! `ds_swizzle_b32` rotate (0xC000..0xE000) and FFT (0xE000..=0xFFFF) modes (Marvisak/VIctory#606).

use super::{Bin, Lower, Why};
use crate::emit::bitcode::{Attr, V, pred};

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

    /// `VICTORY_NAN_GUARD` (on unless `0`): guards pixel colour exports (and, with
    /// `VICTORY_NAN_GUARD=stores`, typed image/buffer stores) by mapping NaN to 0
    /// and ±Inf to ±65504 so one bad pixel cannot spread across bloom/sky/water
    /// (PastiiJay / KLBF Lab, fxceless/victory-fork#837).
    pub(super) fn nan_guard(&mut self, f: V, strict: bool) -> V {
        static MODE: std::sync::OnceLock<u8> = std::sync::OnceLock::new();
        let mode = *MODE.get_or_init(|| match std::env::var("VICTORY_NAN_GUARD").as_deref() {
            Ok("0") => 0,
            Ok("stores") => 2,
            _ => 1,
        });
        if mode == 0 || (!strict && mode < 2) {
            return f;
        }
        let c = self.cur;
        let bits = self.as_int(f);
        let mag = self.s.m.i32(0x7FFF_FFFF);
        let abs = self.s.m.bin(c, Bin::And, bits, mag);
        let inf_bits = self.s.m.i32(0x7F80_0000);
        let nan = if strict {
            self.s.m.cmp(c, pred::IUGT, abs, inf_bits)
        } else {
            let quiet = self.s.m.i32(0x7FC0_0000);
            self.s.m.cmp(c, pred::EQ, abs, quiet)
        };
        let inf = self.s.m.cmp(c, pred::EQ, abs, inf_bits);
        let sign_bit = self.s.m.i32(0x8000_0000);
        let sign = self.s.m.bin(c, Bin::And, bits, sign_bit);
        let big = self.s.m.i32(65504f32.to_bits());
        let big = self.s.m.bin(c, Bin::Or, sign, big);
        let zero = self.s.m.i32(0);
        let r = self.s.m.select(c, inf, big, bits);
        let r = self.s.m.select(c, nan, zero, r);
        self.as_float(r)
    }
}
