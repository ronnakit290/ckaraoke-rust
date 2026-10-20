//! SoundFont 2 loading. Port of tsf_load, tsf_load_presets and their helpers in tsf.h.
//!
//! Like the C build, this reads uncompressed 16-bit samples only. Ogg Vorbis samples (SF3, .sfo) are
//! not supported, and modulators are ignored.

use crate::math::timecents_to_secs_f;
use crate::region::{self, Amount, Preset, Region};

const GEN_INSTRUMENT: u16 = 41;
const GEN_SAMPLE_ID: u16 = 53;

/// What a loaded file contributes to the synthesizer.
pub(crate) struct Font {
    pub presets: Vec<Preset>,
    pub regions: Vec<Region>,
    pub samples: Vec<f32>,
}

#[derive(Clone, Copy)]
struct PresetHeader {
    name: [u8; 20],
    preset: u16,
    bank: u16,
    bag_ndx: u16,
}

#[derive(Clone, Copy)]
struct Bag {
    gen_ndx: u16,
}

#[derive(Clone, Copy)]
struct Gen {
    oper: u16,
    amount: Amount,
}

#[derive(Clone, Copy)]
struct Instrument {
    bag_ndx: u16,
}

#[derive(Clone, Copy)]
struct SampleHeader {
    start: u32,
    end: u32,
    start_loop: u32,
    end_loop: u32,
    sample_rate: u32,
    original_pitch: u8,
    pitch_correction: i8,
}

#[derive(Default)]
struct Hydra {
    phdrs: Option<Vec<PresetHeader>>,
    pbags: Option<Vec<Bag>>,
    pmods: bool,
    pgens: Option<Vec<Gen>>,
    insts: Option<Vec<Instrument>>,
    ibags: Option<Vec<Bag>>,
    imods: bool,
    igens: Option<Vec<Gen>>,
    shdrs: Option<Vec<SampleHeader>>,
}

fn u16_at(d: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([d[o], d[o + 1]])
}

fn u32_at(d: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([d[o], d[o + 1], d[o + 2], d[o + 3]])
}

fn fail(msg: &str) -> String {
    format!("SoundFont ไม่ถูกต้อง: {msg}")
}

/// One RIFF chunk header at `pos`, inside `end`: its id, its body start and its body size.
/// Returns `None` where the C reader stops: a short header, a bad id, or a body that overruns the parent.
fn chunk_at(data: &[u8], pos: usize, end: usize) -> Option<([u8; 4], usize, usize)> {
    if end.saturating_sub(pos) < 8 {
        return None;
    }
    let id = [data[pos], data[pos + 1], data[pos + 2], data[pos + 3]];
    if id[0] <= b' ' || id[0] >= b'z' {
        return None;
    }
    let size = u32_at(data, pos + 4) as usize;
    let body = pos + 8;
    if body + size > end {
        return None;
    }
    Some((id, body, size))
}

/// Reads the records of one `pdta` table. A chunk whose size is not a whole number of records is skipped,
/// as the C loader does, which leaves that table missing.
fn read_table<T>(data: &[u8], body: usize, size: usize, rec: usize, parse: impl Fn(&[u8]) -> T) -> Option<Vec<T>> {
    if !size.is_multiple_of(rec) {
        return None;
    }
    Some((0..size / rec).map(|i| parse(&data[body + i * rec..body + (i + 1) * rec])).collect())
}

fn parse_phdr(r: &[u8]) -> PresetHeader {
    let mut name = [0u8; 20];
    name.copy_from_slice(&r[0..20]);
    PresetHeader { name, preset: u16_at(r, 20), bank: u16_at(r, 22), bag_ndx: u16_at(r, 24) }
}

fn parse_bag(r: &[u8]) -> Bag {
    Bag { gen_ndx: u16_at(r, 0) }
}

fn parse_gen(r: &[u8]) -> Gen {
    Gen { oper: u16_at(r, 0), amount: Amount(u16_at(r, 2)) }
}

fn parse_inst(r: &[u8]) -> Instrument {
    Instrument { bag_ndx: u16_at(r, 20) }
}

fn parse_shdr(r: &[u8]) -> SampleHeader {
    SampleHeader {
        start: u32_at(r, 20),
        end: u32_at(r, 24),
        start_loop: u32_at(r, 28),
        end_loop: u32_at(r, 32),
        sample_rate: u32_at(r, 36),
        original_pitch: r[40],
        pitch_correction: r[41] as i8,
    }
}

/// Parses one `pdta` sub-chunk into the hydra, if it is one of the nine tables with the right record size.
fn read_pdta_chunk(data: &[u8], id: &[u8; 4], body: usize, size: usize, h: &mut Hydra) {
    match id {
        b"phdr" => h.phdrs = read_table(data, body, size, 38, parse_phdr),
        b"pbag" => h.pbags = read_table(data, body, size, 4, parse_bag),
        b"pmod" => h.pmods = size.is_multiple_of(10),
        b"pgen" => h.pgens = read_table(data, body, size, 4, parse_gen),
        b"inst" => h.insts = read_table(data, body, size, 22, parse_inst),
        b"ibag" => h.ibags = read_table(data, body, size, 4, parse_bag),
        b"imod" => h.imods = size.is_multiple_of(10),
        b"igen" => h.igens = read_table(data, body, size, 4, parse_gen),
        b"shdr" => h.shdrs = read_table(data, body, size, 46, parse_shdr),
        _ => {}
    }
}

