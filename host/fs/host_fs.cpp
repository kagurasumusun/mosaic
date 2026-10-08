/* brain-emu -- the filesystem service: a filesystem rooted at a host directory.
 * SPDX-License-Identifier: LicenseRef-brain-emu-1.0
 *
 * `HostFs` is the sandbox: a guest path is resolved component by component
 * against the root, "." is dropped, ".." pops one component and is refused when
 * it would climb above the root, and a leading '/' is ignored (the guest has no
 * absolute view of the host).  What survives is a path inside the root, and only
 * that reaches the host's open/read/write/lseek.
 *
 * The file layer is POSIX fds, not FILE*: the flag set (create/truncate/append
 * without reading) has no exact fopen spelling, and `read`/`write`/`lseek` are
 * the same calls the interface already names.
 */
#include "host/fs/fs.h"

#include <cerrno>
#include <cstring>
#include <fcntl.h>
#include <sys/stat.h>
#include <unistd.h>

namespace brain::host {

struct HostFs::File {
  int fd = -1;
};

const char* Fs::err_text(int code) {
  switch (-code) {
    case 0:      return "ok";
    case ENOENT: return "no such file or directory";
    case EACCES: return "permission denied";
    case EBADF:  return "bad file handle";
    case EEXIST: return "file exists";
    case ENOSPC: return "no space left on device";
    case EISDIR: return "is a directory";
    case ENOTDIR: return "not a directory";
    case EINVAL: return "invalid argument";
    case ENODEV: return "no such device";
    default:     return "filesystem error";
  }
}

HostFs::HostFs(const char* root) : root_(root ? root : "") {
  /* Drop a trailing slash so joining is always root + "/" + path. */
  while (root_.size() > 1 && root_.back() == '/') root_.pop_back();
}

HostFs::~HostFs() {
  for (File* f : files_)
    if (f) {
      if (f->fd >= 0) ::close(f->fd);
      delete f;
    }
}

bool HostFs::resolve(const char* path, std::string* host_path) const {
  if (!path || !*path || root_.empty()) return false;
  /* Split into components, dropping "." and resolving ".." against the parts
   * already accepted.  A ".." with nothing to pop means an escape. */
  std::vector<std::string> parts;
  const char* p = path;
  while (*p) {
    while (*p == '/') ++p;
    const char* start = p;
    while (*p && *p != '/') ++p;
    const std::string comp(start, p - start);
    if (comp.empty() || comp == ".") continue;
    if (comp == "..") {
      if (parts.empty()) return false;      /* would climb above the root */
      parts.pop_back();
      continue;
    }
    parts.push_back(comp);
  }
  std::string joined = root_;
  for (const std::string& c : parts) {
    joined.push_back('/');
    joined += c;
  }
  *host_path = joined;
  return true;
}

HostFs::File* HostFs::file(u32 handle) {
  /* Handles are 1-based indices into files_. */
  if (handle == 0 || handle > files_.size()) return nullptr;
  return files_[handle - 1];
}

int HostFs::open(const char* path, u32 flags, u32* handle) {
  *handle = 0;
  std::string host;
  if (!resolve(path, &host)) return -EACCES;

  const bool rd = (flags & kFsRead) != 0;
  const bool wr = (flags & kFsWrite) != 0;
  if (!rd && !wr) return -EINVAL;
  if ((flags & kFsCreate) && !wr) return -EINVAL;   /* create needs a writer */

  int o_flags = rd ? (wr ? O_RDWR : O_RDONLY) : O_WRONLY;
  if (flags & kFsCreate) o_flags |= O_CREAT;
  if (flags & kFsTruncate) o_flags |= O_TRUNC;
  if (flags & kFsAppend) o_flags |= O_APPEND;

  const int fd = ::open(host.c_str(), o_flags, 0644);
  if (fd < 0) return -errno;

  files_.push_back(new File{fd});
  *handle = (u32)files_.size();
  return 0;
}

int HostFs::close(u32 handle) {
  File* f = file(handle);
  if (!f || f->fd < 0) return -EBADF;   /* unknown handle, or already closed */
  ::close(f->fd);
  f->fd = -1;
  return 0;
}

int HostFs::read(u32 handle, void* buf, u32 n, u32* out_n) {
  *out_n = 0;
  File* f = file(handle);
  if (!f || f->fd < 0) return -EBADF;
  const ssize_t got = ::read(f->fd, buf, n);
  if (got < 0) return -errno;
  *out_n = (u32)got;
  return 0;
}

int HostFs::write(u32 handle, const void* buf, u32 n, u32* out_n) {
  *out_n = 0;
  File* f = file(handle);
  if (!f || f->fd < 0) return -EBADF;
  const ssize_t put = ::write(f->fd, buf, n);
  if (put < 0) return -errno;
  *out_n = (u32)put;
  return *out_n == n ? 0 : -ENOSPC;
}

int HostFs::seek(u32 handle, s64 offset, u32 whence, u64* out_pos) {
  File* f = file(handle);
  if (!f || f->fd < 0) return -EBADF;
  int w = whence == kFsSeekSet ? SEEK_SET : whence == kFsSeekCur ? SEEK_CUR : SEEK_END;
  const off_t pos = ::lseek(f->fd, (off_t)offset, w);
  if (pos < 0) return -EINVAL;
  if (out_pos) *out_pos = (u64)pos;
  return 0;
}

int HostFs::stat(const char* path, FsStat* st) {
  std::string host;
  if (!resolve(path, &host)) return -EACCES;
  struct stat sb;
  if (::stat(host.c_str(), &sb) != 0) return -errno;
  st->size = (u64)sb.st_size;
  st->is_dir = S_ISDIR(sb.st_mode) != 0;
  return 0;
}

int HostFs::unlink(const char* path) {
  std::string host;
  if (!resolve(path, &host)) return -EACCES;
  if (::unlink(host.c_str()) != 0) return -errno;
  return 0;
}

}  // namespace brain::host
