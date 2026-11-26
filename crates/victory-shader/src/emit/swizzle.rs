//! `ds_swizzle_b32` rotate (0xC000..0xE000) and FFT (0xE000..=0xFFFF) modes (Marvisak/VIctory#606).

use super::{Bin, Lower, Rmw, Why};
use crate::emit::bitcode::{Attr, V, pred};
use crate::emit::shader::flags;
use crate::native::StageIo;

/// Whether programs with console lanes in groups of several host waves take their console
/// threads from a group counter (`ticket`, fxceless/victory-fork#1261 / #1243).
/// `VICTORY_THREAD_TICKET=0`: the flat id of the wave's first lane rounded down, as before.
pub(super) fn ticket_on() -> bool {
    std::env::var_os("VICTORY_THREAD_TICKET").is_none_or(|v| v != "0")
}

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

    /// With `halves` or `ticket`: the first console thread of this host wave (0, w, 2w, ... for
    /// host waves of w lanes; in the paired form 0, 64, 128, ...). A GPU need not give a host
    /// wave consecutive thread indices (NVIDIA's 8 x 8 groups, the 16 x 2 tiles of 2-D groups,
    /// fxceless/victory-fork#1243 / #1261), so each wave takes its threads from an LDS counter
    /// instead: every thread zeroes it, the group meets, and each wave's first active lane adds
    /// for the wave. Full waves take the next block of w from the low 16 bits; partial waves
    /// take their lane count from the high 16 bits past the full blocks.
    pub(super) fn wave_base(&mut self) -> Option<V> {
        let slot = self.halves.or(self.ticket)?;
        if let Some(v) = self.wave_base {
            return Some(v);
        }
        let (e, i32t) = (self.s.entry, self.s.t.i32);
        let lds = self.s.lds();
        let k = self.s.m.i32(slot);
        let ptr = self.s.m.element(e, lds, k);
        let zero = self.s.m.i32(0);
        self.s.m.store(e, ptr, zero);
        self.group_sync(e);
        let base = if self.twin.is_some() {
            let lane = self.s.op(e, "waveGetLaneIndex", Attr::ReadOnly, i32t, 111, &[]);
            let first = self.s.m.cmp(e, pred::EQ, lane, zero);
            let step = self.s.m.i32(64);
            let add = self.s.m.select(e, first, step, zero);
            let ptr = self.s.m.element(e, lds, k);
            let old = self.s.m.atomic(e, Rmw::Add, ptr, add);
            let base = self.s.op(e, "waveReadLaneFirst.i32", Attr::NoUnwind, i32t, 118, &[old]);
            self.wave_rank = Some(lane);
            base
        } else {
            let w = self.s.wave_size.unwrap_or(32);
            let group: u32 = self.s.threads.map_or(w, |t| t.iter().product());
            let one = self.s.m.i32(1);
            let (sum, unsigned) = (self.s.m.i8(0), self.s.m.i8(1));
            let count = self.s.op(e, "waveActiveOp.i32", Attr::NoUnwind, i32t, 119, &[one, sum, unsigned]);
            let rank = self.s.op(e, "wavePrefixOp.i32", Attr::NoUnwind, i32t, 121, &[one, sum, unsigned]);
            let kw = self.s.m.i32(w);
            let full = self.s.m.cmp(e, pred::EQ, count, kw);
            let first = self.s.m.cmp(e, pred::EQ, rank, zero);
            let sixteen = self.s.m.i32(16);
            let high = self.s.m.bin(e, Bin::Shl, count, sixteen);
            let take = self.s.m.select(e, full, kw, high);
            let add = self.s.m.select(e, first, take, zero);
            let ptr = self.s.m.element(e, lds, k);
            let old = self.s.m.atomic(e, Rmw::Add, ptr, add);
            let found = self.s.op(e, "waveReadLaneFirst.i32", Attr::NoUnwind, i32t, 118, &[old]);
            let low_mask = self.s.m.i32(0xffff);
            let low = self.s.m.bin(e, Bin::And, found, low_mask);
            let past = self.s.m.bin(e, Bin::LShr, found, sixteen);
            let blocks = self.s.m.i32(group / w * w);
            let tail = self.s.m.bin(e, Bin::Add, blocks, past);
            let base = self.s.m.select(e, full, low, tail);
            self.wave_rank = Some(rank);
            base
        };
        self.s.flags |= flags::WAVE_OPS;
        // a counter in the program's own LDS (the paired form's, when it has no dword to
        // spare): every wave has its base before any thread runs the program
        if matches!(self.x.native.io, StageIo::Cs(cs) if slot < cs.lds_bytes() / 4) {
            self.group_sync(e);
        }
        self.wave_base = Some(base);
        Some(base)
    }
}
