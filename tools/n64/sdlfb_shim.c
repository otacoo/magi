/*
 * SDL 1.2 shim for the Miyoo recompilations on BaseOS.
 *
 * LD_PRELOADed into the 32-bit port. SDL itself only covers the port's
 * threads, timers and audio plumbing; video is a software surface copied to
 * /dev/fb0 on SDL_Flip, input comes straight from the H700 pad (evdev), and
 * audio is the port's SDL callback played through ALSA.
 */
#define _GNU_SOURCE
#include <SDL/SDL.h>
#include <dlfcn.h>
#include <errno.h>
#include <fcntl.h>
#include <linux/fb.h>
#include <linux/input.h>
#include <pthread.h>
#include <signal.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/ioctl.h>
#include <sys/mman.h>
#include <sys/time.h>
#include <time.h>
#include <unistd.h>

#define VOLUME_FILE "/tmp/magi-n64-volume"
#define WARMTH_FILE "/tmp/magi-n64-warmth"
#define OSD_FILE "/tmp/magi-n64-osd"

/* ---------------------------------------------------------------- video */

static int (*real_Init)(Uint32);
static SDL_Surface *(*real_CreateRGBSurface)(Uint32, int, int, int, Uint32, Uint32, Uint32, Uint32);

static int fb_fd = -1;
static uint8_t *fb_map;
static int fb_w, fb_h, fb_stride, fb_vres, fb_page;
static SDL_Surface *screen;

static void load_real(void) {
    if (!real_Init) {
        real_Init = (int (*)(Uint32)) dlsym(RTLD_NEXT, "SDL_Init");
        real_CreateRGBSurface = (SDL_Surface *(*)(Uint32, int, int, int, Uint32, Uint32, Uint32, Uint32))
            dlsym(RTLD_NEXT, "SDL_CreateRGBSurface");
    }
}

int SDL_Init(Uint32 flags) {
    load_real();
    int rc = real_Init((flags & ~SDL_INIT_VIDEO) | SDL_INIT_TIMER);
    fprintf(stderr, "shim: SDL_Init(%x) rc=%d\n", flags, rc);
    return 0;
}

int SDL_ShowCursor(int toggle) { (void) toggle; return 0; }
void SDL_WM_SetCaption(const char *title, const char *icon) { (void) title; (void) icon; }

static int open_fb(void) {
    if (fb_fd >= 0) {
        return 0;
    }
    fb_fd = open("/dev/fb0", O_RDWR);
    if (fb_fd < 0) {
        return -1;
    }
    struct fb_var_screeninfo v;
    struct fb_fix_screeninfo f;
    if (ioctl(fb_fd, FBIOGET_VSCREENINFO, &v) < 0 || ioctl(fb_fd, FBIOGET_FSCREENINFO, &f) < 0) {
        return -1;
    }
    fb_w = (int) v.xres;
    fb_h = (int) v.yres;
    fb_vres = (int) (v.yres_virtual ? v.yres_virtual : v.yres);
    fb_stride = (int) f.line_length;
    v.xoffset = 0;
    v.yoffset = 0;
    ioctl(fb_fd, FBIOPAN_DISPLAY, &v);
    size_t len = (size_t) fb_stride * (size_t) fb_vres;
    fb_map = mmap(NULL, len, PROT_READ | PROT_WRITE, MAP_SHARED, fb_fd, 0);
    if (fb_map == MAP_FAILED) {
        return -1;
    }
    fprintf(stderr, "shim: fb %dx%d vres %d stride %d\n", fb_w, fb_h, fb_vres, fb_stride);
    return 0;
}

SDL_Surface *SDL_SetVideoMode(int w, int h, int bpp, Uint32 flags) {
    (void) bpp;
    (void) flags;
    load_real();
    if (open_fb() < 0) {
        return NULL;
    }
    if (!screen) {
        screen = real_CreateRGBSurface(SDL_SWSURFACE, w, h, 32,
                                       0x00FF0000, 0x0000FF00, 0x000000FF, 0xFF000000);
    }
    return screen;
}

