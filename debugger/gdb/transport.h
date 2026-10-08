/* brain-emu -- the gdb remote serial protocol's transport.
 * SPDX-License-Identifier: LicenseRef-brain-emu-1.0
 *
 * The stub talks the remote serial protocol (RSP) over *some* byte stream: a
 * TCP socket when a debugger connects from outside (`--gdb PORT`), a pair of
 * buffers when a test drives it (`MemTransport`).  `GdbTransport` is that
 * stream: `read` returns the bytes that are there (0 = nothing yet, -1 = the
 * stream is closed for good), `write` sends, and `connected` says whether the
 * peer is still there.  The stub itself knows nothing about sockets - it asks
 * for bytes and gets bytes, which is what makes it testable in-process.
 */
#ifndef BRAINEMU_DEBUGGER_GDB_TRANSPORT_H
#define BRAINEMU_DEBUGGER_GDB_TRANSPORT_H

#include <cstring>
#include <string>

#include "brainemu/types.h"

namespace brain::gdb {

class GdbTransport {
 public:
  virtual ~GdbTransport() = default;

  /* Read up to `n` bytes into `buf`.  Returns how many arrived, or -1 when the
   * stream is closed.  A transport handed to the stub must either block until a
   * byte arrives or report -1 (both shipped transports do: `TcpTransport` blocks
   * in recv, `MemTransport` reports -1 when its input is drained) - the stub
   * reads until a packet is complete or the stream ends. */
  virtual int read(u8* buf, u32 n) = 0;
  virtual void write(const u8* buf, u32 n) = 0;
  virtual bool connected() const = 0;
};

/* A transport over two buffers: what a test drives the stub with.  `feed`
 * queues bytes from the "debugger"; `out` collects what the stub sent.  When
 * the input is drained the transport reports closed, so a scripted session is
 * "feed everything, serve, read the output". */
class MemTransport final : public GdbTransport {
 public:
  void feed(const std::string& s) { in_ += s; }
  void feed(const void* p, u32 n) { in_.append((const char*)p, n); }
  const std::string& out() const { return out_; }
  void clear_out() { out_.clear(); }
  size_t in_pos() const { return in_pos_; }

  int read(u8* buf, u32 n) override {
    if (in_pos_ >= in_.size()) return -1;          /* drained: closed */
    const u32 avail = (u32)(in_.size() - in_pos_);
    const u32 k = n < avail ? n : avail;
    std::memcpy(buf, in_.data() + in_pos_, k);
    in_pos_ += k;
    return (int)k;
  }
  void write(const u8* buf, u32 n) override {
    out_.append((const char*)buf, n);
  }
  bool connected() const override { return in_pos_ < in_.size(); }

 private:
  std::string in_;
  std::string out_;
  size_t in_pos_ = 0;
};

}  // namespace brain::gdb

#endif  /* BRAINEMU_DEBUGGER_GDB_TRANSPORT_H */
