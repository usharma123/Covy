#include <errno.h>
#include <fcntl.h>
#include <limits.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <sys/types.h>
#include <unistd.h>

#define ORIGINAL_CONTENT "agents-original"
#define REWRITTEN_CONTENT "agents-rewritten"

typedef int (*open_variant_fn)(const char *path, int dirfd, int flags);

struct open_variant {
  const char *name;
  open_variant_fn call;
};

static void fail(const char *operation) {
  fprintf(stderr, "%s failed: errno=%d (%s)\n", operation, errno,
          strerror(errno));
  exit(1);
}

static void require(int condition, const char *message) {
  if (!condition) {
    fprintf(stderr, "assertion failed: %s\n", message);
    exit(1);
  }
}

static void make_path(char *out, size_t out_len, const char *root,
                      const char *name) {
  int written = snprintf(out, out_len, "%s/%s", root, name);
  require(written >= 0 && (size_t)written < out_len, "fixture path overflow");
}

static int call_open(const char *path, int dirfd, int flags) {
  (void)dirfd;
  return open(path, flags);
}

static int call_openat(const char *path, int dirfd, int flags) {
  (void)path;
  return openat(dirfd, "AGENTS.md", flags);
}

static void write_all(int fd, const char *content, const char *operation) {
  size_t remaining = strlen(content);
  const char *cursor = content;
  while (remaining != 0) {
    ssize_t written = write(fd, cursor, remaining);
    if (written < 0 && errno == EINTR) {
      continue;
    }
    if (written <= 0) {
      fail(operation);
    }
    cursor += written;
    remaining -= (size_t)written;
  }
}

static int open_real_readonly(const char *path) {
  return open(path, O_RDONLY | O_NONBLOCK);
}

static void restore_real_file(const char *path) {
  int fd = open(path, O_WRONLY | O_TRUNC);
  if (fd < 0) {
    fail("restore real instruction file");
  }
  write_all(fd, ORIGINAL_CONTENT, "write restored instruction file");
  if (close(fd) != 0) {
    fail("close restored instruction file");
  }
}

static void expect_fd_contents(int fd, const char *expected,
                               const char *operation) {
  char buffer[128] = {0};
  ssize_t count = read(fd, buffer, sizeof(buffer) - 1);
  if (count < 0) {
    fail(operation);
  }
  require((size_t)count == strlen(expected), "unexpected content length");
  require(memcmp(buffer, expected, (size_t)count) == 0,
          "unexpected file contents");
}

static void expect_real_contents(const char *path, const char *expected) {
  int fd = open_real_readonly(path);
  if (fd < 0) {
    fail("open real instruction file");
  }
  expect_fd_contents(fd, expected, "read real instruction file");
  if (close(fd) != 0) {
    fail("close real instruction file");
  }
}

static void expect_virtualized_read(const struct open_variant *variant,
                                    const char *path, int dirfd, int flags) {
  int fd = variant->call(path, dirfd, flags);
  if (fd < 0) {
    fail(variant->name);
  }
  expect_fd_contents(fd, REWRITTEN_CONTENT, "read virtualized instruction");
  int status_flags = fcntl(fd, F_GETFL);
  require(status_flags >= 0 && (status_flags & O_ACCMODE) == O_RDONLY,
          "virtualized descriptor was not O_RDONLY");
  int descriptor_flags = fcntl(fd, F_GETFD);
  require(descriptor_flags >= 0 &&
              ((descriptor_flags & FD_CLOEXEC) != 0) ==
                  ((flags & O_CLOEXEC) != 0),
          "virtualized descriptor changed O_CLOEXEC");
  errno = 0;
  require(write(fd, "x", 1) == -1 && errno == EBADF,
          "virtualized instruction was not opened read-only");
  close(fd);
}

static void expect_access_mode(int fd, int expected) {
  int flags = fcntl(fd, F_GETFL);
  require(flags >= 0 && (flags & O_ACCMODE) == expected,
          "real instruction access mode changed");
}

static void exercise_real_semantics(const struct open_variant *variant,
                                    const char *path, int dirfd) {
  restore_real_file(path);
  int fd = variant->call(path, dirfd, O_WRONLY);
  if (fd < 0) {
    fail("O_WRONLY instruction open");
  }
  expect_access_mode(fd, O_WRONLY);
  write_all(fd, "agents-writable", "write O_WRONLY instruction");
  close(fd);
  expect_real_contents(path, "agents-writable");

  restore_real_file(path);
  fd = variant->call(path, dirfd, O_RDWR);
  if (fd < 0) {
    fail("O_RDWR instruction open");
  }
  expect_access_mode(fd, O_RDWR);
  require(pwrite(fd, "X", 1, 0) == 1, "O_RDWR did not target the real file");
  close(fd);
  expect_real_contents(path, "Xgents-original");

  restore_real_file(path);
  fd = variant->call(path, dirfd, O_WRONLY | O_TRUNC);
  if (fd < 0) {
    fail("O_TRUNC instruction open");
  }
  struct stat truncated;
  require(fstat(fd, &truncated) == 0 && truncated.st_size == 0,
          "O_TRUNC did not truncate the real file");
  close(fd);

  restore_real_file(path);
  fd = variant->call(path, dirfd, O_WRONLY | O_APPEND);
  if (fd < 0) {
    fail("O_APPEND instruction open");
  }
  int append_flags = fcntl(fd, F_GETFL);
  require(append_flags >= 0 && (append_flags & O_APPEND) != 0,
          "O_APPEND status was not preserved");
  write_all(fd, "-append", "append to real instruction");
  close(fd);
  expect_real_contents(path, "agents-original-append");

  errno = 0;
  require(variant->call(path, dirfd, O_RDONLY | O_DIRECTORY) == -1 &&
              errno == ENOTDIR,
          "O_DIRECTORY did not preserve ENOTDIR");
  restore_real_file(path);
  expect_real_contents(path, ORIGINAL_CONTENT);
}

int main(int argc, char **argv) {
  require(argc == 2, "usage: macos_open_semantics FIXTURE_ROOT");
  const char *root = argv[1];
  char path[PATH_MAX];
  char regular_path[PATH_MAX];
  make_path(path, sizeof(path), root, "AGENTS.md");
  make_path(regular_path, sizeof(regular_path), root, "not-a-directory.txt");
  int dirfd = open(root, O_RDONLY | O_DIRECTORY);
  if (dirfd < 0) {
    fail("open fixture directory");
  }
  const struct open_variant variants[] = {
      {"open O_RDONLY", call_open},
      {"openat O_RDONLY", call_openat},
  };
  const size_t variant_count = sizeof(variants) / sizeof(variants[0]);
  for (size_t index = 0; index < variant_count; ++index) {
    expect_virtualized_read(&variants[index], path, dirfd, O_RDONLY);
    expect_virtualized_read(&variants[index], path, dirfd,
                            O_RDONLY | O_CLOEXEC);
  }
  int regular_fd = open(regular_path, O_RDONLY);
  if (regular_fd < 0) {
    fail("open regular-file dirfd fixture");
  }
  errno = 0;
  require(openat(regular_fd, "AGENTS.md", O_RDONLY) == -1 && errno == ENOTDIR,
          "openat regular-file dirfd did not preserve ENOTDIR");
  close(regular_fd);
  expect_real_contents(path, ORIGINAL_CONTENT);
  for (size_t index = 0; index < variant_count; ++index) {
    exercise_real_semantics(&variants[index], path, dirfd);
  }
  close(dirfd);
  puts("macos-open-semantics-ok");
  return 0;
}