/// Loads a SoundFont 2 file from memory.
pub(crate) fn load(data: &[u8]) -> Result<Font, String> {
    if data.len() < 12 || &data[0..4] != b"RIFF" || &data[8..12] != b"sfbk" {
        return Err(fail("ไม่ใช่ไฟล์ sfbk"));
    }
    let riff_end = (8 + u32_at(data, 4) as usize).min(data.len());
    let mut hydra = Hydra::default();
    let mut samples: Option<Vec<f32>> = None;

    let mut pos = 12;
    while let Some((id, body, size)) = chunk_at(data, pos, riff_end) {
        let body_end = body + size;
        if &id == b"LIST" && size >= 4 {
            let list_end = body_end;
            let kind = [data[body], data[body + 1], data[body + 2], data[body + 3]];
            let mut p = body + 4;
            while let Some((cid, cbody, csize)) = chunk_at(data, p, list_end) {
                if &kind == b"pdta" {
                    read_pdta_chunk(data, &cid, cbody, csize, &mut hydra);
                } else if &kind == b"sdta" && &cid == b"smpl" && samples.is_none() && csize >= 2 {
                    // 16-bit little-endian PCM, converted the way the C loader does: double division, then float.
                    let decoded = (0..csize / 2)
                        .map(|i| (f64::from(u16_at(data, cbody + 2 * i) as i16) / 32767.0) as f32)
                        .collect();
                    samples = Some(decoded);
                }
                p = cbody + csize;
            }
        }
        pos = body_end;
    }

    let (Some(phdrs), Some(pbags), Some(pgens), Some(insts), Some(ibags), Some(igens), Some(shdrs)) = (
        hydra.phdrs.as_ref(),
        hydra.pbags.as_ref(),
        hydra.pgens.as_ref(),
        hydra.insts.as_ref(),
        hydra.ibags.as_ref(),
        hydra.igens.as_ref(),
        hydra.shdrs.as_ref(),
    ) else {
        return Err(fail("ตารางข้อมูลไม่ครบ"));
    };
    if !hydra.pmods || !hydra.imods {
        return Err(fail("ตารางข้อมูลไม่ครบ"));
    }
    let samples = samples.filter(|s| !s.is_empty()).ok_or_else(|| fail("ไม่มีข้อมูลเสียง"))?;
    let (presets, regions) = build_presets(phdrs, pbags, pgens, insts, ibags, igens, shdrs, samples.len() as u32)?;
    Ok(Font { presets, regions, samples })
}

/// The slice `items[start..end]` of a table, or an error if the indices do not fit.
fn span<T>(items: &[T], start: u16, end: u16) -> Result<&[T], String> {
    let (s, e) = (start as usize, end as usize);
    if s > e || e > items.len() {
        return Err(fail("ดัชนีตารางข้อมูลผิด"));
    }
    Ok(&items[s..e])
}

fn fixed_name(name: &[u8; 20]) -> String {
    let nul = name.iter().position(|&b| b == 0).unwrap_or(name.len());
    String::from_utf8_lossy(&name[..nul]).into_owned()
}

