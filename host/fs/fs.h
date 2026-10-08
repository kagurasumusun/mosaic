/* brain-emu -- the filesystem service.
 * SPDX-License-Identifier: LicenseRef-brain-emu-1.0
 *
 * The one abstraction a guest's file access and the host's own tooling share.
 * A semihosting call or a linux-user syscall on the guest side asks for "open
 * this path, read these bytes"; the host side answers with bytes from wherever
 * files actually live - a host directory (`HostFs`), or memory (`MemFs`, for
 * tests and firmware blobs).  The interface is the same for both, which is what
 * makes the guest-facing wiring independent of where the files are.
 *
 * Paths are slash-separated and relative to the filesystem's own root.  Errors
 * are negative errno-style codes (`-ENOENT` and friends), so a caller reports
 * them without a second error channel.  Handles are small integers the
 * implementation owns; 0 is never a valid handle.
 */
#ifndef BRAINEMU_HOST_FS_H
#define BRAINEMU_HOST_FS_H

#include <cerrno>
#include <map>
#include <string>
#include <vector>

#include "brainemu/types.h"

namespace brain::host {

/* open() flags (the classic set, not a bitfield of host flags). */
enum : u32 {
  kFsRead    = 1u << 0,
  kFsWrite   = 1u << 1,
  kFsCreate  = 1u << 2,
  kFsTruncate = 1u << 3,
  kFsAppend  = 1u << 4,
};

/* seek() whence. */
enum : u32 {
  kFsSeekSet = 0,
  kFsSeekCur = 1,
  kFsSeekEnd = 2,
};

struct FsStat {
  u64 size = 0;
  bool is_dir = false;
};

class Fs {
 public:
  virtual ~Fs() = default;

  virtual int open(const char* path, u32 flags, u32* handle) = 0;
  virtual int close(u32 handle) = 0;
  /* Read/write at most `n` bytes; `*out_n` says how many moved (a short read
   * at end of file is not an error). */
  virtual int read(u32 handle, void* buf, u32 n, u32* out_n) = 0;
  virtual int write(u32 handle, const void* buf, u32 n, u32* out_n) = 0;
  virtual int seek(u32 handle, s64 offset, u32 whence, u64* out_pos) = 0;
  virtual int stat(const char* path, FsStat* st) = 0;
  virtual int unlink(const char* path) = 0;

  /* Error text for a code, for logs and the monitor: static strings. */
  static const char* err_text(int code);
};

/* A filesystem rooted at a host directory.  Guest paths are resolved against
 * the root and can never escape it: ".." that would climb above the root is
 * refused with -EACCES, and absolute guest paths are treated as relative to
 * the root (the guest has no view of the host's filesystem outside it). */
class HostFs final : public Fs {
 public:
  /* `root` is copied.  An empty root refuses every path. */
  explicit HostFs(const char* root);
  ~HostFs() override;

  int open(const char* path, u32 flags, u32* handle) override;
  int close(u32 handle) override;
  int read(u32 handle, void* buf, u32 n, u32* out_n) override;
  int write(u32 handle, const void* buf, u32 n, u32* out_n) override;
  int seek(u32 handle, s64 offset, u32 whence, u64* out_pos) override;
  int stat(const char* path, FsStat* st) override;
  int unlink(const char* path) override;

  const char* root() const { return root_.c_str(); }

 private:
  /* Resolve a guest path against the root.  Returns false when the path would
   * escape the root or is otherwise unusable. */
  bool resolve(const char* path, std::string* host_path) const;
  struct File;
  File* file(u32 handle);
  std::string root_;
  std::vector<File*> files_;
};

/* A filesystem held in memory: a map of path to bytes.  What a test uses for a
 * guest that reads a "file", and what a firmware blob looks like when the host
 * wants to serve it without touching disk. */
class MemFs final : public Fs {
 public:
  int open(const char* path, u32 flags, u32* handle) override;
  int close(u32 handle) override;
  int read(u32 handle, void* buf, u32 n, u32* out_n) override;
  int write(u32 handle, const void* buf, u32 n, u32* out_n) override;
  int seek(u32 handle, s64 offset, u32 whence, u64* out_pos) override;
  int stat(const char* path, FsStat* st) override;
  int unlink(const char* path) override;

  /* Host-side seeding: write a whole file in one call. */
  int put(const char* path, const void* data, u32 n);

 private:
  /* An open handle works on its own copy of the bytes; a writable handle
   * commits the copy back to the file on close, so a read-only handle can
   * never mutate the file. */
  struct File {
    std::string path;
    std::string data;
    u64 pos = 0;
    bool writable = false;
  };
  std::map<std::string, std::string> files_;
  std::map<u32, File> open_;
  u32 next_handle_ = 1;
};

}  // namespace brain::host

#endif  /* BRAINEMU_HOST_FS_H */
