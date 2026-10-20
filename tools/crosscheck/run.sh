#!/bin/bash
# Cross-checks the Rust port against the C++ app on generated fixtures.
#
#   rust/tools/crosscheck/run.sh [work-dir]
#
# Builds the C++ CLI from this repo's Makefile into <work-dir>/cbuild (the repo's own build/ is not touched),
# builds two C++ reference readers from the same sources, generates fixtures, and compares:
# lyrics (direct files and packs), pack tables, pack/unpack trees, MIDI bytes, library scans, and wrong keys.
set -u
HERE="$(cd "$(dirname "$0")" && pwd)"
REPO="$(cd "$HERE/../../.." && pwd)"
W="${1:-${TMPDIR:-/tmp}/ckaraoke-crosscheck}"
RUST="$REPO/rust/target/release/ckaraoke"
mkdir -p "$W" && rm -rf "$W/work" "$W/fixtures" && mkdir -p "$W/work"

echo "== building the Rust CLI"
(cd "$REPO/rust" && cargo build --release -q) || { echo "cargo build failed"; exit 1; }

echo "== building the C++ reference (into $W/cbuild)"
(cd "$REPO" && make cli BUILD="$W/cbuild" >"$W/make.log" 2>&1) || { echo "make failed, see $W/make.log"; exit 1; }
CPP_OBJS="$W/cbuild/lyrics.o $W/cbuild/song.o $W/cbuild/ckr.o $W/cbuild/crk.o $W/cbuild/crypto.o $W/cbuild/cm.o $W/cbuild/library.o"
clang++ -std=c++17 -O2 -I "$REPO/src" -I "$REPO/third_party" "$HERE/harness_lyrics.cpp" $CPP_OBJS -lz -o "$W/cpp_lyrics" || exit 1
clang++ -std=c++17 -O2 -I "$REPO/src" -I "$REPO/third_party" "$HERE/harness_scan.cpp" $CPP_OBJS -lz -o "$W/cpp_scan" || exit 1

echo "== generating fixtures"
python3 "$HERE/make_fixtures.py" "$W/fixtures" >/dev/null || exit 1
F="$W/fixtures"; C="$W/cbuild/ckaraoke"; H="$W/cpp_lyrics"; WK="$W/work"
export CKARAOKE_KEY="testkey-123"
pass=0; fail=0
check() { if [ "$2" -eq 0 ]; then pass=$((pass+1)); echo "PASS  $1"; else fail=$((fail+1)); echo "FAIL  $1"; fi; }

SRC=("$F/plain_utf8.mid" "$F/plain_tis.mid" "$F/plain_latin.mid" "$F/karaoke.kar" \
     "$F/ncn/Song/Pop/ncn1.mid" "$F/ncn/Song/Rock/ncn2.mid" "$F/ncn/Song/Rock/ncn3.mid" "$F/ncn/Song/Pop/ncn4.mid")
ALL=("${SRC[@]}" "$F/big.mid")

echo "== lyrics read straight from files"
for f in "${SRC[@]}" "$F/big.mid"; do
  "$H" "$f" > "$WK/h.txt" 2>/dev/null; hc=$?
  "$RUST" --lyrics "$f" > "$WK/r.txt" 2>/dev/null; rc=$?
  cmp -s "$WK/h.txt" "$WK/r.txt" && [ $hc -eq $rc ]; check "lyrics $(basename "$f")" $?
done