static int read_pct(const char *path, int def) {
    int fd = open(path, O_RDONLY | O_CLOEXEC);
    if (fd < 0) {
        return def;
    }
    char buf[16];
    ssize_t n = read(fd, buf, sizeof buf - 1);
    close(fd);
    if (n <= 0) {
        return def;
    }
    buf[n] = 0;
    int v = atoi(buf);
    return v < 0 ? 0 : (v > 100 ? 100 : v);
}

static int warmth_pct = -1;
static uint8_t warm_r[256], warm_g[256], warm_b[256];

static void refresh_warmth(void) {
    int p = read_pct(WARMTH_FILE, 0);
    if (p == warmth_pct) {
        return;
    }
    warmth_pct = p;
    float f = (float) p / 100.0f;
    float rs = 1.0f + 0.12f * f;
    float bs = 1.0f - 0.40f * f;
    for (int i = 0; i < 256; i++) {
        int r = (int) ((float) i * rs + 0.5f);
        int b = (int) ((float) i * bs + 0.5f);
        warm_r[i] = (uint8_t) (r > 255 ? 255 : r);
        warm_g[i] = (uint8_t) i;
        warm_b[i] = (uint8_t) (b > 255 ? 255 : b);
    }
}

struct osd_header {
    uint32_t magic;
    int32_t x, y;
    uint32_t w, h, pad;
    uint64_t expire_ms;
};

#define OSD_MAGIC 0x4F534431u

static uint32_t *osd_px;
static size_t osd_cap;

static void osd_overlay(uint8_t *dst, int max_w, int max_h, int stride) {
    int fd = open(OSD_FILE, O_RDONLY | O_CLOEXEC);
    if (fd < 0) {
        return;
    }
    struct osd_header h;
    if (read(fd, &h, sizeof h) != (ssize_t) sizeof h) {
        close(fd);
        return;
    }
    struct timespec ts;
    clock_gettime(CLOCK_REALTIME, &ts);
    uint64_t now = (uint64_t) ts.tv_sec * 1000 + (uint64_t) ts.tv_nsec / 1000000;
    if (h.magic != OSD_MAGIC || h.expire_ms < now || h.w == 0 || h.h == 0 ||
        h.w > (uint32_t) max_w || h.h > (uint32_t) max_h) {
        close(fd);
        return;
    }
    size_t n = (size_t) h.w * (size_t) h.h;
    if (n > osd_cap) {
        uint32_t *p = realloc(osd_px, n * sizeof *p);
        if (!p) {
            close(fd);
            return;
        }
        osd_px = p;
        osd_cap = n;
    }
    if (read(fd, osd_px, n * sizeof *osd_px) != (ssize_t) (n * sizeof *osd_px)) {
        close(fd);
        return;
    }
    close(fd);
    for (uint32_t y = 0; y < h.h; y++) {
        int dy = h.y + (int) y;
        if (dy < 0 || dy >= max_h) {
            continue;
        }
        for (uint32_t x = 0; x < h.w; x++) {
            int dx = h.x + (int) x;
            if (dx < 0 || dx >= max_w) {
                continue;
            }
            uint32_t src = osd_px[(size_t) y * h.w + x];
            uint32_t a = src >> 24;
            if (a == 0) {
                continue;
            }
            uint32_t *d = (uint32_t *) (dst + (size_t) dy * (size_t) stride) + dx;
            uint32_t dv = *d;
            uint32_t ia = 255 - a;
            uint32_t r = (((src >> 16) & 0xFF) * a + ((dv >> 16) & 0xFF) * ia) / 255;
            uint32_t g = (((src >> 8) & 0xFF) * a + ((dv >> 8) & 0xFF) * ia) / 255;
            uint32_t b = ((src & 0xFF) * a + (dv & 0xFF) * ia) / 255;
            *d = 0xFF000000u | (r << 16) | (g << 8) | b;
        }
    }
}

