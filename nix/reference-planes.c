/* Test-only adapter: call the unchanged upstream decoder and export its planes.
 * H4M_REFERENCE_SOURCE names the separately fetched, pinned C source. */
#define HVQM4_FFMPEG
#define NATIVE 1
#include H4M_REFERENCE_SOURCE

/* Same buffer rotation and entry points as upstream decode_video(), without
 * its progress output and RGB files. This lets large corpora stream over a pipe. */
static uint32_t decode_planes(Player *player, FILE *input, uint16_t kind, uint32_t size)
{
    if (size < 4) exit(2);
    uint8_t *frame = calloc((size_t)size + 3, 1); /* bit reader's permitted overread */
    if (!frame || fread(frame, 1, size, input) != size) exit(2);
    uint32_t display = read32(frame);
    if (kind != B_FRAME) {
        void *tmp = player->past;
        player->past = player->future;
        player->future = tmp;
    }
    switch (kind) {
    case I_FRAME: HVQM4DecodeIpic(&player->seqobj, frame + 4, player->present); break;
    case P_FRAME: HVQM4DecodePpic(&player->seqobj, frame + 4, player->present, player->past); break;
    case B_FRAME: HVQM4DecodeBpic(&player->seqobj, frame + 4, player->present, player->past, player->future); break;
    default: exit(2);
    }
    free(frame);
    if (kind != B_FRAME) {
        void *tmp = player->present;
        player->present = player->future;
        player->future = tmp;
    }
    return display;
}

static void write_u32(uint32_t value)
{
    uint8_t bytes[4] = {value >> 24, value >> 16, value >> 8, value};
    if (fwrite(bytes, 1, sizeof(bytes), stdout) != sizeof(bytes)) exit(2);
}

int main(int argc, char **argv)
{
    int stream = argc == 3 && strcmp(argv[2], "--stream") == 0;
    if (argc != 2 && !stream) return 2;
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
    if (!state) return 2;
    state->padding[0] = header.version == HVQM4_15;
    HVQM4SetBuffer(&player.seqobj, state);
    decv_init(&player);
    if (!stream) mkdir("output", 0700);
    uint32_t base = 0;
    for (uint32_t block = 0; block < header.blocks; ++block) {
        get32(input); get32(input);
        uint32_t videos = get32(input), audios = get32(input);
        get32(input);
        for (uint32_t n = 0; n < videos + audios; ++n) {
            uint16_t type = get16(input), kind = get16(input);
            uint32_t size = get32(input);
            if (type == 0) { seek_past(size, input); continue; }
            if (type != 1) return 2;
            uint32_t display = decode_planes(&player, input, kind, size);
            const void *pixels = kind == B_FRAME ? player.present : player.future;
            size_t length = 0;
            for (unsigned p = 0; p < 3; ++p) length += state->planes[p].size_in_samples;
            FILE *out = stdout;
            if (stream) {
                /* Big-endian presentation index and byte length, then Y/U/V,
                 * repeated in decode order. No files or whole-movie buffering. */
                if (length > UINT32_MAX) return 2;
                write_u32(base + display);
                write_u32((uint32_t)length);
            } else {
                char name[80];
                snprintf(name, sizeof(name), "output/video_yuv_%04u.yuv", base + display);
                out = fopen(name, "wb");
            }
            if (!out || fwrite(pixels, 1, length, out) != length) return 2;
            if (!stream && fclose(out)) return 2;
        }
        base += videos;
    }
    free(state); free(player.past); free(player.present); free(player.future);
    if (fclose(input) || fflush(stdout)) return 2;
    return 0;
}