echo "== packing"
"$C" --pack "$WK/cpp.ckr" "${ALL[@]}" >/dev/null 2>&1; check "C++ packs the sources" $?
"$RUST" --pack "$WK/rust.ckr" "${ALL[@]}" >/dev/null 2>&1; check "Rust packs the sources" $?
"$C" --list "$WK/cpp.ckr" > "$WK/l1.txt"; "$RUST" --list "$WK/cpp.ckr" > "$WK/l2.txt"; cmp -s "$WK/l1.txt" "$WK/l2.txt"; check "list of C++ pack: C++ and Rust agree" $?
"$C" --list "$WK/rust.ckr" > "$WK/l3.txt"; "$RUST" --list "$WK/rust.ckr" > "$WK/l4.txt"; cmp -s "$WK/l3.txt" "$WK/l4.txt"; check "list of Rust pack: C++ and Rust agree" $?
tail -n +2 "$WK/l1.txt" | cmp -s - <(tail -n +2 "$WK/l3.txt"); check "song tables of both packs are equal" $?
N=$(head -1 "$WK/l1.txt" | sed -E 's/.*\(([0-9]+) เพลง\)/\1/')

echo "== every song read from either pack"
for ((i=1; i<=N; i++)); do
  "$H" "$WK/cpp.ckr#$((i-1))" > "$WK/a.txt" 2>&1
  "$RUST" --lyrics "$WK/cpp.ckr#$i" > "$WK/b.txt" 2>&1; cmp -s "$WK/a.txt" "$WK/b.txt"; check "song $i of C++ pack: C++ and Rust readers" $?
  "$H" "$WK/rust.ckr#$((i-1))" > "$WK/c.txt" 2>&1
  "$RUST" --lyrics "$WK/rust.ckr#$i" > "$WK/d.txt" 2>&1; cmp -s "$WK/c.txt" "$WK/d.txt"; check "song $i of Rust pack: C++ and Rust readers" $?
  cmp -s "$WK/a.txt" "$WK/c.txt"; check "song $i: C++ reads both packs the same" $?
done

echo "== unpacking"
mkdir -p "$WK/uc1" "$WK/ur1" "$WK/uc2" "$WK/ur2"
"$C" --unpack "$WK/uc1" "$WK/cpp.ckr" >/dev/null 2>&1; "$RUST" --unpack "$WK/ur1" "$WK/cpp.ckr" >/dev/null 2>&1
diff -r "$WK/uc1" "$WK/ur1" >/dev/null; check "unpack of C++ pack: same tree" $?
"$C" --unpack "$WK/uc2" "$WK/rust.ckr" >/dev/null 2>&1; "$RUST" --unpack "$WK/ur2" "$WK/rust.ckr" >/dev/null 2>&1
diff -r "$WK/uc2" "$WK/ur2" >/dev/null; check "unpack of Rust pack: same tree" $?
for f in "$F/plain_utf8.mid" "$F/plain_tis.mid" "$F/plain_latin.mid" "$F/karaoke.kar" "$F/ncn/Song/Pop/ncn1.mid" "$F/ncn/Song/Rock/ncn2.mid" "$F/big.mid"; do
  base=$(basename "$f")
  orig=$(shasum -a 256 "$f" | cut -c1-64)
  found=$(find "$WK/ur2" -name "*.mid" -exec shasum -a 256 {} \; | cut -c1-64 | grep -c "^$orig")
  [ "$found" -ge 1 ]; check "MIDI bytes come back unchanged: $base" $?
done

echo "== library scan"
"$W/cpp_scan" "$F" > "$WK/scan_cpp.txt" 2>/dev/null
"$RUST" --scan "$F" > "$WK/scan_rust.txt" 2>/dev/null
diff "$WK/scan_cpp.txt" "$WK/scan_rust.txt" >/dev/null; check "scan of fixtures: same catalogue" $?

echo "== wrong key"
CKARAOKE_KEY=wrong "$C" --list "$WK/cpp.ckr" >/dev/null 2>&1; w1=$?
CKARAOKE_KEY=wrong "$RUST" --list "$WK/cpp.ckr" >/dev/null 2>&1; w2=$?
[ $w1 -ne 0 ] && [ $w2 -ne 0 ]; check "wrong key is refused by both" $?

echo
echo "RESULT: $pass passed, $fail failed  (work dir: $W)"
[ $fail -eq 0 ]
