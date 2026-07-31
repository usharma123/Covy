#define _GNU_SOURCE

#include <errno.h>
#include <fcntl.h>
#include <limits.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <sys/types.h>
#include <unistd.h>

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

static void expect_contents(int fd, const char *expected,
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

static void expect_mode(int fd, mode_t expected, const char *operation) {
  struct stat metadata;
  if (fstat(fd, &metadata) != 0) {
    fail(operation);
  }
  require((metadata.st_mode & 0777) == expected, "created mode was not kept");
}

#ifdef O_TMPFILE
static int tmpfile_unsupported(int error) {
  return error == EINVAL || error == EISDIR || error == ENOENT ||
         error == EOPNOTSUPP;
}
#endif

int main(int argc, char **argv) {
  require(argc == 2, "usage: linux_open_abi FIXTURE_ROOT");
  const char *root = argv[1];
  char path[PATH_MAX];
  mode_t previous_umask = umask(0);

  make_path(path, sizeof(path), root, "existing.txt");
  int fd = open(path, O_RDONLY);
  if (fd < 0) {
    fail("two-argument open");
  }
  expect_contents(fd, "open-existing", "read after open");
  close(fd);

  make_path(path, sizeof(path), root, "AGENTS.md");
  fd = open(path, O_RDONLY);
  if (fd < 0) {
    fail("two-argument open instruction passthrough");
  }
  expect_contents(fd, "agents-existing", "read instruction file after open");
  close(fd);

  fd = open64(path, O_RDONLY);
  if (fd < 0) {
    fail("two-argument open64 instruction passthrough");
  }
  expect_contents(fd, "agents-existing", "read instruction file after open64");
  close(fd);

  make_path(path, sizeof(path), root, "directory");
  fd = open(path, O_RDONLY | O_DIRECTORY);
  if (fd < 0) {
    fail("two-argument open with O_DIRECTORY");
  }
  close(fd);

  make_path(path, sizeof(path), root, "missing.txt");
  errno = 0;
  require(open(path, O_RDONLY) == -1, "missing open unexpectedly succeeded");
  require(errno == ENOENT, "missing open did not preserve ENOENT");

  make_path(path, sizeof(path), root, "created.txt");
  fd = open(path, O_WRONLY | O_CREAT | O_EXCL, (mode_t)0601);
  if (fd < 0) {
    fail("three-argument open");
  }
  expect_mode(fd, (mode_t)0601, "fstat created open file");
  close(fd);

  make_path(path, sizeof(path), root, "existing64.txt");
  fd = open64(path, O_RDONLY);
  if (fd < 0) {
    fail("two-argument open64");
  }
  expect_contents(fd, "open64-existing", "read after open64");
  close(fd);

  make_path(path, sizeof(path), root, "created64.txt");
  fd = open64(path, O_WRONLY | O_CREAT | O_EXCL, (mode_t)0640);
  if (fd < 0) {
    fail("three-argument open64");
  }
  expect_mode(fd, (mode_t)0640, "fstat created open64 file");
  close(fd);

  int dirfd = open(root, O_RDONLY | O_DIRECTORY);
  if (dirfd < 0) {
    fail("open fixture directory");
  }

  fd = openat(dirfd, "AGENTS.md", O_RDONLY);
  if (fd < 0) {
    fail("three-argument openat instruction passthrough");
  }
  expect_contents(fd, "agents-existing", "read instruction file after openat");
  close(fd);

  fd = openat64(dirfd, "AGENTS.md", O_RDONLY);
  if (fd < 0) {
    fail("three-argument openat64 instruction passthrough");
  }
  expect_contents(fd, "agents-existing",
                  "read instruction file after openat64");
  close(fd);

  fd = openat(dirfd, "existing_at.txt", O_RDONLY);
  if (fd < 0) {
    fail("three-argument openat");
  }
  expect_contents(fd, "openat-existing", "read after openat");
  close(fd);

  errno = 0;
  require(openat(-1, "relative.txt", O_RDONLY) == -1,
          "invalid-dirfd openat unexpectedly succeeded");
  require(errno == EBADF, "invalid-dirfd openat did not preserve EBADF");

  fd = openat(dirfd, "created_at.txt", O_WRONLY | O_CREAT | O_EXCL,
              (mode_t)0624);
  if (fd < 0) {
    fail("four-argument openat");
  }
  expect_mode(fd, (mode_t)0624, "fstat created openat file");
  close(fd);

  fd = openat64(dirfd, "existing_at64.txt", O_RDONLY);
  if (fd < 0) {
    fail("three-argument openat64");
  }
  expect_contents(fd, "openat64-existing", "read after openat64");
  close(fd);

  fd = openat64(dirfd, "created_at64.txt", O_WRONLY | O_CREAT | O_EXCL,
                (mode_t)0660);
  if (fd < 0) {
    fail("four-argument openat64");
  }
  expect_mode(fd, (mode_t)0660, "fstat created openat64 file");
  close(fd);

#ifdef O_TMPFILE
  errno = 0;
  fd = open(root, O_TMPFILE | O_RDWR, (mode_t)0603);
  if (fd >= 0) {
    expect_mode(fd, (mode_t)0603, "fstat O_TMPFILE file");
    close(fd);
  } else {
    require(tmpfile_unsupported(errno), "O_TMPFILE failed unexpectedly");
  }
#endif

  close(dirfd);
  umask(previous_umask);
  puts("linux-open-abi-ok");
  return 0;
}
