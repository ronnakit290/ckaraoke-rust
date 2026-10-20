"""Generates a small SoundFont 2 file and a MIDI file that together exercise the synthesizer:
looping and one-shot samples, volume and modulation envelopes, a low-pass filter, vibrato (LFO), pan,
tuning, velocity layers, a drum kit on channel 10 with a piece per key, global zones, pitch bend with a
range change, sustain pedal, expression, a tempo change and a note-on with velocity 0.

    make_synth_fixtures.py <out-dir>
"""
import math
import pathlib
import random
import struct
import sys

OUT = pathlib.Path(sys.argv[1])
OUT.mkdir(parents=True, exist_ok=True)


def pad_name(s):
    return s.encode("ascii")[:19].ljust(20, b"\0")


def chunk(cid, body):
    if len(body) % 2:
        body += b"\0"
    return cid + struct.pack("<I", len(body)) + body


def lst(kind, *chunks):
    body = kind + b"".join(chunks)
    return b"LIST" + struct.pack("<I", len(body)) + body


# ---------------------------------------------------------------- samples
samples = []  # int16 values
shdrs = []


def add_sample(name, data, loop=None, root=60, rate=44100, pitch_corr=0):
    start = len(samples)
    samples.extend(data)
    end = len(samples)
    if loop:
        ls, le = loop
        sl, sle = start + ls, start + le
    else:
        sl, sle = start, start
    shdrs.append(dict(name=name, start=start, end=end, sl=sl, sle=sle, rate=rate, root=root, corr=pitch_corr,
                      type=1 if loop else 2))
    return len(shdrs) - 1


def tone(period, n, harm=(1.0,), amp=12000):
    out = []
    for i in range(n):
        v = sum(a * math.sin(2 * math.pi * k * i / period) for k, a in zip(range(1, len(harm) + 1), harm))
        out.append(max(-32768, min(32767, int(round(v * amp / sum(harm))))))
    return out


rnd = random.Random(11)
s_sine = add_sample("sine", tone(256, 2048), loop=(0, 2048), root=60)
s_saw = add_sample("saw", tone(137.5, 1500, harm=(1, 0.5, 0.33, 0.25, 0.2, 0.16)), loop=(100, 1400), root=72, pitch_corr=12)
s_noise = add_sample("noise", [rnd.randrange(-9000, 9000) for _ in range(3000)], root=36)
s_pad = add_sample("pad", tone(333, 2400, harm=(1, 0.7, 0.2)), loop=(10, 2390), root=48)
s_hit = add_sample("hit", [int(rnd.randrange(-6000, 6000) * (1 - i / 900)) for i in range(900)], root=38)
add_sample("padding", [0] * 64)  # never used: keeps the last real sample off the end of the data

# ---------------------------------------------------------------- generators and zones
# Generator operators (SF2 spec): 5 vibrato/modLfo to pitch, 6 vib, 8 filter fc, 9 Q, 11 modEnv to fc,
# 13 modLfo to volume, 17 pan, 21/22 mod LFO delay/freq, 23/24 vib LFO delay/freq, 25-30 mod env,
# 33-38 vol env, 43 key range, 44 velocity range, 41 instrument, 48 attenuation, 51 coarse tune,
# 52 fine tune, 53 sample id, 54 sample modes, 56 scale tuning, 57 exclusive class, 58 root key.
ENV_VOL = dict(delay=33, attack=34, hold=35, decay=36, sustain=37, release=38)
ENV_MOD = dict(delay=25, attack=26, hold=27, decay=28, sustain=29, release=30)


def g(oper, amount):
    return (oper, amount & 0xFFFF)


def rng(lo, hi):
    return (lo & 0xFF) | ((hi & 0xFF) << 8)


def tc(seconds):
    return int(round(1200 * math.log2(seconds)))


instruments = []  # (name, [zones]) zone = list of gens (last gen is sample id)
presets = []      # (name, bank, preset, [zones]) zone = (gens, instrument index or None)