/// Builds the presets (sorted by bank and number, as tsf does) and their zones.
#[allow(clippy::too_many_arguments)]
fn build_presets(
    phdrs: &[PresetHeader],
    pbags: &[Bag],
    pgens: &[Gen],
    insts: &[Instrument],
    ibags: &[Bag],
    igens: &[Gen],
    shdrs: &[SampleHeader],
    sample_count: u32,
) -> Result<(Vec<Preset>, Vec<Region>), String> {
    if phdrs.len() < 2 {
        return Err(fail("ไม่มี preset"));
    }
    // The last header is the terminator record, so there are one fewer presets than headers.
    let n = phdrs.len() - 1;
    let mut slots: Vec<Option<(Preset, Vec<Region>)>> = (0..n).map(|_| None).collect();

    for (pi, ph) in phdrs[..n].iter().enumerate() {
        let mut sorted_index = 0usize;
        for (oi, oh) in phdrs[..n].iter().enumerate() {
            if oi == pi || oh.bank > ph.bank {
                continue;
            } else if oh.bank < ph.bank {
                sorted_index += 1;
            } else if oh.preset > ph.preset {
                continue;
            } else if oh.preset < ph.preset || oi < pi {
                // An equal preset number sorts by file position.
                sorted_index += 1;
            }
        }
        if sorted_index >= n {
            return Err(fail("preset ซ้ำ"));
        }

        let mut regions: Vec<Region> = Vec::new();
        let next_ph = &phdrs[pi + 1];
        let mut global = Region::cleared(true);
        for (bi_rel, pbag) in span(pbags, ph.bag_ndx, next_ph.bag_ndx)?.iter().enumerate() {
            let is_first_bag = bi_rel == 0;
            let bi = ph.bag_ndx as usize + bi_rel;
            let next_gen = pbags.get(bi + 1).ok_or_else(|| fail("ดัชนีตารางข้อมูลผิด"))?;
            let mut preset_region = global;
            let mut had_gen_instrument = false;

            for pg in span(pgens, pbag.gen_ndx, next_gen.gen_ndx)? {
                if pg.oper != GEN_INSTRUMENT {
                    region::set(&mut preset_region, pg.oper, pg.amount);
                    continue;
                }
                let which = pg.amount.word() as usize;
                if which >= insts.len() {
                    continue;
                }
                let mut inst_region = Region::cleared(false);
                let inst = insts[which];
                let next_inst = insts.get(which + 1).ok_or_else(|| fail("ดัชนีตารางข้อมูลผิด"))?;
                for (ib_rel, ibag) in span(ibags, inst.bag_ndx, next_inst.bag_ndx)?.iter().enumerate() {
                    let ibi = inst.bag_ndx as usize + ib_rel;
                    let next_igen = ibags.get(ibi + 1).ok_or_else(|| fail("ดัชนีตารางข้อมูลผิด"))?;
                    let mut zone = inst_region;
                    let mut had_sample_id = false;
                    for ig in span(igens, ibag.gen_ndx, next_igen.gen_ndx)? {
                        if ig.oper != GEN_SAMPLE_ID {
                            region::set(&mut zone, ig.oper, ig.amount);
                            continue;
                        }
                        let shdr_index = ig.amount.word() as usize;
                        let sh = *shdrs.get(shdr_index).ok_or_else(|| fail("ดัชนีตัวอย่างผิด"))?;
                        // The preset's key and velocity ranges filter the zones.
                        if zone.hikey < preset_region.lokey || zone.lokey > preset_region.hikey {
                            continue;
                        }
                        if zone.hivel < preset_region.lovel || zone.lovel > preset_region.hivel {
                            continue;
                        }
                        zone.lokey = zone.lokey.max(preset_region.lokey);
                        zone.hikey = zone.hikey.min(preset_region.hikey);
                        zone.lovel = zone.lovel.max(preset_region.lovel);
                        zone.hivel = zone.hivel.min(preset_region.hivel);

                        region::merge(&mut zone, &preset_region);
                        envelope_to_secs(&mut zone.ampenv, true);
                        envelope_to_secs(&mut zone.modenv, false);
                        zone.delay_mod_lfo = if zone.delay_mod_lfo < -11950.0 { 0.0 } else { timecents_to_secs_f(zone.delay_mod_lfo) };
                        zone.delay_vib_lfo = if zone.delay_vib_lfo < -11950.0 { 0.0 } else { timecents_to_secs_f(zone.delay_vib_lfo) };

                        // Sample positions: the zone's offsets are relative to the sample's own start.
                        zone.offset = zone.offset.wrapping_add(sh.start);
                        zone.end = zone.end.wrapping_add(sh.end);
                        zone.loop_start = zone.loop_start.wrapping_add(sh.start_loop);
                        zone.loop_end = zone.loop_end.wrapping_add(sh.end_loop);
                        if sh.end_loop > 0 {
                            zone.loop_end = zone.loop_end.wrapping_sub(1);
                        }
                        if zone.loop_end > sample_count {
                            zone.loop_end = sample_count;
                        }
                        if zone.pitch_keycenter == -1 {
                            zone.pitch_keycenter = i32::from(sh.original_pitch);
                        }
                        zone.tune = zone.tune.wrapping_add(i32::from(sh.pitch_correction));
                        zone.sample_rate = sh.sample_rate;
                        if zone.end != 0 && zone.end < sample_count {
                            zone.end += 1;
                        } else {
                            zone.end = sample_count;
                        }

                        regions.push(zone);
                        had_sample_id = true;
                    }
                    // The instrument's global zone (its first zone, with no sample) applies to the others.
                    if ib_rel == 0 && !had_sample_id {
                        inst_region = zone;
                    }
                }
                had_gen_instrument = true;
            }

            if is_first_bag && !had_gen_instrument {
                global = preset_region;
            }
        }

        let preset = Preset {
            name: fixed_name(&ph.name),
            bank: ph.bank,
            preset: ph.preset,
            region_start: 0,
            region_num: regions.len(),
        };
        slots[sorted_index] = Some((preset, regions));
    }

    let mut presets = Vec::with_capacity(n);
    let mut all_regions = Vec::new();
    for slot in slots {
        let (mut preset, regions) = slot.ok_or_else(|| fail("preset ซ้ำ"))?;
        preset.region_start = all_regions.len();
        all_regions.extend(regions);
        presets.push(preset);
    }
    Ok((presets, all_regions))
}

fn envelope_to_secs(p: &mut region::Envelope, sustain_is_gain: bool) {
    region::envelope_to_secs(p, sustain_is_gain);
}