int SDL_Flip(SDL_Surface *s) {
    if (!s || !s->pixels || fb_fd < 0) {
        return -1;
    }
    int w = s->w < fb_w ? s->w : fb_w;
    int h = s->h < fb_h ? s->h : fb_h;
    int page = fb_vres >= 2 * fb_h ? fb_page : 0;
    uint8_t *dst = fb_map + (size_t) page * (size_t) fb_h * (size_t) fb_stride;
    refresh_warmth();
    if (warmth_pct == 0) {
        for (int y = 0; y < h; y++) {
            memcpy(dst + (size_t) y * (size_t) fb_stride,
                   (const uint8_t *) s->pixels + (size_t) y * (size_t) s->pitch,
                   (size_t) w * 4);
        }
    } else {
        for (int y = 0; y < h; y++) {
            const uint32_t *src = (const uint32_t *) ((const uint8_t *) s->pixels +
                                                       (size_t) y * (size_t) s->pitch);
            uint32_t *d = (uint32_t *) (dst + (size_t) y * (size_t) fb_stride);
            for (int x = 0; x < w; x++) {
                uint32_t p = src[x];
                d[x] = 0xFF000000u | ((uint32_t) warm_r[(p >> 16) & 0xFF] << 16) |
                       ((uint32_t) warm_g[(p >> 8) & 0xFF] << 8) | warm_b[p & 0xFF];
            }
        }
    }
    osd_overlay(dst, w, h, fb_stride);
    struct fb_var_screeninfo v;
    if (ioctl(fb_fd, FBIOGET_VSCREENINFO, &v) == 0) {
        v.yoffset = page * fb_h;
        ioctl(fb_fd, FBIOPAN_DISPLAY, &v);
    }
    if (fb_vres >= 2 * fb_h) {
        fb_page ^= 1;
    }
    return 0;
}

/* ---------------------------------------------------------------- input */

enum { LAYOUT_VENDOR, LAYOUT_XBOX, LAYOUT_SONY, LAYOUT_STANDARD };

struct pad {
    int fd;
    int layout;
    char name[64];
    int hat_x, hat_y;
};

static struct pad pads[4];
static int n_pads;
static int scanned;

static const int SDLK_UP_ = 273, SDLK_DOWN_ = 274, SDLK_RIGHT_ = 275, SDLK_LEFT_ = 276;
static const int SDLK_A_ = 32, SDLK_B_ = 306, SDLK_X_ = 304, SDLK_Y_ = 308;
static const int SDLK_L1_ = 'e', SDLK_R1_ = 't', SDLK_L2_ = 9, SDLK_R2_ = 8;
static const int SDLK_SELECT_ = 305, SDLK_START_ = 13, SDLK_MENU_ = 27;

static SDL_Event queue[64];
static int q_head, q_tail;

static void push_key(int sym, int down) {
    int next = (q_head + 1) % 64;
    if (next == q_tail) {
        return;
    }
    SDL_Event *e = &queue[q_head];
    memset(e, 0, sizeof *e);
    e->type = down ? SDL_KEYDOWN : SDL_KEYUP;
    e->key.state = down ? SDL_PRESSED : SDL_RELEASED;
    e->key.keysym.sym = (SDLKey) sym;
    e->key.keysym.scancode = 0;
    e->key.keysym.mod = KMOD_NONE;
    e->key.which = 0;
    q_head = next;
}

static int key_sym(int layout, uint16_t code) {
    switch (layout) {
    case LAYOUT_VENDOR:
        switch (code) {
        case BTN_SOUTH: return SDLK_A_;
        case BTN_EAST: return SDLK_B_;
        case BTN_NORTH: return SDLK_X_;
        case BTN_C: return SDLK_Y_;
        case BTN_WEST: return SDLK_L1_;
        case BTN_Z: return SDLK_R1_;
        case BTN_TL: return SDLK_SELECT_;
        case BTN_TR: return SDLK_START_;
        case BTN_TL2: return SDLK_MENU_;
        case BTN_SELECT: return SDLK_L2_;
        case BTN_START: return SDLK_R2_;
        default: return 0;
        }
    case LAYOUT_XBOX:
        switch (code) {
        case BTN_SOUTH: return SDLK_A_;
        case BTN_EAST: return SDLK_B_;
        case BTN_C: return SDLK_X_;
        case BTN_NORTH: return SDLK_Y_;
        case BTN_WEST: return SDLK_L1_;
        case BTN_Z: return SDLK_R1_;
        case BTN_TL: return SDLK_SELECT_;
        case BTN_TR: return SDLK_START_;
        case BTN_TL2: return SDLK_MENU_;
        default: return 0;
        }
    case LAYOUT_SONY:
        switch (code) {
        case BTN_SOUTH: return SDLK_A_;
        case BTN_EAST: return SDLK_B_;
        case BTN_WEST: return SDLK_X_;
        case BTN_NORTH: return SDLK_Y_;
        case BTN_TL: return SDLK_L1_;
        case BTN_TR: return SDLK_R1_;
        case BTN_SELECT: return SDLK_SELECT_;
        case BTN_START: return SDLK_START_;
        case BTN_MODE: return SDLK_MENU_;
        case BTN_TL2: return SDLK_L2_;
        case BTN_TR2: return SDLK_R2_;
        default: return 0;
        }
    default:
        switch (code) {
        case BTN_SOUTH: return SDLK_A_;
        case BTN_EAST: return SDLK_B_;
        case BTN_NORTH: return SDLK_X_;
        case BTN_WEST: return SDLK_Y_;
        case BTN_TL: return SDLK_L1_;
        case BTN_TR: return SDLK_R1_;
        case BTN_SELECT: return SDLK_SELECT_;
        case BTN_START: return SDLK_START_;
        case BTN_MODE: return SDLK_MENU_;
        case BTN_TL2: return SDLK_L2_;
        case BTN_TR2: return SDLK_R2_;
        default: return 0;
        }
    }
}

