#include <fcntl.h>
#include <stdarg.h>
#include <sys/types.h>

extern int context_instruct_shim_open(const char *path, int flags, mode_t mode);
extern int context_instruct_shim_openat(int dirfd, const char *path, int flags, mode_t mode);
extern void context_instruct_shim_set_initialized(int value);

void context_instruct_shim_macos_interpose_anchor(void) {}

__attribute__((constructor)) static void context_instruct_shim_init(void) {
  context_instruct_shim_set_initialized(1);
}

static int replacement_open(const char *path, int flags, ...) {
  mode_t mode = 0;
  if (flags & O_CREAT) {
    va_list args;
    va_start(args, flags);
    mode = va_arg(args, int);
    va_end(args);
  }
  return context_instruct_shim_open(path, flags, mode);
}

static int replacement_openat(int dirfd, const char *path, int flags, ...) {
  mode_t mode = 0;
  if (flags & O_CREAT) {
    va_list args;
    va_start(args, flags);
    mode = va_arg(args, int);
    va_end(args);
  }
  return context_instruct_shim_openat(dirfd, path, flags, mode);
}

struct interpose_entry {
  const void *replacement;
  const void *replacee;
};

__attribute__((used)) static struct interpose_entry interpose_open
    __attribute__((section("__DATA,__interpose"))) = {
        (const void *)(unsigned long)&replacement_open,
        (const void *)(unsigned long)&open,
};

__attribute__((used)) static struct interpose_entry interpose_openat
    __attribute__((section("__DATA,__interpose"))) = {
        (const void *)(unsigned long)&replacement_openat,
        (const void *)(unsigned long)&openat,
};
