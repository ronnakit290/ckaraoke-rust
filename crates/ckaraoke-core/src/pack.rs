//! `.ckr` packs: many songs in one encrypted, compressed file. Port of `src/ckr.cpp`.
//!
//! Layout (integers little-endian):
//!   0    "CKR2"          (files written by older builds say "CKR1" and are still opened)
//!   4    salt[16]        random per file; PBKDF2 input together with the passphrase
//!   20   iterations u32  PBKDF2-HMAC-SHA256 rounds
//!   24   nonce[12]       ChaCha20 nonce
//!   36   index offset u64
//!   44   index size u32  (ciphertext bytes)
//!   48   records         each is ChaCha20 ciphertext of one song
//!   ..   index           ChaCha20 ciphertext: song table (title, artist, key, offset, size, record tag)
//!   end  32 bytes        HMAC-SHA256(macKey, header + index ciphertext)
//!
//! A CKR2 record holds three streams, MIDI, lyric and timing table, each coded with the context-mixing
//! codec (`cm`) when that is smaller. The timing table is first turned into deltas. A CKR1 record was
//! `[u32 raw size][zlib(MIDI, LYR, CUR)]` and is only read.
//!
//! The keystream position is the byte's file offset. Each record has a tag over its offset and
//! ciphertext, so one song can be read and checked alone; the index tag covers those record tags.

use std::fs::{self, File};
use std::io::{BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use flate2::read::ZlibDecoder;
use zeroize::Zeroize;

use crate::crypto::{self, Digest};
use crate::{cm, library, song, Error, Result};

const MAGIC_V1: &[u8; 4] = b"CKR1";
const MAGIC_V2: &[u8; 4] = b"CKR2";
pub const HEADER_SIZE: usize = 48;
const TAG_SIZE: usize = 32;
const ITERATIONS: u32 = 100_000; // PBKDF2 rounds for new packs
const MAX_ITERATIONS: u32 = 10_000_000;
const MAX_INDEX_SIZE: u64 = 128 << 20;
const MAX_RAW_SIZE: u32 = 256 << 20; // one song, uncompressed
const MAX_ENTRIES: usize = 1_000_000;

/// One song as stored in a pack.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PackSong {
    pub title: String,
    pub artist: String,
    pub key: String,
    /// Standard MIDI file.
    pub midi: Vec<u8>,
    /// NCN `.lyr` text (TIS-620 or UTF-8). Empty if the song has none.
    pub lyric: Vec<u8>,
    /// NCN `.cur` timing table. Empty if none.
    pub cursor: Vec<u8>,
}

/// One row of a pack's song table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackEntry {
    pub title: String,
    pub artist: String,
    pub key: String,
    pub offset: u64,
    pub size: u32,
    pub tag: Digest,
}

// ---------------------------------------------------------------------------
// Little-endian helpers

fn get32(p: &[u8]) -> u32 {
    u32::from_le_bytes([p[0], p[1], p[2], p[3]])
}

fn get64(p: &[u8]) -> u64 {
    u64::from_le_bytes(p[..8].try_into().expect("8 bytes"))
}

fn put32(b: &mut Vec<u8>, v: u32) {
    b.extend_from_slice(&v.to_le_bytes());
}

fn put_blob(b: &mut Vec<u8>, data: &[u8]) {
    put32(b, data.len() as u32);
    b.extend_from_slice(data);
}

/// Bounds-checked reader over decrypted bytes: every read can fail, none goes past the end.
struct Cursor<'a> {
    buf: &'a [u8],
    off: usize,
}

impl<'a> Cursor<'a> {
    fn new(buf: &'a [u8]) -> Self {
        Cursor { buf, off: 0 }
    }

    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        if self.buf.len() - self.off < n {
            return None;
        }
        let s = &self.buf[self.off..self.off + n];
        self.off += n;
        Some(s)
    }

    fn u8(&mut self) -> Option<u8> {
        self.take(1).map(|s| s[0])
    }

    fn u32(&mut self) -> Option<u32> {
        self.take(4).map(get32)
    }

    fn u64(&mut self) -> Option<u64> {
        self.take(8).map(get64)
    }

    fn blob(&mut self) -> Option<&'a [u8]> {
        let n = self.u32()? as usize;
        self.take(n)
    }

    fn string(&mut self) -> Option<String> {
        self.blob().map(|b| String::from_utf8_lossy(b).into_owned())
    }

    /// LEB128, at most ten bytes.
    fn varint(&mut self) -> Option<u64> {
        let mut v = 0u64;
        for shift in (0..64).step_by(7) {
            let b = self.u8()?;
            v |= u64::from(b & 0x7F) << shift;
            if b & 0x80 == 0 {
                return Some(v);
            }
        }
        None
    }

    fn done(&self) -> bool {
        self.off == self.buf.len()
    }
}

