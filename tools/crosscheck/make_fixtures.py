"""Generates karaoke fixtures for cross-checking the Rust port against the C++ build.

Covers: MIDI with UTF-8 / TIS-620 / Latin-1 lyrics, running status, tempo changes, .kar text events,
NCN trees (16-bit and 32-bit timing tables, CRLF lyric files).
"""
import pathlib
import random
import struct
import sys

OUT = pathlib.Path(sys.argv[1])
OUT.mkdir(parents=True, exist_ok=True)


def vlq(n):
    out = [n & 0x7F]
    n >>= 7
    while n:
        out.append((n & 0x7F) | 0x80)
        n >>= 7
    return bytes(reversed(out))


def meta(kind, data):
    return bytes([0xFF, kind]) + vlq(len(data)) + data


def smf(division, events, scramble=False):
    track = b""
    for delta, ev in events:
        track += vlq(delta) + ev
    track += vlq(0) + bytes([0xFF, 0x2F, 0x00])
    data = b"MThd" + struct.pack(">IHHH", 6, 0, 1, division) + b"MTrk" + struct.pack(">I", len(track)) + track
    return b"ZZZZ" + data[4:] if scramble else data


def write(path, data):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(data)


def tis(text):
    return text.encode("tis-620")


def cursor_for(expected, stride, start=0, step=12):
    """A timing table with one entry per expected slot; some slots are 'no time' (0xFFFF)."""
    values = []
    t = start
    for i in range(expected):
        values.append(0xFFFF if i % 17 == 16 else t)
        t += step
    if stride == 2:
        return b"".join(struct.pack("<H", v) for v in values)
    return b"".join(struct.pack("<I", v) for v in values)


def expected_slots(lyr_lines):
    return 1 + sum(len(line) + 1 for line in lyr_lines)


# 1. UTF-8 lyrics, tempo change, running status.
utf8_words = ["คิด", "ถึง", "เธอ", "\\ทุกวัน", "ยัง", "/คง", "อยู่", "ตรงนี้", "ไม่", "เคย", "ลืม"]
events = [(0, meta(0x51, (500000).to_bytes(3, "big")))]
events.append((0, b"\x90\x3c\x64"))  # note on, starts running status
for i, w in enumerate(utf8_words):
    events.append((40 + (i % 3) * 20, meta(0x05, w.encode("utf-8"))))
    events.append((0, b"\x3e\x64"))  # running-status note on
events.append((480, meta(0x51, (400000).to_bytes(3, "big"))))  # tempo change
for w in ["เปลี่ยน", "จังหวะ", "ตรง", "นี้"]:
    events.append((120, meta(0x05, w.encode("utf-8"))))
write(OUT / "plain_utf8.mid", smf(480, events))

# 2. TIS-620 lyrics with a long line that must wrap at 48 columns.
tis_words = ["ดวงดาว", "บน", "ฟ้า", "/", "สวย", "งาม", "เหลือเกิน", "\\", "ใจ", "เอ๋ย", "ใจ", "เอ๋ย", "ดวงดาว", "ข้า", "ลืม", "ไม่", "ลง", "ฉัน", "ยัง", "จำ", "ได้", "ทุก", "คืน", "วัน"]
events = []
for w in tis_words:
    events.append((48, meta(0x05, tis(w))))
write(OUT / "plain_tis.mid", smf(96, events))

# 3. Latin-1-looking bytes (C1 controls) so the decoder takes the Latin-1 path.
events = [(0, meta(0x05, bytes([0x80, 0x81, 0x41]))), (96, meta(0x05, bytes([0x82, 0x42]))), (96, meta(0x05, b"\\C"))]
write(OUT / "plain_latin.mid", smf(96, events))

# 4. .kar: FF 01 text events with @T headers, TIS-620 body, a "/" line break.
events = [
    (0, meta(0x01, b"@TMy Title")),
    (0, meta(0x01, b"@TMy Artist")),
    (0, meta(0x01, b"@K Em")),
    (96, meta(0x01, tis("ความรัก"))),
    (96, meta(0x01, tis("/ของเรา"))),
    (96, meta(0x01, tis("สองคน"))),
]
write(OUT / "karaoke.kar", smf(96, events))