def inst(name, zones):
    instruments.append((name, zones))
    return len(instruments) - 1


def preset(name, bank, number, zones):
    presets.append((name, bank, number, zones))


# Instrument 0: sine with a volume envelope and a low-pass filter.
i_sine = inst("sine", [
    [g(43, rng(0, 127)), g(ENV_VOL["attack"], tc(0.01)), g(ENV_VOL["decay"], tc(0.5)),
     g(ENV_VOL["sustain"], 44), g(ENV_VOL["release"], tc(0.3)), g(8, 8000), g(9, 120),
     g(54, 1), g(53, s_sine)],
])
# Instrument 1: saw with a loop, pan, tuning and a modulation envelope driving the filter.
i_saw = inst("saw", [
    [g(43, rng(0, 127)), g(ENV_VOL["attack"], tc(0.005)), g(ENV_VOL["sustain"], 20),
     g(ENV_VOL["release"], tc(0.2)), g(ENV_MOD["attack"], tc(0.05)), g(ENV_MOD["decay"], tc(0.4)),
     g(ENV_MOD["sustain"], 300), g(11, 2400), g(8, 3000), g(17, 300), g(52, 7), g(51, 0),
     g(54, 1), g(53, s_saw)],
])
# Instrument 2: drum noise, one piece per key (overriding root key = the key).
drum_keys = [35, 36, 38, 42, 46, 49]
i_drum = inst("drums", [
    [g(43, rng(k, k)), g(58, k), g(17, int((k - 40) * 20)), g(48, 20 + (k % 5) * 6), g(54, 0),
     g(ENV_VOL["release"], tc(0.15)), g(53, s_noise if k % 2 else s_hit)]
    for k in drum_keys
])
# Instrument 3: pad with vibrato and a delayed LFO.
i_pad = inst("pad", [
    [g(43, rng(0, 127)), g(ENV_VOL["attack"], tc(0.3)), g(ENV_VOL["release"], tc(0.8)), g(ENV_VOL["sustain"], 10),
     g(5, 0), g(6, 40), g(23, tc(0.2)), g(24, 500), g(21, tc(0.1)), g(22, 300), g(13, 40), g(8, 9000),
     g(54, 1), g(53, s_pad)],
])

# Presets. Bank 0 presets are the melodic ones; bank 128 is the drum kit.
# Preset 0: a global zone (attenuation for all zones), then two key ranges.
preset("Sine Keys", 0, 0, [
    ([g(48, 15)], None),  # global zone
    ([g(43, rng(0, 63)), g(41, i_sine)], i_sine),
    ([g(43, rng(64, 127)), g(41, i_saw)], i_saw),
])
# Preset 1 (program 48): pad.
preset("Pad", 0, 48, [([g(41, i_pad)], i_pad)])
# Preset 2 (program 5): velocity layers of the saw and the sine.
preset("Lead", 0, 5, [
    ([g(44, rng(0, 63)), g(41, i_sine)], i_sine),
    ([g(44, rng(64, 127)), g(41, i_saw)], i_saw),
])
# Preset 3 (drum kit, bank 128): every drum piece on its own zone.
preset("Drum Kit", 128, 0, [([g(41, i_drum)], i_drum)])

# The preset generator list needs the instrument generator to refer to instrument indices. Rebuild it.
pbag_list, pgen_list = [], []
phdr_list = []
for name, bank, number, zones in presets:
    phdr_list.append((name, number, bank, len(pbag_list)))
    for gens, instr in zones:
        pbag_list.append((len(pgen_list), 0))
        for oper, amount in gens:
            pgen_list.append((oper, amount))
        if instr is not None:
            pgen_list.append((41, instr))
phdr_list.append(("EOP", 0, 0, len(pbag_list)))
pbag_list.append((len(pgen_list), 0))
pgen_list.append((0, 0))