fn put_varint(b: &mut Vec<u8>, mut v: u64) {
    while v >= 0x80 {
        b.push((v as u8) | 0x80);
        v >>= 7;
    }
    b.push(v as u8);
}

// ---------------------------------------------------------------------------
// Timing-table deltas and stream coding

/// The timing table holds one little-endian value of `width` bytes per lyric byte, and the values only
/// grow. Each is sent as its difference from the previous one, zigzagged and written as a varint.
fn delta_encode(raw: &[u8], width: usize) -> Vec<u8> {
    let mask: u64 = if width == 8 { u64::MAX } else { (1u64 << (8 * width)) - 1 };
    let mut out = Vec::new();
    let mut prev = 0u64;
    for chunk in raw.chunks_exact(width) {
        let v = chunk.iter().rev().fold(0u64, |acc, &b| (acc << 8) | u64::from(b));
        let d = v.wrapping_sub(prev) & mask;
        let sd: i64 = if (d >> (8 * width - 1)) != 0 { d as i64 - mask as i64 - 1 } else { d as i64 };
        let zig = if sd >= 0 { (sd as u64) << 1 } else { ((-(sd + 1)) as u64) << 1 | 1 };
        put_varint(&mut out, zig);
        prev = v;
    }
    out
}

/// Undoes `delta_encode`. `size` is the length of the table, which must be a whole number of values.
fn delta_decode(coded: &[u8], width: usize, size: usize) -> Option<Vec<u8>> {
    if !size.is_multiple_of(width) {
        return None;
    }
    let mask: u64 = if width == 8 { u64::MAX } else { (1u64 << (8 * width)) - 1 };
    let mut c = Cursor::new(coded);
    let mut out = vec![0u8; size];
    let mut prev = 0u64;
    for chunk in out.chunks_exact_mut(width) {
        let z = c.varint()?;
        let sd: i64 = if z & 1 != 0 { -((z >> 1) as i64) - 1 } else { (z >> 1) as i64 };
        let v = prev.wrapping_add(sd as u64) & mask;
        for (b, slot) in chunk.iter_mut().enumerate() {
            *slot = (v >> (8 * b)) as u8;
        }
        prev = v;
    }
    c.done().then_some(out)
}

/// One stream of a CKR2 record.
#[derive(Default)]
struct CodedStream {
    method: u8,    // 0 = stored as is, 1 = cm::compress
    transform: u8, // 0 = none, otherwise the value width used by delta_encode (2 or 4)
    size: u32,     // length of the stream itself
    coded: u32,    // length after the transform, before the method
    packed: Vec<u8>,
}

/// Codes one stream in the cheapest form. Only the timing table tries the delta forms.
fn code_stream(raw: &[u8], timing: bool) -> CodedStream {
    let mut best = CodedStream {
        size: raw.len() as u32,
        coded: raw.len() as u32,
        packed: raw.to_vec(),
        ..Default::default()
    };
    let mut consider = |transform: u8, coded: &[u8]| {
        let z = cm::compress(coded);
        let (method, packed) = if !z.is_empty() && z.len() < coded.len() { (1, z) } else { (0, coded.to_vec()) };
        if packed.len() < best.packed.len() {
            best = CodedStream {
                method,
                transform,
                size: raw.len() as u32,
                coded: coded.len() as u32,
                packed,
            };
        }
    };
    consider(0, raw);
    if timing {
        if raw.len().is_multiple_of(2) {
            consider(2, &delta_encode(raw, 2));
        }
        if raw.len().is_multiple_of(4) {
            consider(4, &delta_encode(raw, 4));
        }
    }
    best
}

