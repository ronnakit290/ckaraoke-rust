// Reference scanner: prints the catalogue the C++ app builds for a folder, in the Rust `--scan` format.
#include <cstdio>
#include <string>
#include "library.hpp"
int main(int argc, char** argv) {
    if (argc < 2) return 2;
    ScanProgress progress;
    std::vector<SongInfo> songs = scanLibrary(argv[1], progress);
    std::printf("พบ %zu เพลง\n", songs.size());
    for (const SongInfo& s : songs) {
        std::string line = s.title;
        if (!s.artist.empty()) line += " — " + s.artist;
        if (!s.key.empty()) line += " (" + s.key + ")";
        std::printf("%s\n", line.c_str());
    }
    return 0;
}
