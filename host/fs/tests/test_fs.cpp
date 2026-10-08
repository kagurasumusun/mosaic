/* brain-emu -- host/fs tests: both filesystems, the same contract.
 * SPDX-License-Identifier: LicenseRef-brain-emu-1.0
 *
 * `HostFs` runs against a scratch directory under /tmp (made with mkdtemp and
 * removed at the end), and `MemFs` holds its files in memory.  Both are driven
 * through the same `Fs` interface with the same assertions, which is the point
 * of the abstraction: a guest-side caller cannot tell which one it is talking
 * to.
 */
#include <cerrno>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <string>
#include <unistd.h>

#include <sys/stat.h>

#include "host/fs/fs.h"
#include "testkit/test_support.h"

namespace {

using brain::host::Fs;
using brain::host::FsStat;
using brain::host::HostFs;
using brain::host::MemFs;
using brain::host::kFsAppend;
using brain::host::kFsCreate;
using brain::host::kFsRead;
using brain::host::kFsSeekCur;
using brain::host::kFsSeekEnd;
using brain::host::kFsSeekSet;
using brain::host::kFsTruncate;
using brain::host::kFsWrite;

/* One shared contract, exercised against a `Fs`.  `host` is true when the
 * filesystem is HostFs (MemFs has no directories and its paths are flat). */
void run_contract(Fs& fs, bool host) {
  const char* path = "sub/dir/hello.txt";

  /* Seed the file through the interface itself, so the write+create path is
   * under test too. */
  u32 w = 0;
  CHECK(fs.open(path, kFsWrite | kFsCreate | kFsTruncate, &w) == 0);
  u32 n = 0;
  CHECK(fs.write(w, "hello, fs", 9, &n) == 0 && n == 9);
  CHECK(fs.close(w) == 0);

  /* Read it back, in two short reads. */
  u32 h = 0;
  CHECK(fs.open(path, kFsRead, &h) == 0);
  CHECK(h != 0);
  char buf[64];
  CHECK(fs.read(h, buf, 5, &n) == 0 && n == 5);
  CHECK(std::memcmp(buf, "hello", 5) == 0);
  CHECK(fs.read(h, buf, 50, &n) == 0 && n == 4);   /* short read at EOF */
  CHECK(std::memcmp(buf, ", fs", 4) == 0);
  CHECK(fs.read(h, buf, 50, &n) == 0 && n == 0);   /* EOF: not an error */
  CHECK(fs.close(h) == 0);
  CHECK(fs.close(h) == -EBADF);                    /* double close */

  /* Seek around. */
  CHECK(fs.open(path, kFsRead, &h) == 0);
  u64 pos = 0;
  CHECK(fs.seek(h, 2, kFsSeekCur, &pos) == 0 && pos == 2);
  CHECK(fs.read(h, buf, 3, &n) == 0 && n == 3 && std::memcmp(buf, "llo", 3) == 0);
  CHECK(fs.seek(h, -1, kFsSeekEnd, &pos) == 0 && pos == 8);
  CHECK(fs.seek(h, 0, kFsSeekSet, &pos) == 0 && pos == 0);
  CHECK(fs.close(h) == 0);

  /* Stat. */
  FsStat st;
  CHECK(fs.stat(path, &st) == 0);
  CHECK(st.size == 9);
  CHECK(!st.is_dir);
  CHECK(fs.stat("no/such/file", &st) == -ENOENT);

  /* Append to the file. */
  CHECK(fs.open(path, kFsWrite | kFsAppend, &w) == 0);
  CHECK(fs.write(w, "!!", 2, &n) == 0 && n == 2);
  CHECK(fs.close(w) == 0);
  CHECK(fs.stat(path, &st) == 0 && st.size == 11);

  /* Truncate on open. */
  CHECK(fs.open(path, kFsWrite | kFsTruncate, &w) == 0);
  CHECK(fs.write(w, "xy", 2, &n) == 0);
  CHECK(fs.close(w) == 0);
  CHECK(fs.stat(path, &st) == 0 && st.size == 2);

  /* A read-open of a missing file, a write-open without create, and a
   * neither-read-nor-write open. */
  CHECK(fs.open("missing.bin", kFsRead, &h) == -ENOENT);
  CHECK(fs.open("missing.bin", kFsWrite, &w) == -ENOENT);
  CHECK(fs.open(path, 0, &h) == -EINVAL);

  /* The root is a sandbox: ".." may not climb out of it. */
  if (host) {
    CHECK(fs.open("../hello.txt", kFsRead, &h) == -EACCES);
    CHECK(fs.open("sub/dir/../../../hello.txt", kFsRead, &h) == -EACCES);
    /* But ".." that stays inside the root is fine. */
    CHECK(fs.open("sub/dir/../dir/hello.txt", kFsRead, &h) == 0);
    CHECK(fs.close(h) == 0);
    /* A leading '/' is relative to the root, not to the host's. */
    CHECK(fs.open("/sub/dir/hello.txt", kFsRead, &h) == 0);
    CHECK(fs.close(h) == 0);
    /* Directories are reported as directories. */
    CHECK(fs.stat("sub", &st) == 0 && st.is_dir);
  }
  /* Unlink. */
  CHECK(fs.unlink(path) == 0);
  CHECK(fs.unlink(path) == -ENOENT);
  CHECK(fs.stat(path, &st) == -ENOENT);

}

}  // namespace

int main() {
  /* HostFs in a scratch directory under /tmp. */
  char tmpl[] = "/tmp/brainemu_fs_test_XXXXXX";
  const char* root = ::mkdtemp(tmpl);
  CHECK(root != nullptr);
  if (root) {
    const std::string r = root;
    ::mkdir((r + "/sub").c_str(), 0755);
    ::mkdir((r + "/sub/dir").c_str(), 0755);
    {
      HostFs fs(r.c_str());
      CHECK(fs.root() == r);
      run_contract(fs, true);
    }
    /* Remove the scratch tree (the contract left it empty except the dirs). */
    ::rmdir((r + "/sub/dir").c_str());
    ::rmdir((r + "/sub").c_str());
    ::rmdir(r.c_str());
  }

  /* MemFs needs no disk at all. */
  {
    MemFs fs;
    run_contract(fs, false);
    /* Seeding and error text. */
    CHECK(fs.put("blob.bin", "abc", 3) == 0);
    FsStat st;
    CHECK(fs.stat("blob.bin", &st) == 0 && st.size == 3);
    CHECK(std::strcmp(Fs::err_text(0), "ok") == 0);
    CHECK(std::strcmp(Fs::err_text(-ENOENT), "no such file or directory") == 0);
  }

  return brainemutest::summary("host/fs");
}