fn put_stream(b: &mut Vec<u8>, s: &CodedStream) {
    b.push(s.method);
    b.push(s.transform);
    put32(b, s.size);
    put32(b, s.coded);
    put32(b, s.packed.len() as u32);
    b.extend_from_slice(&s.packed);
}

fn read_stream(c: &mut Cursor) -> Result<Vec<u8>> {
    let bad = || Error::new("ข้อมูลเพลงในไฟล์ .ckr ไม่ถูกต้อง");
    let method = c.u8().ok_or_else(bad)?;
    let transform = c.u8().ok_or_else(bad)?;
    let size = c.u32().ok_or_else(bad)?;
    let coded = c.u32().ok_or_else(bad)?;
    let packed_size = c.u32().ok_or_else(bad)? as usize;
    let packed = c.take(packed_size).ok_or_else(bad)?;
    if size > MAX_RAW_SIZE || coded > MAX_RAW_SIZE {
        return Err(bad());
    }
    let coded_len = coded as usize;
    let bytes = match method {
        0 => {
            if packed_size != coded_len {
                return Err(bad());
            }
            packed.to_vec()
        }
        1 => cm::decompress(packed, coded_len)
            .ok_or_else(|| Error::new("คลายข้อมูลเพลงในไฟล์ .ckr ไม่สำเร็จ"))?,
        _ => return Err(bad()),
    };
    match transform {
        0 if coded == size => Ok(bytes),
        0 => Err(bad()),
        2 | 4 => delta_decode(&bytes, transform as usize, size as usize).ok_or_else(bad),
        _ => Err(bad()),
    }
}

/// Song record before encryption (CKR2): three streams, MIDI, lyric and timing table.
fn pack_record(s: &PackSong) -> Result<Vec<u8>> {
    let total = s.midi.len() as u64 + s.lyric.len() as u64 + s.cursor.len() as u64;
    if total > u64::from(MAX_RAW_SIZE) {
        return Err(Error::new(format!("เพลง \"{}\" ใหญ่เกินไป", s.title)));
    }
    let mut blob = Vec::new();
    put_stream(&mut blob, &code_stream(&s.midi, false));
    put_stream(&mut blob, &code_stream(&s.lyric, false));
    put_stream(&mut blob, &code_stream(&s.cursor, true));
    Ok(blob)
}

fn unpack_record_v2(blob: &[u8]) -> Result<PackSong> {
    let mut c = Cursor::new(blob);
    let midi = read_stream(&mut c)?;
    let lyric = read_stream(&mut c)?;
    let cursor = read_stream(&mut c)?;
    if !c.done() {
        return Err(Error::new("ข้อมูลเพลงในไฟล์ .ckr ไม่ถูกต้อง"));
    }
    Ok(PackSong { midi, lyric, cursor, ..Default::default() })
}

/// Song record of a CKR1 file: `[u32 raw size][zlib(midi, lyric, cursor)]`. Read only.
fn unpack_record_v1(blob: &[u8]) -> Result<PackSong> {
    let bad = || Error::new("ข้อมูลเพลงในไฟล์ .ckr ไม่ถูกต้อง");
    if blob.len() < 4 || get32(blob) > MAX_RAW_SIZE {
        return Err(bad());
    }
    let raw_size = get32(blob) as usize;
    let mut raw = Vec::with_capacity(raw_size);
    ZlibDecoder::new(&blob[4..])
        .take(raw_size as u64 + 1)
        .read_to_end(&mut raw)
        .map_err(|_| Error::new("คลายข้อมูลเพลงในไฟล์ .ckr ไม่สำเร็จ"))?;
    if raw.len() > raw_size {
        return Err(Error::new("คลายข้อมูลเพลงในไฟล์ .ckr ไม่สำเร็จ"));
    }
    let mut c = Cursor::new(&raw);
    let midi = c.blob().ok_or_else(bad)?.to_vec();
    let lyric = c.blob().ok_or_else(bad)?.to_vec();
    let cursor = c.blob().ok_or_else(bad)?.to_vec();
    if !c.done() {
        return Err(bad());
    }
    Ok(PackSong { midi, lyric, cursor, ..Default::default() })
}