# 5. NCN tree: Song/Pop/ncn1.mid with a CRLF .lyr and a 16-bit .cur; Song/Rock/ncn2.mid with a UTF-8 .lyr
#    and a 32-bit .cur; Song/Rock/ncn3.mid with a .lyr but no .cur; Song/Pop/ncn4.mid with nothing.
ncn1_lines = [tis("ทดสอบ 1"), tis("ทดสอบ เสียง"), tis("ฟ้าสีคราม"), tis("ดาวสีทอง")]
ncn1_lyr = b"\r\n".join([tis("คิดถึง ( Em )"), tis("ศิลปิน"), tis("Em"), b""] + ncn1_lines) + b"\r\n"
root = OUT / "ncn"
mid_events = [(0, meta(0x51, (500000).to_bytes(3, "big")))] + [(0, b"\x90\x3c\x64")] + [(0, b"\x00")]
write(root / "Song" / "Pop" / "ncn1.mid", smf(480, [(0, meta(0x51, (500000).to_bytes(3, "big")))]))
write(root / "Lyrics" / "Pop" / "ncn1.lyr", ncn1_lyr)
write(root / "Cursor" / "Pop" / "ncn1.cur", cursor_for(expected_slots([l for l in ncn1_lines]), 2, step=24))

ncn2_lines = ["ดาวดวงที่หนึ่ง", "แสงจันทร์", "ลอยเลื่อน"]
ncn2_lyr = "\n".join(["Rock song ( G )", "Band", "G", ""] + ncn2_lines).encode("utf-8") + b"\n"
write(root / "Song" / "Rock" / "ncn2.mid", smf(96, [(0, meta(0x51, (600000).to_bytes(3, "big")))]))
write(root / "Lyrics" / "Rock" / "ncn2.lyr", ncn2_lyr)
def cursor_utf8_chars(lines, step=7, start=100):
    """One time per character: every UTF-8 continuation byte repeats its lead byte's time."""
    vals, t = [0], start
    for line in lines:
        prev = None
        for b in line:
            if (b & 0xC0) == 0x80:
                vals.append(prev)
            else:
                vals.append(t)
                prev = t
                t += step
        vals.append(t)
        t += step
    return b"".join(struct.pack("<I", v) for v in vals)


write(root / "Cursor" / "Rock" / "ncn2.cur", cursor_utf8_chars([l.encode("utf-8") for l in ncn2_lines]))

write(root / "Song" / "Rock" / "ncn3.mid", smf(96, [(0, meta(0x51, (500000).to_bytes(3, "big")))]))
write(root / "Lyrics" / "Rock" / "ncn3.lyr", tis("ไม่มีตัวจับเวลา\nนักร้อง\nC\n\nเนื้อเพลง"))
write(root / "Song" / "Pop" / "ncn4.mid", smf(96, [(0, meta(0x05, b"loose lyric"))]))

print("fixtures written to", OUT)

# 7. A large MIDI (about 217 KB) so the codec's hash tables and match model get real work.
rnd = random.Random(7)
big_words = ["คิด", "ถึง", "เธอ", "ทุก", "วัน", "ยัง", "คง", "อยู่", "ตรง", "นี้", "ไม่", "เคย", "ลืม", "เลย"]
parts = []
for n in range(30000):
    w = rnd.choice(big_words) if rnd.random() < 0.8 else "".join(chr(0x0E01 + rnd.randrange(40)) for _ in range(3))
    data = w.encode("tis-620", errors="replace")
    if n % 9 == 8:
        data = b"\\" + data
    delta = rnd.choice([0, 12, 24, 48, 96, 96, 96, 240])
    parts.append(vlq(delta) + bytes([0xFF, 0x05]) + vlq(len(data)) + data)
    if n % 500 == 0:
        parts.append(vlq(0) + bytes([0x90, 60, 100]))
track = b"".join(parts) + vlq(0) + bytes([0xFF, 0x2F, 0])
write(OUT / "big.mid", b"MThd" + struct.pack(">IHHH", 6, 0, 1, 96) + b"MTrk" + struct.pack(">I", len(track)) + track)
