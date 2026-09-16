/* Test-only adapter: call the unchanged upstream decoder and export its planes.
 * H4M_REFERENCE_SOURCE names the separately fetched, pinned C source. */
#define HVQM4_FFMPEG
#define NATIVE 1
#include H4M_REFERENCE_SOURCE

int main(int argc, char **argv)
{
    if (argc != 2) return 2;
    FILE *input = fopen(argv[1], "rb");
    if (!input) return 2;
    uint8_t bytes[68];
    if (fread(bytes, 1, sizeof(bytes), input) != sizeof(bytes)) return 2;
    HVQM4_header header;
    load_header(&header, bytes);
    Player player = {0};
    VideoInfo info = {header.hres, header.vres, header.hsamp, header.vsamp, header.video_mode};
    HVQM4InitDecoder();
    HVQM4InitSeqObj(&player.seqobj, &info);
    VideoState *state = calloc(1, HVQM4BuffSize(&player.seqobj));
    state->padding[0] = header.version == HVQM4_15;
    HVQM4SetBuffer(&player.seqobj, state);
    decv_init(&player);
    mkdir("output", 0700);
    uint32_t base = 0;
    for (uint32_t block = 0; block < header.blocks; ++block) {
        get32(input); get32(input);
        uint32_t videos = get32(input), audios = get32(input);
        get32(input);
        for (uint32_t n = 0; n < videos + audios; ++n) {
            uint16_t type = get16(input), kind = get16(input);
            uint32_t size = get32(input);
            if (type == 0) { seek_past(size, input); continue; }
            long start = ftell(input);
            uint32_t display = get32(input);
            fseek(input, start, SEEK_SET);
            decode_video(&player, input, base, kind, size);
            const void *pixels = kind == B_FRAME ? player.present : player.future;
            size_t bytes = 0;
            for (unsigned p = 0; p < 3; ++p) bytes += state->planes[p].size_in_samples;
            char name[80];
            snprintf(name, sizeof(name), "output/video_yuv_%04u.yuv", base + display);
            FILE *out = fopen(name, "wb");
            if (!out || fwrite(pixels, 1, bytes, out) != bytes || fclose(out)) return 2;
        }
        base += videos;
    }
    free(state); free(player.past); free(player.present); free(player.future);
    fclose(input);
    return 0;
}