fn pack_index(entries: &[PackEntry]) -> Vec<u8> {
    let mut b = Vec::new();
    put32(&mut b, entries.len() as u32);
    for e in entries {
        put_blob(&mut b, e.title.as_bytes());
        put_blob(&mut b, e.artist.as_bytes());
        put_blob(&mut b, e.key.as_bytes());
        b.extend_from_slice(&e.offset.to_le_bytes());
        put32(&mut b, e.size);
        b.extend_from_slice(&e.tag);
    }
    b
}

fn parse_index(b: &[u8]) -> Option<Vec<PackEntry>> {
    let mut c = Cursor::new(b);
    let count = c.u32()? as usize;
    if count == 0 || count > MAX_ENTRIES {
        return None;
    }
    let mut out = Vec::with_capacity(count);
    for _ in 0..count {
        let title = c.string()?;
        let artist = c.string()?;
        let key = c.string()?;
        let offset = c.u64()?;
        let size = c.u32()?;
        let tag: Digest = c.take(TAG_SIZE)?.try_into().ok()?;
        out.push(PackEntry { title, artist, key, offset, size, tag });
    }
    c.done().then_some(out)
}

fn read_at(f: &mut File, pos: u64, buf: &mut [u8]) -> std::io::Result<()> {
    f.seek(SeekFrom::Start(pos))?;
    f.read_exact(buf)
}

// ---------------------------------------------------------------------------
// Reading

/// An opened pack. Opening derives the keys (about 0.2 s in release builds) and checks the song table;
/// songs are read one at a time afterwards, so a whole pack is never loaded.
pub struct PackReader {
    file: PathBuf,
    enc_key: [u8; 32],
    mac_key: [u8; 32],
    nonce: [u8; 12],
    version: u8, // 1 for CKR1 (zlib records), 2 for CKR2
    entries: Vec<PackEntry>,
}

impl Drop for PackReader {
    fn drop(&mut self) {
        self.enc_key.zeroize();
        self.mac_key.zeroize();
    }
}

impl PackReader {
    pub fn open(file: impl AsRef<Path>, passphrase: &str) -> Result<Self> {
        let file = file.as_ref();
        let not_pack = || Error::new("ไม่ใช่ไฟล์ .ckr ที่รองรับ");
        let mut f = File::open(file).map_err(|_| Error::new(format!("เปิดไฟล์ไม่ได้: {}", file.display())))?;
        let file_size = f.metadata().map_err(|_| Error::new("อ่านไฟล์ .ckr ไม่สำเร็จ"))?.len();
        if file_size < (HEADER_SIZE + TAG_SIZE) as u64 {
            return Err(not_pack());
        }

        let mut header = [0u8; HEADER_SIZE];
        read_at(&mut f, 0, &mut header).map_err(|_| not_pack())?;
        let version = match &header[0..4] {
            m if m == MAGIC_V2 => 2,
            m if m == MAGIC_V1 => 1,
            _ => return Err(not_pack()),
        };
        let iterations = get32(&header[20..]);
        let index_offset = get64(&header[36..]);
        let index_size = get32(&header[44..]);
        let structure_ok = iterations != 0
            && iterations <= MAX_ITERATIONS
            && index_size != 0
            && u64::from(index_size) <= MAX_INDEX_SIZE
            && index_offset >= HEADER_SIZE as u64
            && index_offset.checked_add(u64::from(index_size) + TAG_SIZE as u64) == Some(file_size);
        if !structure_ok {
            return Err(Error::new("ไฟล์ .ckr เสียหาย (โครงสร้างไม่ถูกต้อง)"));
        }

        let mut index = vec![0u8; index_size as usize];
        let mut stored: Digest = [0; 32];
        read_at(&mut f, index_offset, &mut index)
            .and_then(|_| read_at(&mut f, index_offset + u64::from(index_size), &mut stored))
            .map_err(|_| Error::new("อ่านไฟล์ .ckr ไม่สำเร็จ"))?;

        let mut salt = [0u8; 16];
        salt.copy_from_slice(&header[4..20]);
        let (enc_key, mac_key) = crypto::derive_keys(passphrase, &salt, iterations);
        let mut reader = PackReader {
            file: file.to_path_buf(),
            enc_key,
            mac_key,
            nonce: [0; 12],
            version,
            entries: Vec::new(),
        };
        reader.nonce.copy_from_slice(&header[24..36]);

        if !crypto::hmac_verify(&reader.mac_key, &[header.as_slice(), index.as_slice()], &stored) {
            return Err(Error::new("คีย์ไม่ถูกต้อง หรือไฟล์ .ckr เสียหาย"));
        }
        crypto::chacha_xor(&reader.enc_key, &reader.nonce, index_offset, &mut index)?;
        let entries = parse_index(&index).ok_or_else(|| Error::new("อ่านรายการเพลงในไฟล์ .ckr ไม่ได้"))?;
        for e in &entries {
            let placed = e.offset >= HEADER_SIZE as u64
                && e.offset <= index_offset
                && e.size >= 4
                && u64::from(e.size) <= index_offset - e.offset;
            if !placed {
                return Err(Error::new("ไฟล์ .ckr เสียหาย (ตำแหน่งเพลงผิด)"));
            }
        }
        reader.entries = entries;
        Ok(reader)
    }

