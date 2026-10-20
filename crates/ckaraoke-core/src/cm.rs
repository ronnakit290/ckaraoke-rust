//! Context-mixing codec for the streams inside a CKR2 record (MIDI, lyric, timing table).
//!
//! Port of `src/cm.cpp`. It is bit-exact with the C++ encoder: both sides run the same integer model,
//! so a stream packed by one build unpacks in the other. Change nothing here without re-running the
//! cross-check in `rust/README.md`.
//!
//! Each bit is predicted by six context models (orders 0, 1, 2, 3, 4 and 6 bytes) and a match model.
//! A logistic mixer with one weight set per partial byte combines them; a binary arithmetic coder
//! writes the result.

use std::sync::OnceLock;

const ORDERS: usize = 6;
const INPUTS: usize = ORDERS + 2; // context models, match model, bias
const MIN_MATCH: usize = 6; // bytes that must agree before the match model predicts
const MAX_MATCH_LEN: u32 = 31; // longer matches share the last counter
const LEARN_SHIFT: i32 = 10; // mixer learning rate
const COUNT_LIMIT: u8 = 40; // a counter settles at about 1 / COUNT_LIMIT

/// stretch(p) = ln(p / (1 - p)) is the inverse of squash. Probabilities are 12 bits; stretched values
/// are in -2047..=2047 with eight fractional bits.
fn squash(d: i32) -> i32 {
    const T: [i32; 33] = [
        1, 2, 3, 6, 10, 16, 27, 45, 73, 120, 194, 310, 488, 747, 1101, 1546, 2047, 2549, 2994, 3348, 3607,
        3785, 3901, 3975, 4022, 4050, 4068, 4079, 4085, 4089, 4092, 4093, 4094,
    ];
    if d > 2047 {
        return 4095;
    }
    if d < -2047 {
        return 1;
    }
    let w = d & 127;
    let i = ((d >> 7) + 16) as usize;
    (T[i] * (128 - w) + T[i + 1] * w + 64) >> 7
}

fn stretch(p: usize) -> i32 {
    static TABLE: OnceLock<Vec<i16>> = OnceLock::new();
    let table = TABLE.get_or_init(|| {
        let mut v = vec![0i16; 4096];
        let mut next = 0usize;
        for x in -2047..=2047 {
            let s = squash(x) as usize;
            for slot in v.iter_mut().take(s + 1).skip(next) {
                *slot = x as i16;
            }
            next = s + 1;
        }
        for slot in v.iter_mut().skip(next) {
            *slot = 2047;
        }
        v
    });
    i32::from(table[p])
}

/// An adaptive probability (16 bits) that the next bit is 1. It learns fast at first, then settles.
#[derive(Clone, Copy)]
struct Slot {
    p: u16,
    n: u8,
}

impl Default for Slot {
    fn default() -> Self {
        Slot { p: 32768, n: 0 }
    }
}

fn train(s: &mut Slot, bit: i32) {
    let target = if bit != 0 { 65535 } else { 0 };
    let rate = 8192 / (2 * i32::from(s.n) + 3); // 1 / (n + 1.5) in units of 1/8192
    s.p = (i32::from(s.p) + (((target - i32::from(s.p)) * rate) >> 13)) as u16;
    if s.n < COUNT_LIMIT {
        s.n += 1;
    }
}

fn mix(h: u32, v: u32) -> u32 {
    let h = (h ^ v).wrapping_mul(0x9E37_79B1);
    h ^ (h >> 15)
}

/// Table size for a stream of `n` bytes: about four slots per byte, between 4K and 4M.
fn table_bits(n: usize) -> u32 {
    let mut bits = 12;
    while bits < 22 && (1usize << bits) < n * 4 {
        bits += 1;
    }
    bits
}

/// The predictor. `p` gives the probability of the next bit; `update` then learns from the bit that came.
/// The encoder and decoder make the same calls in the same order, so they stay in step.
struct Model {
    bits: u32,
    tab: Vec<Vec<Slot>>,
    hash: [u32; ORDERS],
    cur: [usize; ORDERS],
    st: [i32; INPUTS],
    pr: i32,
    wset: usize,
    w: Vec<i32>,
    hist: Vec<u8>,
    c4: u32,
    c8: u32,
    c0: u32,      // bits of the current byte behind a leading 1
    bit_pos: u32, // bits of the current byte coded so far
    mtab: Vec<u32>,
    mptr: u32,
    mlen: u32,
    mm: [Slot; 2 * (MAX_MATCH_LEN as usize + 1)],
    m_slot: Option<usize>,
}