static int layout_for(const char *name) {
    if (strstr(name, "ANBERNIC")) return LAYOUT_VENDOR;
    if (strstr(name, "Xbox")) return LAYOUT_XBOX;
    if (strstr(name, "DualSense") || strstr(name, "DualShock") ||
        strstr(name, "Wireless Controller")) return LAYOUT_SONY;
    return LAYOUT_STANDARD;
}

static void scan_pads(void) {
    if (n_pads > 0 || scanned) {
        return;
    }
    scanned = 1;
    for (int i = 0; i < 32; i++) {
        char path[64];
        snprintf(path, sizeof path, "/dev/input/event%d", i);
        int fd = open(path, O_RDONLY | O_NONBLOCK);
        if (fd < 0) continue;
        char name[64] = {0};
        if (ioctl(fd, EVIOCGNAME(sizeof name - 1), name) < 0 ||
            (!strstr(name, "ANBERNIC") && !strstr(name, "Xbox") &&
             !strstr(name, "DualSense") && !strstr(name, "DualShock") &&
             !strstr(name, "Wireless Controller") && !strstr(name, "Gamepad") &&
             !strstr(name, "gamepad"))) {
            close(fd);
            continue;
        }
        pads[n_pads].fd = fd;
        pads[n_pads].layout = layout_for(name);
        snprintf(pads[n_pads].name, sizeof pads[n_pads].name, "%s", name);
        pads[n_pads].hat_x = pads[n_pads].hat_y = 0;
        fprintf(stderr, "shim: pad %s (%s)\n", path, name);
        n_pads++;
        if (n_pads == 4) break;
    }
    scanned = 1;
}

static void poll_pads(void) {
    scan_pads();
    for (int i = 0; i < n_pads; i++) {
        struct pad *p = &pads[i];
        struct input_event ev;
        while (read(p->fd, &ev, sizeof ev) == (ssize_t) sizeof ev) {
            if (ev.type == EV_KEY) {
                int sym = key_sym(p->layout, ev.code);
                if (sym) push_key(sym, ev.value != 0);
                switch (ev.code) {
                case BTN_DPAD_UP: push_key(SDLK_UP_, ev.value != 0); break;
                case BTN_DPAD_DOWN: push_key(SDLK_DOWN_, ev.value != 0); break;
                case BTN_DPAD_LEFT: push_key(SDLK_LEFT_, ev.value != 0); break;
                case BTN_DPAD_RIGHT: push_key(SDLK_RIGHT_, ev.value != 0); break;
                default: break;
                }
            } else if (ev.type == EV_ABS) {
                if (ev.code == ABS_HAT0X) {
                    if (ev.value < 0) { push_key(SDLK_LEFT_, 1); push_key(SDLK_RIGHT_, 0); }
                    else if (ev.value > 0) { push_key(SDLK_RIGHT_, 1); push_key(SDLK_LEFT_, 0); }
                    else { push_key(SDLK_LEFT_, 0); push_key(SDLK_RIGHT_, 0); }
                    p->hat_x = ev.value;
                } else if (ev.code == ABS_HAT0Y) {
                    if (ev.value < 0) { push_key(SDLK_UP_, 1); push_key(SDLK_DOWN_, 0); }
                    else if (ev.value > 0) { push_key(SDLK_DOWN_, 1); push_key(SDLK_UP_, 0); }
                    else { push_key(SDLK_UP_, 0); push_key(SDLK_DOWN_, 0); }
                    p->hat_y = ev.value;
                } else if (p->layout == LAYOUT_XBOX || p->layout == LAYOUT_SONY) {
                    if (ev.code == ABS_Z) push_key(SDLK_L2_, ev.value > 300);
                    else if (ev.code == ABS_RZ) push_key(SDLK_R2_, ev.value > 300);
                }
            }
        }
    }
}

