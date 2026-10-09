/* The application engine of Vector (core-rs/src/app.rs) as a C ABI. Everything a UI needs goes through these four functions:
   vcr_app_call("method", "{json args}") starts work and returns at once; results arrive as events (name, json) on the callback,
   which runs on engine threads (hand them to the UI thread). A few methods answer directly in the returned JSON. */
#ifndef VECTOR_APP_H
#define VECTOR_APP_H

#ifdef __cplusplus
extern "C" {
#endif

typedef struct App VcrApp;
typedef void (*VcrEventFn)(void *user, const char *name, const char *json);

VcrApp *vcr_app_new(const char *data_dir, VcrEventFn cb, void *user);
char *vcr_app_call(VcrApp *app, const char *method, const char *args_json); /* release with vcr_string_free */
void vcr_string_free(char *s);

/* Video calls: pictures are RGBA, width * height * 4 bytes, rows top to bottom. */
typedef void (*VcrVideoFn)(void *user, const char *who, int width, int height, const unsigned char *rgba); /* who: user id in a group call, "" otherwise */
void vcr_video_set_sink(VcrApp *app, VcrVideoFn cb, void *user); /* the other side's pictures; the buffer is valid during the callback only; decoder thread */
void vcr_video_push(VcrApp *app, int width, int height, const unsigned char *rgba); /* one camera picture (copied); ignored unless the camera is on */
void vcr_app_free(VcrApp *app);

#ifdef __cplusplus
}
#endif

#endif
