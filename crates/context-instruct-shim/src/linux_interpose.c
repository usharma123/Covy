#define _GNU_SOURCE

#include <dlfcn.h>
#include <errno.h>
#include <fcntl.h>
#include <pthread.h>
#include <stdarg.h>
#include <stddef.h>
#include <string.h>
#include <sys/types.h>

/*
 * Keep every variadic declaration, argument read, and invocation in C. The
 * Rust ELF exports are naked, argument-transparent tail branches into these C
 * entry points; the only Rust business-logic calls are the two fixed callbacks
 * below. No Rust code retypes a variadic function or RTLD_NEXT pointer.
 */
extern int context_instruct_shim_linux_try_open(const char *path, int flags,
                                                int *replacement_fd);
extern int context_instruct_shim_linux_try_openat(int dirfd, const char *path,
                                                  int flags,
                                                  int *replacement_fd);

typedef int (*open_fn)(const char *path, int flags, ...);
typedef int (*openat_fn)(int dirfd, const char *path, int flags, ...);

static pthread_once_t real_open_once = PTHREAD_ONCE_INIT;
static pthread_once_t real_open64_once = PTHREAD_ONCE_INIT;
static pthread_once_t real_openat_once = PTHREAD_ONCE_INIT;
static pthread_once_t real_openat64_once = PTHREAD_ONCE_INIT;
static open_fn real_open;
static open_fn real_open64;
static openat_fn real_openat;
static openat_fn real_openat64;

/*
 * `_FILE_OFFSET_BITS=64` may macro-redirect these names in <fcntl.h>. The
 * preload library must still define all four concrete ELF symbols.
 */
#undef open
#undef open64
#undef openat
#undef openat64

static void resolve_open(void) {
  void *symbol = dlsym(RTLD_NEXT, "open");
  _Static_assert(sizeof(real_open) == sizeof(symbol),
                 "POSIX function and object pointers must have equal size");
  memcpy(&real_open, &symbol, sizeof(real_open));
}

static void resolve_open64(void) {
  void *symbol = dlsym(RTLD_NEXT, "open64");
  _Static_assert(sizeof(real_open64) == sizeof(symbol),
                 "POSIX function and object pointers must have equal size");
  memcpy(&real_open64, &symbol, sizeof(real_open64));
}

static void resolve_openat(void) {
  void *symbol = dlsym(RTLD_NEXT, "openat");
  _Static_assert(sizeof(real_openat) == sizeof(symbol),
                 "POSIX function and object pointers must have equal size");
  memcpy(&real_openat, &symbol, sizeof(real_openat));
}

static void resolve_openat64(void) {
  void *symbol = dlsym(RTLD_NEXT, "openat64");
  _Static_assert(sizeof(real_openat64) == sizeof(symbol),
                 "POSIX function and object pointers must have equal size");
  memcpy(&real_openat64, &symbol, sizeof(real_openat64));
}

static open_fn required_open(void) {
  int error = pthread_once(&real_open_once, resolve_open);
  if (error != 0) {
    errno = error;
    return NULL;
  }
  if (real_open == NULL) {
    errno = ENOSYS;
  }
  return real_open;
}

static open_fn optional_open64(void) {
  int error = pthread_once(&real_open64_once, resolve_open64);
  if (error != 0) {
    errno = error;
    return NULL;
  }
  return real_open64 != NULL ? real_open64 : required_open();
}

static openat_fn required_openat(void) {
  int error = pthread_once(&real_openat_once, resolve_openat);
  if (error != 0) {
    errno = error;
    return NULL;
  }
  if (real_openat == NULL) {
    errno = ENOSYS;
  }
  return real_openat;
}

static openat_fn optional_openat64(void) {
  int error = pthread_once(&real_openat64_once, resolve_openat64);
  if (error != 0) {
    errno = error;
    return NULL;
  }
  return real_openat64 != NULL ? real_openat64 : required_openat();
}

static int flags_require_mode(int flags) {
  if ((flags & O_CREAT) != 0) {
    return 1;
  }
#ifdef O_TMPFILE
  /*
   * O_TMPFILE contains O_DIRECTORY on Linux. Testing for any overlapping bit
   * would consume a nonexistent variadic argument for ordinary O_DIRECTORY
   * calls, so require the complete mask.
   */
  if ((flags & O_TMPFILE) == O_TMPFILE) {
    return 1;
  }
#endif
  return 0;
}

static int call_open(open_fn function, const char *path, int flags, mode_t mode,
                     int has_mode) {
  if (function == NULL) {
    return -1;
  }
  return has_mode != 0 ? function(path, flags, mode) : function(path, flags);
}

static int call_openat(openat_fn function, int dirfd, const char *path,
                       int flags, mode_t mode, int has_mode) {
  if (function == NULL) {
    return -1;
  }
  return has_mode != 0 ? function(dirfd, path, flags, mode)
                       : function(dirfd, path, flags);
}

int context_instruct_shim_linux_open(const char *path, int flags, ...) {
  int has_mode = flags_require_mode(flags);
  mode_t mode = 0;
  if (has_mode != 0) {
    va_list args;
    va_start(args, flags);
    mode = va_arg(args, mode_t);
    va_end(args);
  }

  int replacement_fd = -1;
  if (context_instruct_shim_linux_try_open(path, flags, &replacement_fd) != 0) {
    return replacement_fd;
  }
  return call_open(required_open(), path, flags, mode, has_mode);
}

int context_instruct_shim_linux_open64(const char *path, int flags, ...) {
  int has_mode = flags_require_mode(flags);
  mode_t mode = 0;
  if (has_mode != 0) {
    va_list args;
    va_start(args, flags);
    mode = va_arg(args, mode_t);
    va_end(args);
  }

  int replacement_fd = -1;
  if (context_instruct_shim_linux_try_open(path, flags, &replacement_fd) != 0) {
    return replacement_fd;
  }
  return call_open(optional_open64(), path, flags, mode, has_mode);
}

int context_instruct_shim_linux_openat(int dirfd, const char *path, int flags,
                                      ...) {
  int has_mode = flags_require_mode(flags);
  mode_t mode = 0;
  if (has_mode != 0) {
    va_list args;
    va_start(args, flags);
    mode = va_arg(args, mode_t);
    va_end(args);
  }

  int replacement_fd = -1;
  if (context_instruct_shim_linux_try_openat(dirfd, path, flags,
                                             &replacement_fd) != 0) {
    return replacement_fd;
  }
  return call_openat(required_openat(), dirfd, path, flags, mode, has_mode);
}

int context_instruct_shim_linux_openat64(int dirfd, const char *path, int flags,
                                        ...) {
  int has_mode = flags_require_mode(flags);
  mode_t mode = 0;
  if (has_mode != 0) {
    va_list args;
    va_start(args, flags);
    mode = va_arg(args, mode_t);
    va_end(args);
  }

  int replacement_fd = -1;
  if (context_instruct_shim_linux_try_openat(dirfd, path, flags,
                                             &replacement_fd) != 0) {
    return replacement_fd;
  }
  return call_openat(optional_openat64(), dirfd, path, flags, mode, has_mode);
}