int SDL_PollEvent(SDL_Event *e) {
    if (q_tail == q_head) poll_pads();
    if (q_tail == q_head) return 0;
    *e = queue[q_tail];
    q_tail = (q_tail + 1) % 64;
    return 1;
}

void SDL_LockAudio(void) {}
void SDL_UnlockAudio(void) {}

/* ---------------------------------------------------------------- audio */

typedef struct _snd_pcm snd_pcm_t;
static int (*pa_open)(snd_pcm_t **, const char *, int, int);
static int (*pa_set_params)(snd_pcm_t *, int, int, unsigned int, unsigned int, int, unsigned int);
static long (*pa_writei)(snd_pcm_t *, const void *, unsigned long);
static int (*pa_close)(snd_pcm_t *);

struct audio_state {
    SDL_AudioSpec spec;
    snd_pcm_t *pcm;
    pthread_t thread;
    int running;
    int paused;
    int started;
};

static struct audio_state audio;
static int fifo_fd = -1;

static int write_all(int fd, const void *data, size_t len) {
    const uint8_t *p = (const uint8_t *) data;
    while (len > 0) {
        ssize_t n = write(fd, p, len);
        if (n < 0) {
            if (errno == EINTR) {
                continue;
            }
            return 0;
        }
        p += n;
        len -= (size_t) n;
    }
    return 1;
}

static void put16(int fd, uint16_t v) {
    uint8_t b[2] = {(uint8_t) v, (uint8_t) (v >> 8)};
    write_all(fd, b, sizeof b);
}

static void put32(int fd, uint32_t v) {
    uint8_t b[4] = {(uint8_t) v, (uint8_t) (v >> 8), (uint8_t) (v >> 16), (uint8_t) (v >> 24)};
    write_all(fd, b, sizeof b);
}

static int write_wav_header(int fd, int rate, int channels) {
    uint16_t align = (uint16_t) (channels * 2);
    uint32_t byte_rate = (uint32_t) rate * align;
    if (!write_all(fd, "RIFF", 4)) return 0;
    put32(fd, 0xFFFFFFFFu);
    if (!write_all(fd, "WAVEfmt ", 8)) return 0;
    put32(fd, 16);
    put16(fd, 1);
    put16(fd, (uint16_t) channels);
    put32(fd, (uint32_t) rate);
    put32(fd, byte_rate);
    put16(fd, align);
    put16(fd, 16);
    if (!write_all(fd, "data", 4)) return 0;
    put32(fd, 0xFFFFFFFFu);
    return 1;
}

static void *audio_thread(void *arg) {
    (void) arg;
    int bytes = audio.spec.channels * 2;
    int len = audio.spec.samples * bytes;
    uint8_t *buf = malloc((size_t) len);
    if (!buf) return NULL;
    while (audio.running) {
        if (audio.paused || (!audio.pcm && fifo_fd < 0)) {
            usleep(10000);
            continue;
        }
        if (audio.spec.callback) {
            audio.spec.callback(audio.spec.userdata, buf, len);
        } else {
            memset(buf, 0, (size_t) len);
        }
        int pct = read_pct(VOLUME_FILE, 100);
        if (pct <= 0) {
            memset(buf, 0, (size_t) len);
        } else if (pct < 100) {
            int32_t g2 = pct * pct;
            int16_t *samples = (int16_t *) buf;
            for (int i = 0; i < len / 2; i++) {
                samples[i] = (int16_t) (((int32_t) samples[i] * g2) / 10000);
            }
        }
        if (fifo_fd >= 0) {
            if (!write_all(fifo_fd, buf, (size_t) len)) {
                close(fifo_fd);
                fifo_fd = -1;
            }
        } else if (audio.pcm) {
            if (pa_writei(audio.pcm, buf, audio.spec.samples) < 0) {
                pa_close(audio.pcm);
                audio.pcm = NULL;
            }
        }
    }
    free(buf);
    return NULL;
}