# Instrument bags and generators, with the sample id as the last generator of each zone.
ibag_list, igen_list, inst_bags = [], [], []
for name, zones in instruments:
    inst_bags.append(len(ibag_list))
    for zone in zones:
        ibag_list.append((len(igen_list), 0))
        igen_list.extend(zone)
ibag_list.append((len(igen_list), 0))
igen_list.append((0, 0))
inst_recs = [(name, inst_bags[i]) for i, (name, _z) in enumerate(instruments)]
inst_recs.append(("EOI", len(ibag_list) - 1))

# Sample headers.
shdr_recs = []
for s in shdrs:
    shdr_recs.append((s["name"], s["start"], s["end"], s["sl"], s["sle"], s["rate"], s["root"], s["corr"], 0, s["type"]))
shdr_recs.append(("EOS", 0, 0, 0, 0, 0, 0, 0, 0, 0))


def rec_phdr(name, number, bank, bag):
    return pad_name(name) + struct.pack("<HHHIII", number, bank, bag, 0, 0, 0)


def rec_inst(name, bag):
    return pad_name(name) + struct.pack("<H", bag)


def rec_shdr(name, start, end, sl, sle, rate, root, corr, link, typ):
    return pad_name(name) + struct.pack("<IIIIIBbHH", start, end, sl, sle, rate, root, corr, link, typ)


pdta_body = [
    chunk(b"phdr", b"".join(rec_phdr(*p) for p in phdr_list)),
    chunk(b"pbag", b"".join(struct.pack("<HH", *b) for b in pbag_list)),
    chunk(b"pmod", bytes(10)),
    chunk(b"pgen", b"".join(struct.pack("<HH", o, a & 0xFFFF) for o, a in pgen_list)),
    chunk(b"inst", b"".join(rec_inst(*i) for i in inst_recs)),
    chunk(b"ibag", b"".join(struct.pack("<HH", *b) for b in ibag_list)),
    chunk(b"imod", bytes(10)),
    chunk(b"igen", b"".join(struct.pack("<HH", o, a & 0xFFFF) for o, a in igen_list)),
    chunk(b"shdr", b"".join(rec_shdr(*s) for s in shdr_recs)),
]
smpl = struct.pack("<%dh" % len(samples), *samples)
info = [chunk(b"ifil", struct.pack("<HH", 2, 1)), chunk(b"isng", b"EMU8000\0"), chunk(b"INAM", b"CKaraoke Test\0")]
body = b"sfbk" + lst(b"INFO", *info) + lst(b"sdta", chunk(b"smpl", smpl)) + lst(b"pdta", *pdta_body)
(OUT / "test.sf2").write_bytes(b"RIFF" + struct.pack("<I", len(body)) + body)

# ---------------------------------------------------------------- MIDI
def vlq(n):
    out = [n & 0x7F]
    n >>= 7
    while n:
        out.append((n & 0x7F) | 0x80)
        n >>= 7
    return bytes(reversed(out))


events = []  # (tick, bytes)


def ev(tick, *data):
    events.append((tick, bytes(data)))


def meta(tick, kind, data):
    events.append((tick, bytes([0xFF, kind]) + vlq(len(data)) + data))


D = 480  # ticks per quarter
meta(0, 0x51, (500000).to_bytes(3, "big"))
meta(0, 0x03, b"synth test")
# Channel 0: piano-ish sine, volume and pan, notes with overlap and a sustain pedal.
ev(0, 0xB0, 7, 100); ev(0, 0xB0, 10, 40); ev(0, 0xC0, 0)
for i, n in enumerate([60, 64, 67, 72, 71, 67]):
    ev(D * i, 0x90, n, 70 + i * 9)
    ev(D * i + D, 0x80, n, 0)
