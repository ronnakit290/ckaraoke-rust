#!/bin/bash
# Checks the Rust synthesizer and player against the C++ app, byte for byte, on generated fixtures:
#   - default renders (test SoundFont + MIDI, and a stress file that steals voices),
#   - a mixer and transport script (mute, solo, gain, drum pieces, transpose, speed, seek, pause, volume).
#
#   rust/tools/crosscheck/run_synth.sh [work-dir]
#
# The C++ reference is built with -ffp-contract=off: on arm64 the compiler would otherwise fuse multiply
# and add, and the Rust port (which never fuses) would differ in the last bit of some samples.
set -u
HERE="$(cd "$(dirname "$0")" && pwd)"
REPO="$(cd "$HERE/../../.." && pwd)"
W="${1:-${TMPDIR:-/tmp}/ckaraoke-synth-check}"
mkdir -p "$W" && rm -rf "$W/fx" "$W/out" && mkdir -p "$W/out"
RUST_CLI="$REPO/rust/target/release/ckaraoke"
RUST_EX="$REPO/rust/target/release/examples/player_script"
pass=0; fail=0
check() { if [ "$2" -eq 0 ]; then pass=$((pass+1)); echo "PASS  $1"; else fail=$((fail+1)); echo "FAIL  $1"; fi; }

echo "== building"
(cd "$REPO/rust" && cargo build --release -q && cargo build --release -q --example player_script) || { echo "cargo build failed"; exit 1; }
(cd "$REPO" && make cli BUILD="$W/cbuild" CFLAGS="-O2 -ffp-contract=off" CXXFLAGS="-O2 -std=c++17 -ffp-contract=off" >"$W/make.log" 2>&1) || { echo "make failed: $W/make.log"; exit 1; }
clang++ -std=c++17 -O2 -ffp-contract=off -I "$REPO/src" -I "$REPO/third_party" "$HERE/harness_player.cpp" \
  "$W/cbuild/player.o" "$W/cbuild/song.o" "$W/cbuild/ckr.o" "$W/cbuild/crk.o" "$W/cbuild/crypto.o" "$W/cbuild/cm.o" \
  "$W/cbuild/library.o" "$W/cbuild/lyrics.o" "$W/cbuild/impl.o" "$W/cbuild/audio_output.o" \
  -lz -framework CoreFoundation -framework CoreAudio -framework AudioToolbox -o "$W/cpp_player" 2>/dev/null || { echo "harness build failed"; exit 1; }

echo "== generating fixtures"
python3 "$HERE/make_synth_fixtures.py" "$W/fx" >/dev/null || exit 1
F="$W/fx"; O="$W/out"; C="$W/cbuild/ckaraoke"

for song in synth stress; do
  "$C" "$F/test.sf2" "$F/$song.mid" --wav "$O/$song-cpp.wav" >/dev/null 2>&1
  "$RUST_CLI" "$F/test.sf2" "$F/$song.mid" --wav "$O/$song-rust.wav" >/dev/null 2>&1
  cmp -s "$O/$song-cpp.wav" "$O/$song-rust.wav"; check "render $song.mid: C++ and Rust identical" $?
done

"$W/cpp_player" "$F/test.sf2" "$F/synth.mid" "$HERE/mixer_script.txt" "$O/mix-cpp.wav" >/dev/null 2>&1
"$RUST_EX" "$F/test.sf2" "$F/synth.mid" "$HERE/mixer_script.txt" "$O/mix-rust.wav" >/dev/null 2>&1
cmp -s "$O/mix-cpp.wav" "$O/mix-rust.wav"; check "mixer and transport script: C++ and Rust identical" $?

echo
echo "RESULT: $pass passed, $fail failed  (work dir: $W)"
[ $fail -eq 0 ]