int SDL_OpenAudio(SDL_AudioSpec *desired, SDL_AudioSpec *obtained) {
    fprintf(stderr, "shim: SDL_OpenAudio %d Hz %d ch fmt %x samples %d\n",
            desired ? desired->freq : 0, desired ? desired->channels : 0,
            desired ? desired->format : 0, desired ? desired->samples : 0);
    if (!desired) return -1;
    audio.spec = *desired;
    if (obtained) *obtained = *desired;

    const char *fifo = getenv("MAGI_AUDIO_FIFO");
    if (fifo && *fifo) {
        signal(SIGPIPE, SIG_IGN);
        fifo_fd = open(fifo, O_WRONLY | O_NONBLOCK | O_CLOEXEC);
        if (fifo_fd >= 0 &&
            !write_wav_header(fifo_fd, audio.spec.freq, audio.spec.channels)) {
            close(fifo_fd);
            fifo_fd = -1;
        }
        if (fifo_fd >= 0) {
            fprintf(stderr, "shim: audio FIFO %s (%d Hz, %d ch)\n", fifo,
                    audio.spec.freq, audio.spec.channels);
        } else {
            fprintf(stderr, "shim: audio FIFO %s unavailable\n", fifo);
        }
    }

    if (fifo_fd < 0) {
        void *lib = dlopen("libasound.so.2", RTLD_LAZY | RTLD_LOCAL);
        if (lib) {
            pa_open = (int (*)(snd_pcm_t **, const char *, int, int)) dlsym(lib, "snd_pcm_open");
            pa_set_params = (int (*)(snd_pcm_t *, int, int, unsigned int, unsigned int, int, unsigned int))
                dlsym(lib, "snd_pcm_set_params");
            pa_writei = (long (*)(snd_pcm_t *, const void *, unsigned long)) dlsym(lib, "snd_pcm_writei");
            pa_close = (int (*)(snd_pcm_t *)) dlsym(lib, "snd_pcm_close");
        }
        if (pa_open && pa_set_params && pa_writei && pa_close) {
            const char *want = getenv("MAGI_ALSA_DEVICE");
            if (!want || !*want) {
                want = "default";
            }
            if (pa_open(&audio.pcm, want, 0, 0) < 0) {
                audio.pcm = NULL;
                if (strcmp(want, "default") != 0) {
                    fprintf(stderr, "shim: ALSA %s failed; falling back to default\n", want);
                    pa_open(&audio.pcm, "default", 0, 0);
                }
            }
            if (!audio.pcm) {
                audio.pcm = NULL;
            } else if (pa_set_params(audio.pcm, 2 /* S16_LE */, 3 /* RW interleaved */,
                                     (unsigned int) audio.spec.channels,
                                     (unsigned int) audio.spec.freq, 1, 100000) < 0) {
                pa_close(audio.pcm);
                audio.pcm = NULL;
            }
        }
    }
    if (fifo_fd < 0) {
        fprintf(stderr, "shim: ALSA %s (%s)\n", audio.pcm ? "open" : "unavailable",
                getenv("MAGI_ALSA_DEVICE") ? getenv("MAGI_ALSA_DEVICE") : "default");
    }
    audio.running = 1;
    audio.paused = 1;
    if (pthread_create(&audio.thread, NULL, audio_thread, NULL) == 0) {
        audio.started = 1;
    }
    return 0;
}

void SDL_PauseAudio(int pause_on) { audio.paused = pause_on != 0; }

void SDL_CloseAudio(void) {
    if (audio.started) {
        audio.running = 0;
        pthread_join(audio.thread, NULL);
        audio.started = 0;
    }
    if (audio.pcm && pa_close) {
        pa_close(audio.pcm);
        audio.pcm = NULL;
    }
    if (fifo_fd >= 0) {
        close(fifo_fd);
        fifo_fd = -1;
    }
}