    pub fn entries(&self) -> &[PackEntry] {
        &self.entries
    }

    /// Reads and checks one song, by its position in the table (from 0).
    pub fn read(&self, index: usize) -> Result<PackSong> {
        let e = self
            .entries
            .get(index)
            .ok_or_else(|| Error::new(format!("ไม่พบเพลงลำดับที่ {} ในไฟล์ .ckr", index + 1)))?;
        let mut f = File::open(&self.file).map_err(|_| Error::new("อ่านไฟล์ .ckr ไม่สำเร็จ"))?;
        let mut blob = vec![0u8; e.size as usize];
        read_at(&mut f, e.offset, &mut blob)
            .map_err(|_| Error::new(format!("อ่านเพลง \"{}\" จากไฟล์ .ckr ไม่ได้", e.title)))?;
        // Check the ciphertext before decrypting it.
        if !crypto::hmac_verify(&self.mac_key, &[e.offset.to_le_bytes().as_slice(), blob.as_slice()], &e.tag) {
            return Err(Error::new(format!("ข้อมูลเพลง \"{}\" ในไฟล์ .ckr เสียหาย", e.title)));
        }
        crypto::chacha_xor(&self.enc_key, &self.nonce, e.offset, &mut blob)?;
        let mut song = if self.version == 1 { unpack_record_v1(&blob)? } else { unpack_record_v2(&blob)? };
        song.title = e.title.clone();
        song.artist = e.artist.clone();
        song.key = e.key.clone();
        Ok(song)
    }
}

// ---------------------------------------------------------------------------
// Writing

/// Writes a pack one song at a time, so a large library never sits in memory whole. The file is built
/// under a temporary name and renamed into place by `finish`; anything else leaves no file behind, and
/// an existing pack stays as it was.
pub struct PackWriter {
    file: PathBuf,
    tmp: PathBuf,
    out: Option<BufWriter<File>>,
    header: [u8; HEADER_SIZE],
    nonce: [u8; 12],
    enc_key: [u8; 32],
    mac_key: [u8; 32],
    pos: u64,
    index_bytes: u64,
    entries: Vec<PackEntry>,
    finished: bool,
}

impl PackWriter {
    pub fn open(file: impl AsRef<Path>, passphrase: &str) -> Result<Self> {
        let file = file.as_ref().to_path_buf();
        if passphrase.is_empty() {
            return Err(Error::new("ยังไม่ได้ตั้งคีย์ .ckr"));
        }
        let mut header = [0u8; HEADER_SIZE];
        header[0..4].copy_from_slice(MAGIC_V2);
        crypto::random_bytes(&mut header[4..20])?;
        header[20..24].copy_from_slice(&ITERATIONS.to_le_bytes());
        crypto::random_bytes(&mut header[24..36])?;
        let mut salt = [0u8; 16];
        salt.copy_from_slice(&header[4..20]);
        let (enc_key, mac_key) = crypto::derive_keys(passphrase, &salt, ITERATIONS);

        let mut tmp_name = file.clone().into_os_string();
        tmp_name.push(".tmp");
        let tmp = PathBuf::from(tmp_name);
        let f = File::create(&tmp).map_err(|_| Error::new(format!("สร้างไฟล์ไม่ได้: {}", file.display())))?;
        let mut out = BufWriter::new(f);
        // Written now with placeholders, and rewritten with the final values in `finish`.
        out.write_all(&header).map_err(|_| Error::new(format!("สร้างไฟล์ไม่ได้: {}", file.display())))?;
        let mut nonce = [0u8; 12];
        nonce.copy_from_slice(&header[24..36]);
        Ok(PackWriter {
            file,
            tmp,
            out: Some(out),
            header,
            nonce,
            enc_key,
            mac_key,
            pos: HEADER_SIZE as u64,
            index_bytes: 0,
            entries: Vec::new(),
            finished: false,
        })
    }

