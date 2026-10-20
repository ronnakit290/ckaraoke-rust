// C++ reference for the mixer and transport: renders a song while applying the same script as the Rust
// example `player_script`. Build with the strict flags used by run_synth.sh (no fused multiply-add).
#include <cstdint>
#include <cstdio>
#include <cstdlib>
#include <fstream>
#include <sstream>
#include <string>
#include <vector>

#include "player.hpp"
#include "song.hpp"

namespace {

void writeLE(std::ofstream& f, uint32_t v, int bytes) {
    for (int i = 0; i < bytes; ++i) f.put(static_cast<char>((v >> (8 * i)) & 0xFF));
}

void apply(Player& p, const std::string& cmd, const std::vector<std::string>& a) {
    auto f = [&](size_t i) { return std::atof(a.at(i).c_str()); };
    if (cmd == "mute") p.setChannelMute(static_cast<int>(f(0)), f(1) != 0.0);
    else if (cmd == "solo") p.setChannelSolo(static_cast<int>(f(0)), f(1) != 0.0);
    else if (cmd == "gain") p.setChannelGain(static_cast<int>(f(0)), static_cast<float>(f(1)));
    else if (cmd == "drumgain") p.setDrumGain(static_cast<int>(f(0)), static_cast<float>(f(1)));
    else if (cmd == "drummute") p.setDrumMute(static_cast<int>(f(0)), f(1) != 0.0);
    else if (cmd == "transpose") p.setTranspose(static_cast<int>(f(0)));
    else if (cmd == "speed") p.setSpeed(f(0));
    else if (cmd == "seek") p.seek(f(0));
    else if (cmd == "pause") p.togglePause();
    else if (cmd == "volume") p.setVolume(static_cast<float>(f(0)));
    else if (cmd == "resetmixer") p.resetMixer();
    else if (cmd == "resetdrums") p.resetDrums();
    else { std::fprintf(stderr, "unknown command %s\n", cmd.c_str()); std::exit(2); }
}

}  // namespace

int main(int argc, char** argv) {
    if (argc < 5) return 2;
    Player player;
    std::string err;
    if (!player.loadSoundFont(argv[1], 44100, err)) { std::fprintf(stderr, "%s\n", err.c_str()); return 1; }
    Song song;
    if (!loadSong(argv[2], song, err, "") || !player.loadMidi(song.midi, err)) { std::fprintf(stderr, "%s\n", err.c_str()); return 1; }

    struct Cmd { size_t block; std::string cmd; std::vector<std::string> args; };
    std::vector<Cmd> commands;
    std::ifstream script(argv[3]);
    for (std::string line; std::getline(script, line);) {
        std::istringstream in(line);
        Cmd c;
        if (!(in >> c.block >> c.cmd)) continue;
        for (std::string w; in >> w;) c.args.push_back(w);
        commands.push_back(c);
    }

    std::ofstream out(argv[4], std::ios::binary);
    out.write("RIFF\0\0\0\0WAVEfmt ", 16);
    writeLE(out, 16, 4); writeLE(out, 1, 2); writeLE(out, 2, 2);
    writeLE(out, 44100, 4); writeLE(out, 44100 * 4, 4); writeLE(out, 4, 2); writeLE(out, 16, 2);
    out.write("data\0\0\0\0", 8);

    std::vector<float> buf(1024 * 2);
    uint32_t dataBytes = 0;
    size_t block = 0, frames = 0;
    while (!player.finished() && block < 5000) {
        for (const Cmd& c : commands)
            if (c.block == block) apply(player, c.cmd, c.args);
        player.render(buf.data(), 1024);
        for (float s : buf) {
            int v = static_cast<int>(std::lround(std::max(-1.0f, std::min(1.0f, s)) * 32767.0f));
            writeLE(out, static_cast<uint16_t>(static_cast<int16_t>(v)), 2);
        }
        dataBytes += static_cast<uint32_t>(buf.size() * 2);
        frames += 1024;
        ++block;
    }
    out.seekp(4); writeLE(out, 36 + dataBytes, 4);
    out.seekp(40); writeLE(out, dataBytes, 4);
    std::printf("blocks=%zu frames=%zu\n", block, frames);
    return 0;
}
