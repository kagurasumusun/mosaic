/* brain-emu -- the filesystem service: the in-memory filesystem.
 * SPDX-License-Identifier: LicenseRef-brain-emu-1.0
 *
 * `MemFs` holds files as strings in a map.  It is what a test uses for a guest
 * that reads a "file" and writes one back, and what the host uses to serve a
 * firmware blob without touching disk.  The rules match `HostFs`: create needs
 * a writer, a read-open of a missing file is -ENOENT, and a short read at end
 * of file is not an error.
 *
 * A handle works on its own copy of the file; a writable handle commits that
 * copy back on close, so a read-only handle can never mutate the file.
 */
#include "host/fs/fs.h"

#include <cstring>

namespace brain::host {

int MemFs::open(const char* path, u32 flags, u32* handle) {
  *handle = 0;
  if (!path || !*path) return -EINVAL;
  const bool rd = (flags & kFsRead) != 0;
  const bool wr = (flags & kFsWrite) != 0;
  if (!rd && !wr) return -EINVAL;
  if ((flags & kFsCreate) && !wr) return -EINVAL;

  auto it = files_.find(path);
  if (it == files_.end()) {
    if (!(flags & kFsCreate)) return -ENOENT;
    it = files_.emplace(path, std::string()).first;
  } else if (flags & kFsTruncate) {
    it->second.clear();
  }

  File f;
  f.path = path;
  f.writable = wr;
  f.data = it->second;
  f.pos = (flags & kFsAppend) ? f.data.size() : 0;
  const u32 h = next_handle_++;
  open_[h] = std::move(f);
  *handle = h;
  return 0;
}

int MemFs::close(u32 handle) {
  auto it = open_.find(handle);
  if (it == open_.end()) return -EBADF;
  if (it->second.writable) files_[it->second.path] = it->second.data;  /* commit */
  open_.erase(it);
  return 0;
}

int MemFs::read(u32 handle, void* buf, u32 n, u32* out_n) {
  *out_n = 0;
  auto it = open_.find(handle);
  if (it == open_.end()) return -EBADF;
  File& f = it->second;
  const u64 avail = f.pos < f.data.size() ? f.data.size() - f.pos : 0;
  const u32 take = n < avail ? n : (u32)avail;
  if (take) std::memcpy(buf, f.data.data() + f.pos, take);
  f.pos += take;
  *out_n = take;
  return 0;
}

int MemFs::write(u32 handle, const void* buf, u32 n, u32* out_n) {
  *out_n = 0;
  auto it = open_.find(handle);
  if (it == open_.end()) return -EBADF;
  File& f = it->second;
  if (!f.writable) return -EACCES;
  if (f.pos + n > f.data.size()) f.data.resize(f.pos + n);
  if (n) std::memcpy(&f.data[f.pos], buf, n);
  f.pos += n;
  *out_n = n;
  return 0;
}

int MemFs::seek(u32 handle, s64 offset, u32 whence, u64* out_pos) {
  auto it = open_.find(handle);
  if (it == open_.end()) return -EBADF;
  File& f = it->second;
  const s64 base = whence == kFsSeekSet ? 0
                   : whence == kFsSeekCur ? (s64)f.pos
                                          : (s64)f.data.size();
  const s64 pos = base + offset;
  if (pos < 0) return -EINVAL;
  f.pos = (u64)pos;
  if (out_pos) *out_pos = f.pos;
  return 0;
}

int MemFs::stat(const char* path, FsStat* st) {
  auto it = files_.find(path ? path : "");
  if (it == files_.end()) return -ENOENT;
  st->size = (u64)it->second.size();
  st->is_dir = false;
  return 0;
}

int MemFs::unlink(const char* path) {
  if (!path || files_.erase(path) == 0) return -ENOENT;
  return 0;
}

int MemFs::put(const char* path, const void* data, u32 n) {
  if (!path || !*path) return -EINVAL;
  files_[path] = std::string((const char*)data, n);
  return 0;
}

}  // namespace brain::host