impl Model {
    fn new(n: usize) -> Self {
        let bits = table_bits(n);
        Model {
            bits,
            tab: (0..ORDERS).map(|_| vec![Slot::default(); 1usize << bits]).collect(),
            hash: [0; ORDERS],
            cur: [0; ORDERS],
            st: [0; INPUTS],
            pr: 2048,
            wset: 0,
            w: vec![1 << 14; 256 * INPUTS],
            hist: Vec::with_capacity(n),
            c4: 0,
            c8: 0,
            c0: 1,
            bit_pos: 0,
            mtab: vec![0; 1usize << bits],
            mptr: 0,
            mlen: 0,
            mm: [Slot::default(); 2 * (MAX_MATCH_LEN as usize + 1)],
            m_slot: None,
        }
    }

    fn p(&mut self) -> i32 {
        let shift = 32 - self.bits;
        for i in 0..ORDERS {
            let idx = (self.hash[i].wrapping_add(self.c0).wrapping_mul(0x9E37_79B1)) >> shift;
            self.cur[i] = idx as usize;
            self.st[i] = stretch(usize::from(self.tab[i][idx as usize].p) >> 4);
        }

        self.m_slot = None;
        self.st[ORDERS] = 0;
        if self.mlen > 0 {
            let expected = u32::from(self.hist[self.mptr as usize]);
            if ((expected | 256) >> (8 - self.bit_pos)) == self.c0 {
                let bit = ((expected >> (7 - self.bit_pos)) & 1) as usize;
                let idx = 2 * self.mlen.min(MAX_MATCH_LEN) as usize + bit;
                self.m_slot = Some(idx);
                self.st[ORDERS] = stretch(usize::from(self.mm[idx].p) >> 4);
            } else {
                self.mlen = 0; // this byte has already left the match
            }
        }
        self.st[ORDERS + 1] = 256;

        self.wset = self.c0 as usize * INPUTS;
        let mut dot: i64 = 0;
        for i in 0..INPUTS {
            dot += i64::from(self.st[i]) * i64::from(self.w[self.wset + i]);
        }
        self.pr = squash((dot >> 16).clamp(-2047, 2047) as i32);
        self.pr
    }

    fn update(&mut self, bit: i32) {
        for i in 0..ORDERS {
            train(&mut self.tab[i][self.cur[i]], bit);
        }
        if let Some(m) = self.m_slot {
            train(&mut self.mm[m], bit);
        }

        let err = (bit << 12) - self.pr;
        for i in 0..INPUTS {
            // Wrapping, as the C++ int arithmetic does in practice.
            self.w[self.wset + i] = self.w[self.wset + i].wrapping_add((self.st[i] * err) >> LEARN_SHIFT);
        }

        self.c0 = (self.c0 << 1) | bit as u32;
        self.bit_pos += 1;
        if self.bit_pos == 8 {
            self.byte_done(self.c0 as u8);
            self.c0 = 1;
            self.bit_pos = 0;
        }
    }

    fn byte_done(&mut self, c: u8) {
        self.hist.push(c);
        self.c8 = (self.c8 << 8) | (self.c4 >> 24);
        self.c4 = (self.c4 << 8) | u32::from(c);
        self.hash[0] = mix(0, 0);
        self.hash[1] = mix(1, self.c4 & 0xFF);
        self.hash[2] = mix(2, self.c4 & 0xFFFF);
        self.hash[3] = mix(3, self.c4 & 0xFF_FFFF);
        self.hash[4] = mix(4, self.c4);
        self.hash[5] = mix(mix(5, self.c4), self.c8 & 0xFFFF);

        // Follow the current match while it keeps agreeing; otherwise look for a new one.
        let n = self.hist.len();
        if self.mlen > 0 && self.hist[self.mptr as usize] == c {
            self.mlen += 1;
            self.mptr += 1;
        } else {
            self.mlen = 0;
        }
        if n >= MIN_MATCH {
            let mut h: u32 = 0;
            for &b in &self.hist[n - MIN_MATCH..n] {
                h = mix(h, u32::from(b));
            }
            let h = (h >> (32 - self.bits)) as usize;
            if self.mlen == 0 && self.mtab[h] > 0 {
                let cand = self.mtab[h] as usize;
                // Count back over the bytes that really agree (the hash can collide).
                let mut len = 0usize;
                while len < cand && len < 255 && self.hist[cand - 1 - len] == self.hist[n - 1 - len] {
                    len += 1;
                }
                if len >= MIN_MATCH {
                    self.mlen = len as u32;
                    self.mptr = cand as u32;
                }
            }
            self.mtab[h] = n as u32;
        }
    }
}

