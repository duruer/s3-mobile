/* C API of libs3ios.a (crates/s3-ios/src/lib.rs), used by the Swift shell in port/ios.
 * All calls except s3ios_audio_pull come from the main thread. See docs/porting/ios.md. */
#ifndef S3IOS_H
#define S3IOS_H

#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef struct S3App S3App;
typedef struct S3AudioRing S3AudioRing;

/* What the shell should do after s3ios_tick. */
typedef struct S3Out {
    uint8_t redraw;    /* something new to show: display() the GL view */
    uint8_t keyboard;  /* soft keyboard: 0 no change, 1 show, 2 hide, 3 toggle */
    uint8_t quit;      /* the launcher's Quit was tapped */
    uint8_t in_game;   /* the game runs (not the launcher) */
    uint32_t haptic_ms; /* vibrate (0 = no) */
    uint8_t pick_folder; /* game folder: 1 show the folder picker, 2 use the app's own folder again */
} S3Out;

/* game_dir: the user's Settlers III files (Documents/Settlers3); data_dir: settings, logs (Documents/Data). */
S3App *s3ios_create(const char *game_dir, const char *data_dir);
void s3ios_destroy(S3App *app);
/* View size in pixels and the screen density. */
void s3ios_resize(S3App *app, uint32_t w, uint32_t h, uint32_t dpi);
/* The screen's maximum refresh rate (UIScreen.maximumFramesPerSecond). */
void s3ios_set_refresh(S3App *app, uint32_t hz);
/* Once per display refresh, before drawing. */
S3Out s3ios_tick(S3App *app);
/* Draw into the bound framebuffer (GLKView draw, its EAGLContext current). */
void s3ios_draw(S3App *app, uint32_t w, uint32_t h);
/* action: 0 down (first finger), 1 another finger down, 2 move, 3 a finger up (not the last), 4 last finger up,
 * 5 cancel. id/x/y: the acting finger (pixels); ids/xs/ys: all n fingers down (including the acting one). */
void s3ios_touch(S3App *app, int32_t action, int32_t id, float x, float y, int32_t n, const int32_t *ids,
                 const float *xs, const float *ys);
/* Typed character (Unicode scalar; 8 = backspace, 10/13 = return). */
void s3ios_text(S3App *app, uint32_t c);
/* Hardware key as a Windows virtual key code. */
void s3ios_key(S3App *app, uint32_t vk, uint8_t down);
/* Character a hardware key produced (after s3ios_key): WM_CHAR only. */
void s3ios_char(S3App *app, uint32_t c);
/* Game folder: a folder the user picked (external = 1, security-scoped access started) or the app's own (0). */
void s3ios_set_game_dir(S3App *app, const char *dir, uint8_t external);
/* 1 = the app goes to the background (the game pauses), 0 = back in the foreground. */
void s3ios_set_background(S3App *app, uint8_t background);

/* Audio for the audio unit's render thread: interleaved stereo int16 at 44100 Hz. */
const S3AudioRing *s3ios_audio_handle(S3App *app);
void s3ios_audio_pull(const S3AudioRing *ring, int16_t *out, uint32_t frames);
void s3ios_audio_release(const S3AudioRing *ring);

#ifdef __cplusplus
}
#endif

#endif
