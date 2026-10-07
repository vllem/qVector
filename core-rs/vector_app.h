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
void vcr_app_free(VcrApp *app);

#ifdef __cplusplus
}
#endif

#endif
