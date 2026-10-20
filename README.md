# CKaraoke — เวอร์ชัน Rust

พอร์ตจากโค้ด C/C++ ในโฟลเดอร์ `src/` เป็น Rust ทีละส่วน ตอนนี้มี **แกนข้อมูล, synthesizer, การเล่นเสียงสดจาก CLI และการเรนเดอร์ WAV** ตรวจผลกับ C++ แล้ว ส่วน GUI, VST3, รีโมทผ่านเว็บ และหน้าจอคิว/มิกเซอร์ยังไม่ได้พอร์ต (ดูหัวข้อ [ยังไม่ได้พอร์ต](#ยังไม่ได้พอร์ต))

ไฟล์ `.ckr` ที่เขียนโดย C++ เปิดด้วย Rust ได้ และในทางกลับกัน โดยใช้คีย์เดียวกัน

## พอร์ตแล้ว

| ส่วน | โค้ดต้นฉบับ (C++) | โค้ด Rust | สถานะ |
|---|---|---|---|
| รูปแบบ `.ckr` (เขียน CKR2, อ่าน CKR2 และ CKR1) | `src/ckr.cpp` | `ckaraoke-core/src/pack.rs` | ✅ ตรวจกับ C++ แล้ว |
| codec แบบ context mixing (`.ckr` ใช้บีบอัดเพลง) | `src/cm.cpp` | `ckaraoke-core/src/cm.rs` | ✅ ตรงบิตต่อบิต |
| การเข้ารหัส (PBKDF2, HMAC, ChaCha20) | `src/crypto.cpp` | `ckaraoke-core/src/crypto.rs` | ✅ ใช้ crate มาตรฐาน RustCrypto |
| MIDI: เนื้อร้อง, tempo, division | `src/lyrics.cpp` | `midi.rs`, `lyrics.rs` | ✅ ตรวจกับ C++ แล้ว |
| เนื้อร้อง NCN (`.lyr` + `.cur`) | `src/lyrics.cpp`, `src/song.cpp` | `lyrics.rs`, `song.rs` | ✅ ตรวจกับ C++ แล้ว |
| ช่วงเนื้อร้องที่ร้องจริง (ตัดเครดิตหัว/ท้าย) | `sungRange` | `lyrics::sung_range` | ✅ ตรวจกับ C++ แล้ว |
| คลังเพลง: สแกน, บันทึก index, ค้นหา | `src/library.cpp` | `library.rs` | ✅ ตรวจกับ C++ แล้ว |
| คำสั่ง `--list`, `--pack`, `--unpack` | `src/main.cpp` | `ckaraoke-cli` | ✅ |
| คำสั่ง `--lyrics`, `--scan`, `--search` | (ไม่มีใน CLI เดิม) | `ckaraoke-cli` | ✅ เพิ่มเพื่อการตรวจสอบ |
| SoundFont 2 synthesizer (TinySoundFont v0.9) | `third_party/tsf.h` | `crates/tsf` | ✅ เรนเดอร์ตรงกับ C++ ทั้งไบต์ (ดูด้านล่าง) |
| ลำดับ MIDI และเวลาเป็น ms (TinyMidiLoader) | `third_party/tml.h` | `ckaraoke-core/src/tml.rs` | ✅ รวมกรณีการสลับตัวแรกของไลบรารี C |
| เครื่องเล่น: มิกเซอร์, drum piece, transpose, speed, seek, meter | `src/player.cpp` | `ckaraoke-core/src/player.rs` | ✅ ตรวจด้วยสคริปต์คำสั่งที่ตรงกันทั้งสองฝั่ง |
| เสียงออก, คีย์บอร์ด, หน้าจอเนื้อร้อง | `src/audio_output.cpp`, `src/main.cpp` (`playSong`, `draw`) | `ckaraoke-cli/src/play.rs` (cpal, crossterm) | ✅ ทดสอบใน pseudo-terminal (ยังไม่ได้ฟังเสียงจริง) |
| การเรนเดอร์เป็น WAV (`--wav`) | `renderToWav` ใน `src/main.cpp` | `ckaraoke-cli` | ✅ ไฟล์เหมือนกันทุกไบต์ |

## ยังไม่ได้พอร์ต

- **GUI ส่วนที่เหลือ** (`src/gui_mac.mm`): เมดเลย์, ตั้งค่าเนื้อร้อง (ฟอนต์, ขนาด, สี), การค้นหาแบบ overlay บนจอเนื้อร้อง, การบันทึกเพลย์ลิสต์
- **VST3** (`src/vst3_host.cpp`): ฝัง editor ของปลั๊กอินใน NSView ต้องคิดแยกต่างหาก
- **รีโมทผ่านเว็บ** (`src/remote.cpp`, `web/`): HTTP server เล็ก ๆ พอพอร์ตได้ตรง ๆ
- **SoundFont แบบบีบอัด** (SF3, `.sfo` ที่เป็น Ogg Vorbis) และ modulator ของ SF2: ทั้ง C++ (ที่ build ด้วย TSF แบบไม่มี Vorbis) และ Rust ไม่รองรับ
- **เอาต์พุตโมโนหรือ unweaved** ของ TSF: แอปใช้แค่สเตอริโอ interleaved
- การเลือกไฟล์ด้วยหน้าต่าง Finder และตัวเลือกคำสั่งที่เหลือของ CLI เดิม (`--no-gui`, `--loop`)

## ความต่างที่ตั้งใจ

1. **`.lyr` เป็น UTF-8 ที่มีเวลาต่อไบต์**: C++ จะแยกอักขระไทยหลายไบต์ไปคนละ syllable เมื่อเวลาของแต่ละไบต์ต่างกัน ทำให้ได้ไบต์ที่ไม่สมบูรณ์ (ตัวอักษรเสีย) Rust ให้ไบต์ต่อเนื่องติดกับไบต์นำเสมอ ไฟล์ที่เวลาของทั้งอักขระเท่ากัน (รูปแบบที่ไฟล์จริงใช้) ได้ผลเหมือนกันทุกประการ
2. **ชื่อเพลงและเนื้อร้องเป็น `String`** (ข้อความ UTF-8): เมื่ออ่านจาก pack ที่ไบต์ไม่ใช่ UTF-8 จะแทนด้วย U+FFFD ส่วน C++ เก็บไบต์ดิบ ไฟล์ที่แอปสร้างเองเป็น UTF-8 เสมอ จึงไม่กระทบ
3. **ไม่มีตำแหน่ง index ตายตัว**: C++ เก็บที่ `~/Library/Application Support/CKaraoke/library.tsv` แต่ Rust รับ `--index <ไฟล์>` เอง เพื่อไม่ผูกกับ macOS

## Build

ต้องมี Rust 1.87 ขึ้นไป (ทดสอบกับ 1.99 ผ่าน `brew install rust`) เพื่อให้ตรวจ synth กับ C++ ต้องมี `clang` และ `make` ของ Xcode command line tools

```bash
cd rust
cargo build --release
cargo test --workspace
```

ไบนารีอยู่ที่ `rust/target/release/ckaraoke`

## การใช้งาน

```bash
export CKARAOKE_KEY='รหัสของคุณ'

ckaraoke --pack songs.ckr ~/Songs          # แพ็กทั้งโฟลเดอร์เป็น .ckr ไฟล์เดียว
ckaraoke --list songs.ckr                  # ดูรายการเพลง (หมายเลขเริ่มที่ 1)
ckaraoke --lyrics songs.ckr#3              # เนื้อร้องพร้อมเวลาของเพลงที่ 3
ckaraoke --unpack ./out songs.ckr          # แตกเป็นโฟลเดอร์ Song / Lyrics / Cursor
ckaraoke --scan ~/Songs --index lib.tsv    # สแกนคลังเพลงและบันทึก index
ckaraoke --search "คิดถึง" ~/Songs --index lib.tsv
```

## ตรวจสอบความถูกต้องกับ C++

มีชุดตรวจสอบสองชุด รันซ้ำได้จากโฟลเดอร์ `rust/` ทั้งคู่ ไม่แตะโฟลเดอร์ `build/` ของคุณ

```bash
rust/tools/crosscheck/run.sh [โฟลเดอร์ทำงาน]        # เนื้อร้อง, pack, คลังเพลง
rust/tools/crosscheck/run_synth.sh [โฟลเดอร์ทำงาน]  # เสียง: เรนเดอร์ WAV และการควบคุมเครื่องเล่น
```

**`run.sh`** คอมไพล์ C++ CLI จาก `Makefile` และเทียบ: เนื้อร้องจากไฟล์และจาก pack, รายการใน pack, การแพ็กและแตกไฟล์ทั้งสองทิศทาง, ไบต์ของ MIDI หลังแตกไฟล์, คลังเพลง และการปฏิเสธรหัสผิด (fixture สร้างจาก `make_fixtures.py`) ผลล่าสุด: **55 ผ่าน, 0 ไม่ผ่าน**

**`run_synth.sh`** สร้าง SoundFont และ MIDI สังเคราะห์ (`make_synth_fixtures.py`) ที่มี sample แบบวนลูปและไม่วน envelope ของเสียงและ modulation, ฟิลเตอร์ low-pass, vibrato (LFO), pan, การปรับเสียง, velocity layer, ชุดกลองที่แต่ละคีย์เป็นชิ้นของตัวเอง, global zone, pitch bend พร้อมการเปลี่ยนช่วง (RPN), sustain pedal, expression, การเปลี่ยนเทมโป และ note-on ที่ velocity 0 แล้วเทียบ WAV จาก C++ กับ Rust ทั้งไบต์:
- เพลงปกติ และเพลงที่มีโน้ตเกิน 256 เสียงเพื่อทดสอบการแย่งเสียง (voice stealing)
- สคริปต์ควบคุมมิกเซอร์และการเล่น (`mixer_script.txt`): mute, solo, gain, ระดับ drum piece, transpose, speed, seek, pause, volume และ reset (สคริปต์นี้เปลี่ยนเสียงจริง จึงเป็นการทดสอบที่มีความหมาย)

ผลล่าสุด: **3 ผ่าน, 0 ไม่ผ่าน** และ `cargo test` 26 รายการผ่าน

หมายเหตุเรื่องความเท่ากันทุกไบต์: reference ของ C++ ที่ใช้ตรวจคอมไพล์ด้วย `-ffp-contract=off` เพราะบน arm64 คอมไพเลอร์อาจรวม `a*b+c` เป็นคำสั่งเดียว (FMA) ซึ่ง Rust ไม่ทำ เมื่อเทียบกับ C++ ที่คอมไพล์แบบปกติ ต่างกันไม่เกิน 1 LSB ใน 34 ตัวอย่างจาก 610,304 ตัวอย่าง (เทียบไม่ได้ทางการฟัง)

## แอปเดสก์ท็อป (GUI)

```bash
cargo run --release -p ckaraoke-gui
```

แอป `ckaraoke-gui` สร้างด้วย `egui` (eframe) เป็น Rust ล้วน ใช้ฟอนต์ Sathu ของระบบสำหรับอักษรไทย มีหน้าต่างหลักแบ่งเป็น:
- **คลังเพลง** (ซ้าย): เลือกโฟลเดอร์ สแกนในเบื้องหลัง (มี index ไว้ไม่ต้องสแกนซ้ำ) ค้นหาทันที กด ▶ เล่นเลย หรือ + เข้าคิว (Enter ในช่องค้นหาคือเข้าคิวอันดับแรก)
- **เนื้อเพลง** (กลาง): ชื่อ ศิลปิน คีย์ บรรทัดที่กำลังร้องพร้อมไฮไลต์ตามพยางค์ และบรรทัดถัดไป
- **แถบควบคุม** (ล่าง): เล่น/หยุด, ก่อนหน้า/ถัดไป, ย้อน/ข้าม 5 วินาที, เลื่อนตำแหน่ง, ความเร็ว 50–200%, คีย์ ±12, ระดับเสียง
- **คิว** (ปุ่มคิว): เล่นต่ออัตโนมัติ, ลบออกจากคิวเมื่อเริ่มเล่น, เล่นหรือลบทีละเพลง
- **มิกเซอร์** (ปุ่มมิกเซอร์): ระดับ mute/solo/เฟดเดอร์ของ 16 ช่อง พร้อมมิเตอร์ และชิ้นกลองที่เพลงใช้ (มีชื่อตาม GM)
- **ตั้งค่า**: รหัสไฟล์ `.ckr` (พิมพ์ทุกครั้ง ไม่บันทึกลงเครื่อง)

ลากไฟล์ `.sf2`, `.mid` / `.kar`, `.ckr` หรือโฟลเดอร์มาวางที่หน้าต่างได้ ปุ่มลัด: Space เล่น/หยุด, ← → ย้อน/ข้าม 5 วินาที, ⌘← ⌘→ เพลงก่อน/ถัดไป, ⌘[ ⌘] ความเร็ว, ⌘\ ความเร็ว 100%

ค่าที่จำไว้ระหว่างเปิดแอป: SoundFont, โฟลเดอร์เพลง, ระดับเสียง, ตัวเลือกคิว (ไฟล์ `settings.json` ใน `~/Library/Application Support/CKaraoke/`)

ยังไม่มีในแอปนี้: เมดเลย์, การตั้งค่าขนาดและฟอนต์ของเนื้อร้อง, รีโมทผ่านเว็บ, VST3, การบันทึกเพลย์ลิสต์

การทดสอบหน้าจอ: `CKARAOKE_SCREENSHOT=<ไฟล์> [CKARAOKE_SCREENSHOT_AT=<เฟรม>]` จะบันทึกภาพหน้าต่างเป็นข้อมูลดิบ แล้วแปลงเป็น PNG ด้วย `tools/gui/png_from_raw.py` และปิดแอป (ใช้ทดสอบเท่านั้น)

## เล่นเสียง

```bash
ckaraoke soundfont.sf2 song.mid                 # เล่นสด (ค่าเริ่มต้นเสียงออกของเครื่อง)
ckaraoke soundfont.sf2 songs.ckr#3              # เล่นเพลงที่ 3 ในไฟล์ .ckr (ตั้ง CKARAOKE_KEY ก่อน)
ckaraoke soundfont.sf2 song.mid --wav out.wav   # เรนเดอร์เป็น WAV 44.1 kHz สเตอริโอ
```

ปุ่มที่ใช้ขณะเล่น: Space หยุด/เล่น, ← → เลื่อน 5 วินาที, ↑ ↓ หรือ + − ระดับเสียง, r เริ่มใหม่, q ออก

เครื่องเล่นจะเล่นที่อัตราของอุปกรณ์เสียงที่เลือก (เช่น 48 kHz) ไม่ได้บังคับ 44.1 kHz เหมือน C++ การเรนเดอร์ WAV ใช้ 44.1 kHz เพื่อให้เทียบกับ C++ ได้

## โครงสร้าง

```
rust/
├── Cargo.toml                  workspace
├── crates/
│   ├── tsf/                    SoundFont 2 synthesizer (พอร์ตจาก third_party/tsf.h)
│   ├── ckaraoke-audio/         เสียงออกของเครื่อง (cpal) ใช้ร่วมกันทั้ง CLI และ GUI
│   ├── ckaraoke-gui/           แอปเดสก์ท็อป egui: คลัง, คิว, เนื้อเพลง, มิกเซอร์
│   ├── ckaraoke-core/          แกนข้อมูลและเครื่องเล่น ไม่มี UI
│   │   ├── src/                lib, text, cm, crypto, pack, midi, tml, lyrics, song, library, player
│   │   └── examples/           player_script (ตัวขับสคริปต์ให้ตรวจกับ C++)
│   └── ckaraoke-cli/           ไบนารี ckaraoke: ค้นหา/แพ็ก/แตก, เล่นสด, เรนเดอร์ WAV
└── tools/crosscheck/           สคริปต์ตรวจสอบกับ C++, fixture generator, ตัวขับ C++ และสคริปต์ควบคุม
```

crate ที่ใช้: `sha2`, `hmac`, `pbkdf2`, `chacha20` (RustCrypto), `flate2` (zlib สำหรับ CKR1), `zeroize` (ล้างคีย์จากหน่วยความจำ), `getrandom`, `cpal` (เสียงออก), `crossterm` (คีย์บอร์ดและหน้าจอ)

## การตัดสินใจที่ต้องทำ

- **เครื่องสังเคราะห์เสียง**: (ก) ใช้ crate `rustysynth` ซึ่งพร้อมใช้ แต่เสียงอาจต่างจาก TinySoundFont เล็กน้อย และการปรับระดับต่อกลอง (drum piece) ต้องประมาณ (ข) พอร์ต `third_party/tsf.h` ทั้งหมดเป็น Rust ให้เสียงตรงกับ C++ แต่ใช้เวลานานกว่ามาก
- **GUI**: เช่น `egui` (ข้ามแพลตฟอร์ม, ต้องโหลดฟอนต์ไทยเอง) หรือ Tauri ที่ใช้หน้าเว็บรีโมทเดิมใน `web/` ได้เลย
- **VST3**: ควรเก็บเป็นฟีเจอร์แยก หรือไม่พอร์ตในรอบแรก