    pub fn count(&self) -> usize {
        self.entries.len()
    }

    pub fn add(&mut self, song: &PackSong) -> Result<()> {
        // Each table row costs its strings, three length prefixes, offset, size and tag.
        let row_bytes = 12
            + song.title.len() as u64
            + song.artist.len() as u64
            + song.key.len() as u64
            + 8
            + 4
            + TAG_SIZE as u64;
        if self.entries.len() >= MAX_ENTRIES || self.index_bytes + row_bytes > MAX_INDEX_SIZE {
            return Err(Error::new("เพลงมากเกินไปสำหรับไฟล์ .ckr เดียว แบ่งเป็นหลายไฟล์"));
        }
        let mut blob = pack_record(song)?;
        crypto::chacha_xor(&self.enc_key, &self.nonce, self.pos, &mut blob)?;
        let tag = crypto::record_tag(&self.mac_key, self.pos, &blob);
        let out = self.out.as_mut().ok_or_else(|| Error::new("ไฟล์ .ckr ยังไม่ได้เปิด"))?;
        out.write_all(&blob)
            .map_err(|_| Error::new("เขียนไฟล์ .ckr ไม่สำเร็จ (ตรวจสอบพื้นที่ว่างในดิสก์)"))?;
        self.entries.push(PackEntry {
            title: song.title.clone(),
            artist: song.artist.clone(),
            key: song.key.clone(),
            offset: self.pos,
            size: blob.len() as u32,
            tag,
        });
        self.pos += blob.len() as u64;
        self.index_bytes += row_bytes;
        Ok(())
    }

    pub fn finish(&mut self) -> Result<()> {
        if self.entries.is_empty() {
            return Err(Error::new("ไม่มีเพลงให้แพ็ก"));
        }
        let write_err = || Error::new("เขียนไฟล์ .ckr ไม่สำเร็จ (ตรวจสอบพื้นที่ว่างในดิสก์)");
        let mut index = pack_index(&self.entries);
        let index_offset = self.pos;
        crypto::chacha_xor(&self.enc_key, &self.nonce, index_offset, &mut index)?;

        self.header[36..44].copy_from_slice(&index_offset.to_le_bytes());
        self.header[44..48].copy_from_slice(&(index.len() as u32).to_le_bytes());
        let tag = crypto::hmac(&self.mac_key, &[self.header.as_slice(), index.as_slice()]);

        let mut out = self.out.take().ok_or_else(write_err)?;
        out.write_all(&index).and_then(|_| out.write_all(&tag)).map_err(|_| write_err())?;
        out.seek(SeekFrom::Start(0)).and_then(|_| out.write_all(&self.header)).map_err(|_| write_err())?;
        out.flush().map_err(|_| write_err())?;
        drop(out);

        fs::rename(&self.tmp, &self.file).map_err(|_| Error::new(format!("บันทึกไฟล์ .ckr ไม่ได้: {}", self.file.display())))?;
        self.finished = true;
        Ok(())
    }
}

impl Drop for PackWriter {
    fn drop(&mut self) {
        if !self.finished {
            self.out = None;
            let _ = fs::remove_file(&self.tmp);
        }
        self.enc_key.zeroize();
        self.mac_key.zeroize();
    }
}

