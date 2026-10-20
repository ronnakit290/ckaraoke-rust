// Reference reader: loads one song through the C++ app's own loadSong() and prints its lyrics in the
// same plain format as `ckaraoke --lyrics` in the Rust port. Pack songs are addressed from 0 here.
#include <cstdio>
#include <cstdlib>
#include <string>
#include "song.hpp"
#include "lyrics.hpp"

int main(int argc, char** argv) {
    if (argc < 2) return 2;
    const char* k = std::getenv("CKARAOKE_KEY");
    std::string key = k ? k : "";
    Song song;
    std::string err;
    if (!loadSong(argv[1], song, err, key)) {
        std::fprintf(stderr, "%s\n", err.c_str());
        return 1;
    }
    const Lyrics& L = song.lyrics;
    std::printf("title: %s\n", L.title.c_str());
    std::printf("artist: %s\n", L.artist.c_str());
    std::printf("key: %s\n", L.key.c_str());
    SungRange sung = sungRange(L);
    std::printf("sung %.3f %.3f\n", sung.startMs, sung.endMs);
    for (const auto& line : L.lines) {
        std::printf("line %.3f\n", line.startMs);
        for (const auto& s : line.sylls) std::printf("  syll %.3f %s\n", s.ms, s.text.c_str());
    }
    return 0;
}