ev(D * 2, 0xB0, 64, 127); ev(D * 4, 0x90, 55, 90); ev(D * 5, 0xB0, 64, 0); ev(D * 6, 0x80, 55, 0)
# Channel 1: pad (program 48) with pitch bend, expression and a long chord.
ev(0, 0xC1, 48); ev(0, 0xB1, 7, 90); ev(0, 0xB1, 11, 70)
for n in (48, 55, 60):
    ev(D * 6, 0x91, n, 80)
for k in range(12):
    ev(D * 6 + k * 120, 0xE1, 0, 64 + int(63 * math.sin(k / 2.0)))  # pitch bend, 14-bit 0..127 lsb, msb 64 center
ev(D * 6 + 12 * 120, 0xE1, 0, 64)
for n in (48, 55, 60):
    ev(D * 6 + 1900, 0x81, n, 0)
# Channel 2: lead layers by velocity, with a velocity-0 note-on (acts as note-off in MIDI).
ev(0, 0xC2, 5)
for i, (n, vel) in enumerate([(64, 30), (67, 100), (72, 127), (60, 64)]):
    ev(D * 7 + D * i // 2, 0x92, n, vel)
    ev(D * 7 + D * i // 2 + D // 2, 0x82, n, 0)
ev(D * 9, 0x92, 66, 0)  # velocity 0 note-on
# Channel 4: pitch-bend range set by RPN, then bends.
ev(0, 0xB4, 101, 0); ev(0, 0xB4, 100, 0); ev(0, 0xB4, 6, 12); ev(0, 0xB4, 38, 0)
ev(0, 0xC4, 0); ev(D * 8, 0x94, 57, 100)
for k in range(8):
    ev(D * 8 + k * 60, 0xE4, 0, 127 if k % 2 else 0)
ev(D * 8 + 600, 0x84, 57, 0)
# Channel 9 (MIDI channel 10): the drum kit on bank 128 (CC0 = 0, program 0).
ev(0, 0xB9, 0, 0); ev(0, 0xC9, 0)
for i, k in enumerate(drum_keys * 2):
    ev(D * 9 + i * 120, 0x99, k, 90 - (i % 4) * 15)
    ev(D * 9 + i * 120 + 100, 0x89, k, 0)
ev(D * 11, 0xB9, 7, 110)
# Tempo change and a late note on channel 0.
meta(D * 12, 0x51, (300000).to_bytes(3, "big"))
ev(D * 12, 0x90, 48, 100); ev(D * 12 + D * 2, 0x80, 48, 0)
ev(D * 14, 0xB0, 123, 0)

events.sort(key=lambda e: e[0])  # stable: equal ticks keep their order
track = b""
last = 0
for tick, data in events:
    track += vlq(tick - last) + data
    last = tick
track += vlq(0) + bytes([0xFF, 0x2F, 0])
midi = b"MThd" + struct.pack(">IHHH", 6, 0, 1, D) + b"MTrk" + struct.pack(">I", len(track)) + track
(OUT / "synth.mid").write_bytes(midi)
print("wrote", OUT / "test.sf2", len(samples), "samples;", OUT / "synth.mid", len(midi), "bytes")

# ---------------------------------------------------------------- stress: more notes than voices
# 420 notes in under a second on one channel: the synthesizer must steal voices once 256 are in use.
events = []
meta(0, 0x51, (500000).to_bytes(3, "big"))
ev(0, 0xC0, 0)
for i in range(420):
    t = 4 * i
    k = 36 + (i * 7) % 60
    ev(t, 0x90, k, 60 + (i * 13) % 67)
    ev(t + 1900 + (i % 50) * 3, 0x80, k, 0)
events.sort(key=lambda e: e[0])
track = b""
last = 0
for tick, data in events:
    track += vlq(tick - last) + data
    last = tick
track += vlq(0) + bytes([0xFF, 0x2F, 0])
(OUT / "stress.mid").write_bytes(b"MThd" + struct.pack(">IHHH", 6, 0, 1, D) + b"MTrk" + struct.pack(">I", len(track)) + track)
print("wrote", OUT / "stress.mid")
