/* Application-private EGL adapter for Mesa builds exposing desktop OpenGL only.
 * wgpu probes configs with EGL_OPENGL_ES2_BIT before choosing desktop OpenGL.
 * Keep the real EGL ABI and change only that filter when ES is unavailable.
 * No system library, libc symbol version, or GPU feature limit is changed.
 */
#define _GNU_SOURCE
#include <dlfcn.h>
#include <pthread.h>
#include <string.h>

typedef unsigned int EGLBoolean;
typedef int EGLint;
typedef void *EGLDisplay;
typedef void *EGLConfig;
typedef EGLBoolean (*ChooseConfig)(EGLDisplay, const EGLint *, EGLConfig *, EGLint, EGLint *);
typedef const char *(*QueryString)(EGLDisplay, EGLint);

static pthread_once_t once = PTHREAD_ONCE_INIT;
static ChooseConfig real_choose;
static QueryString real_query;

static void load_real(void) {
    void *library = dlopen("libEGL_nyaterm.so.1", RTLD_NOW | RTLD_LOCAL);
    if (library) {
        real_choose = (ChooseConfig)dlsym(library, "eglChooseConfig");
        real_query = (QueryString)dlsym(library, "eglQueryString");
    }
}

EGLBoolean eglChooseConfig(EGLDisplay display, const EGLint *attributes,
                          EGLConfig *configs, EGLint size, EGLint *count) {
    enum { EGL_NONE = 0x3038, EGL_RENDERABLE_TYPE = 0x3040,
           EGL_CLIENT_APIS = 0x308D, EGL_OPENGL_ES2_BIT = 4, EGL_OPENGL_BIT = 8 };
    pthread_once(&once, load_real);
    if (!real_choose || !real_query) return 0;
    const char *apis = real_query(display, EGL_CLIENT_APIS);
    if (attributes && apis && strstr(apis, "OpenGL") && !strstr(apis, "OpenGL_ES")) {
        EGLint copy[128];
        unsigned int i = 0;
        for (; i + 2 < sizeof(copy) / sizeof(copy[0]); i += 2) {
            copy[i] = attributes[i];
            if (attributes[i] == EGL_NONE)
                return real_choose(display, copy, configs, size, count);
            copy[i + 1] = attributes[i + 1];
            if (copy[i] == EGL_RENDERABLE_TYPE && (copy[i + 1] & EGL_OPENGL_ES2_BIT))
                copy[i + 1] = (copy[i + 1] & ~EGL_OPENGL_ES2_BIT) | EGL_OPENGL_BIT;
        }
    }
    return real_choose(display, attributes, configs, size, count);
}