/// Binary arithmetic coder. It writes a byte whenever the two bounds agree in their top byte.
struct Encoder<'a> {
    out: &'a mut Vec<u8>,
    x1: u32,
    x2: u32,
}

impl Encoder<'_> {
    fn code(&mut self, bit: i32, p: i32) {
        let mid = self.x1.wrapping_add(((u64::from(self.x2 - self.x1) * p as u64) >> 12) as u32);
        if bit != 0 {
            self.x2 = mid;
        } else {
            self.x1 = mid.wrapping_add(1);
        }
        while (self.x1 ^ self.x2) & 0xFF00_0000 == 0 {
            self.out.push((self.x2 >> 24) as u8);
            self.x1 <<= 8;
            self.x2 = (self.x2 << 8) | 255;
        }
    }

    fn flush(&mut self) {
        for _ in 0..4 {
            self.out.push((self.x1 >> 24) as u8);
            self.x1 <<= 8;
        }
    }
}

struct Decoder<'a> {
    input: &'a [u8],
    pos: usize, // may run past the end: that counts as overrun, and the caller rejects it
    x1: u32,
    x2: u32,
    x: u32,
}

impl<'a> Decoder<'a> {
    fn new(input: &'a [u8]) -> Self {
        let mut d = Decoder { input, pos: 0, x1: 0, x2: 0xFFFF_FFFF, x: 0 };
        for _ in 0..4 {
            d.x = (d.x << 8) | d.next_byte();
        }
        d
    }

    fn next_byte(&mut self) -> u32 {
        let v = self.input.get(self.pos).copied().map_or(0, u32::from);
        self.pos += 1;
        v
    }

    fn decode(&mut self, p: i32) -> i32 {
        let mid = self.x1.wrapping_add(((u64::from(self.x2 - self.x1) * p as u64) >> 12) as u32);
        let bit = i32::from(self.x <= mid);
        if bit != 0 {
            self.x2 = mid;
        } else {
            self.x1 = mid.wrapping_add(1);
        }
        while (self.x1 ^ self.x2) & 0xFF00_0000 == 0 {
            self.x1 <<= 8;
            self.x2 = (self.x2 << 8) | 255;
            self.x = (self.x << 8) | self.next_byte();
        }
        bit
    }
}

/// Compresses `data`. The output is empty for empty input.
pub fn compress(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    if data.is_empty() {
        return out;
    }
    let mut model = Model::new(data.len());
    let mut enc = Encoder { out: &mut out, x1: 0, x2: 0xFFFF_FFFF };
    for &byte in data {
        for b in (0..8).rev() {
            let bit = i32::from((byte >> b) & 1);
            let p = model.p();
            enc.code(bit, p);
            model.update(bit);
        }
    }
    enc.flush();
    out
}

/// Decodes exactly `out_len` bytes. Returns `None` if `input` is not exactly the length the encoder
/// wrote (a truncated or padded stream). Garbage in gives garbage out, so callers check a MAC first.
pub fn decompress(input: &[u8], out_len: usize) -> Option<Vec<u8>> {
    if out_len == 0 {
        return input.is_empty().then(Vec::new);
    }
    let mut model = Model::new(out_len);
    let mut dec = Decoder::new(input);
    let mut out = Vec::with_capacity(out_len);
    for _ in 0..out_len {
        let mut c = 0u8;
        for _ in 0..8 {
            let bit = dec.decode(model.p());
            model.update(bit);
            c = (c << 1) | bit as u8;
        }
        out.push(c);
    }
    (dec.pos == input.len()).then_some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_text_and_binary() {
        let text = "ทดสอบ ก ข ค ".repeat(40);
        let packed = compress(text.as_bytes());
        assert!(packed.len() < text.len());
        assert_eq!(decompress(&packed, text.len()).as_deref(), Some(text.as_bytes()));

        let mut x: u32 = 12345;
        let noise: Vec<u8> = (0..5000)
            .map(|_| {
                x = x.wrapping_mul(1_103_515_245).wrapping_add(12_345);
                (x >> 16) as u8
            })
            .collect();
        let packed = compress(&noise);
        assert_eq!(decompress(&packed, noise.len()).as_deref(), Some(noise.as_slice()));
    }

    #[test]
    fn rejects_wrong_length() {
        let packed = compress(b"hello hello hello");
        assert!(decompress(&packed[..packed.len() - 1], 17).is_none());
        assert!(decompress(&packed, 0).is_none());
        assert_eq!(decompress(&[], 0), Some(Vec::new()));
    }
}