/// Writes all songs into one pack file.
pub fn write_pack(file: impl AsRef<Path>, passphrase: &str, songs: &[PackSong]) -> Result<()> {
    if songs.is_empty() {
        return Err(Error::new("ไม่มีเพลงให้แพ็ก"));
    }
    let mut writer = PackWriter::open(file, passphrase)?;
    for s in songs {
        writer.add(s)?;
    }
    writer.finish()
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct PackBuildResult {
    pub packed: usize,
    /// `"<file name>: <reason>"` for each source that could not be packed.
    pub skipped: Vec<String>,
}

/// Packs source files (see `pack_song_from_file`) into one pack, one at a time. `progress` gets the
/// number of sources done after each one. Only a failure of the pack itself is an error.
pub fn pack_sources(
    sources: &[PathBuf],
    file: impl AsRef<Path>,
    passphrase: &str,
    mut progress: impl FnMut(usize),
) -> Result<PackBuildResult> {
    let mut writer = PackWriter::open(file, passphrase)?;
    let mut result = PackBuildResult::default();
    for (done, src) in sources.iter().enumerate() {
        match pack_song_from_file(src) {
            Ok(song) => writer.add(&song)?, // a failure here is the pack's own (disk, size limit)
            Err(e) => {
                let name = src.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                result.skipped.push(format!("{name}: {e}"));
            }
        }
        progress(done + 1);
    }
    result.packed = writer.count();
    writer.finish()?;
    Ok(result)
}

/// Reads a source song (`.mid` / `.kar` with its NCN `.lyr` and `.cur` siblings) into the form a pack stores.
pub fn pack_song_from_file(path: &Path) -> Result<PackSong> {
    let not_read = || Error::new(format!("อ่านเพลงไม่ได้: {}", path.display()));
    let bytes = fs::read(path).map_err(|_| Error::new(format!("เปิดไฟล์ไม่ได้: {}", path.display())))?;
    let info = library::read_song_info(path).ok_or_else(not_read)?;
    // Each sibling is taken when it exists, even if the other one is missing.
    let sib = song::ncn_siblings(path);
    let lyric = sib.lyr.and_then(|p| fs::read(p).ok()).unwrap_or_default();
    let cursor = sib.cur.and_then(|p| fs::read(p).ok()).unwrap_or_default();
    Ok(PackSong { title: info.title, artist: info.artist, key: info.key, midi: bytes, lyric, cursor })
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct UnpackResult {
    pub written: usize,
    pub failures: Vec<String>,
}

/// A file name from a song title: characters a file name cannot hold become `_`.
fn safe_name(title: &str) -> String {
    const RESERVED: &[u8] = b"/\\:*?\"<>|";
    let mut out: Vec<u8> = title
        .bytes()
        .map(|c| if c < 0x20 || RESERVED.contains(&c) { b'_' } else { c })
        .collect();
    let trimmed = {
        let a = out.iter().position(|&c| c != b' ' && c != b'.');
        let b = out.iter().rposition(|&c| c != b' ' && c != b'.');
        match (a, b) {
            (Some(a), Some(b)) => out[a..=b].to_vec(),
            _ => Vec::new(),
        }
    };
    out = trimmed;
    if out.len() > 120 {
        // Cut on a character boundary.
        out.truncate(120);
        while out.last().is_some_and(|&c| c & 0xC0 == 0x80) {
            out.pop();
        }
        if out.last().is_some_and(|&c| c >= 0xC0) {
            out.pop();
        }
    }
    if out.is_empty() {
        return "song".to_string();
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// A base name whose `.mid`, `.lyr` and `.cur` files are all free; repeats get ` (2)`, ` (3)`, and so on.
fn free_base(root: &Path, title: &str) -> String {
    let base = safe_name(title);
    let taken = |name: &str| {
        root.join("Song").join(format!("{name}.mid")).exists()
            || root.join("Lyrics").join(format!("{name}.lyr")).exists()
            || root.join("Cursor").join(format!("{name}.cur")).exists()
    };
    let mut name = base.clone();
    let mut n = 2;
    while taken(&name) {
        name = format!("{base} ({n})");
        n += 1;
    }
    name
}

/// Writes every song of a pack under `root` as an NCN tree (`Song/NAME.mid`, `Lyrics/NAME.lyr`,
/// `Cursor/NAME.cur`), so the folder reads like any song folder and can be packed again.
pub fn unpack_pack(pack_file: impl AsRef<Path>, passphrase: &str, root: impl AsRef<Path>) -> Result<UnpackResult> {
    let reader = PackReader::open(pack_file, passphrase)?;
    let root = root.as_ref();
    fs::create_dir_all(root.join("Song"))
        .map_err(|_| Error::new(format!("สร้างโฟลเดอร์ไม่ได้: {}", root.display())))?;

    let mut result = UnpackResult::default();
    for i in 0..reader.entries().len() {
        let song = match reader.read(i) {
            Ok(song) => song,
            Err(e) => {
                result.failures.push(e.message().to_string());
                continue;
            }
        };
        let name = free_base(root, &song.title);
        let mut ok = fs::write(root.join("Song").join(format!("{name}.mid")), &song.midi).is_ok();
        if ok && !song.lyric.is_empty() {
            ok = fs::create_dir_all(root.join("Lyrics")).is_ok()
                && fs::write(root.join("Lyrics").join(format!("{name}.lyr")), &song.lyric).is_ok();
        }
        if ok && !song.cursor.is_empty() {
            ok = fs::create_dir_all(root.join("Cursor")).is_ok()
                && fs::write(root.join("Cursor").join(format!("{name}.cur")), &song.cursor).is_ok();
        }
        if ok {
            result.written += 1;
        } else {
            result.failures.push(format!("เขียนไฟล์ของ \"{}\" ไม่ได้", song.title));
        }
    }
    Ok(result)
}

/// Songs inside a pack are addressed as `<pack file>#<index>`, with the index from 0.
pub fn pack_song_path(pack_file: &str, index: usize) -> String {
    format!("{pack_file}#{index}")
}

/// Splits `<pack>.ckr#<index>` into the pack path and the index. Returns `None` for any other path.
pub fn split_pack_song_path(path: &str) -> Option<(String, usize)> {
    let hash = path.rfind('#')?;
    let digits = &path[hash + 1..];
    if digits.is_empty() || digits.len() > 9 || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let base = &path[..hash];
    let is_ckr = Path::new(base).extension().is_some_and(|e| e.eq_ignore_ascii_case("ckr"));
    if !is_ckr {
        return None;
    }
    Some((base.to_string(), digits.parse().ok()?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delta_coding_round_trips() {
        let table: Vec<u8> = [0u16, 480, 481, 300, 65535, 2, 9000, 9000]
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect();
        let coded = delta_encode(&table, 2);
        assert_eq!(delta_decode(&coded, 2, table.len()).as_deref(), Some(table.as_slice()));

        let wide: Vec<u8> = [0u32, 70_000, 5, 4_000_000_000].iter().flat_map(|v| v.to_le_bytes()).collect();
        let coded = delta_encode(&wide, 4);
        assert_eq!(delta_decode(&coded, 4, wide.len()).as_deref(), Some(wide.as_slice()));
    }

    #[test]
    fn pack_song_paths_split_back() {
        assert_eq!(split_pack_song_path("/x/My Songs.ckr#3"), Some(("/x/My Songs.ckr".into(), 3)));
        assert_eq!(split_pack_song_path("/x/song.mid#3"), None);
        assert_eq!(split_pack_song_path("pack.ckr#"), None);
        assert_eq!(pack_song_path("a.ckr", 0), "a.ckr#0");
    }

    #[test]
    fn safe_names_drop_reserved_characters() {
        assert_eq!(safe_name("a/b:c*"), "a_b_c_");
        assert_eq!(safe_name(" .. "), "song");
    }

    #[test]
    fn pack_round_trips_and_rejects_a_wrong_key() {
        let dir = std::env::temp_dir().join(format!("ckr-test-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let file = dir.join("t.ckr");
        let songs = vec![
            PackSong {
                title: "คิดถึง".into(),
                artist: "ศิลปิน".into(),
                key: "Em".into(),
                midi: [1u8, 2, 3, 4, 5].repeat(100),
                lyric: "บรรทัดแรก\nบรรทัดสอง".as_bytes().to_vec(),
                cursor: [0u8, 0, 10, 0, 30, 0, 255, 255].to_vec(),
            },
            PackSong { title: "empty parts".into(), midi: vec![9; 40], ..Default::default() },
        ];
        write_pack(&file, "secret", &songs).unwrap();

        let reader = PackReader::open(&file, "secret").unwrap();
        assert_eq!(reader.entries().len(), 2);
        assert_eq!(reader.read(0).unwrap(), songs[0]);
        assert_eq!(reader.read(1).unwrap(), songs[1]);

        let err = PackReader::open(&file, "wrong").err().expect("wrong key must fail");
        assert_eq!(err.message(), "คีย์ไม่ถูกต้อง หรือไฟล์ .ckr เสียหาย");
        fs::remove_dir_all(&dir).unwrap();
    }
}
