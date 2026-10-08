/* brain-emu -- host-side byte I/O: sinks, sources and the console facade.
 * SPDX-License-Identifier: LicenseRef-brain-emu-1.0
 *
 * The plumbing the host services share: a guest's output bytes need somewhere to
 * go (a UART's TX line, the console, a log file) and a guest's input bytes need
 * somewhere to come from (stdin, a script, a socket).  `ByteSink` and
 * `ByteSource` are those two shapes; the stdio and buffer implementations cover
 * the real console and every test.
 *
 * `HostIo` is the facade a service asks for when it wants "the console": out,
 * err and a non-blocking stdin.  Nothing in `devices/` depends on this - a
 * device takes plain function pointers (see devices/uart/) and the adapters at
 * the bottom of this file turn a sink/source into one.
 */
#ifndef BRAINEMU_HOST_IO_H
#define BRAINEMU_HOST_IO_H

#include <cstdio>
#include <string>

#include "brainemu/types.h"

namespace brain::host {

/* Where guest output bytes go. */
class ByteSink {
 public:
  virtual ~ByteSink() = default;
  virtual void put(u8 b) = 0;
  virtual void flush() {}
};

/* Where guest input bytes come from.  `get()` returns the byte, or -1 when
 * nothing is waiting; `exhausted()` says the source will never produce more
 * (a script that ran out), which is how a service knows to stop polling. */
class ByteSource {
 public:
  virtual ~ByteSource() = default;
  virtual int get() = 0;
  virtual bool exhausted() const { return false; }
};

/* ---- stdio -------------------------------------------------------------- */

class StdioSink final : public ByteSink {
 public:
  explicit StdioSink(std::FILE* f) : f_(f) {}
  void put(u8 b) override { std::fputc(b, f_); }
  void flush() override { std::fflush(f_); }

 private:
  std::FILE* f_;
};

/* stdin, polled: `get()` never blocks, so a device ticking the machine is
 * never held up by a keyboard that has nothing to say. */
class StdioSource final : public ByteSource {
 public:
  StdioSource();
  int get() override;
  bool exhausted() const override { return eof_; }

 private:
  bool eof_ = false;
};

/* ---- buffers (tests, scripts) -------------------------------------------- */

class BufferSink final : public ByteSink {
 public:
  void put(u8 b) override { buf_.push_back((char)b); }
  const std::string& str() const { return buf_; }
  void clear() { buf_.clear(); }

 private:
  std::string buf_;
};

class BufferSource final : public ByteSource {
 public:
  explicit BufferSource(std::string data) : data_(std::move(data)) {}
  int get() override {
    if (pos_ >= data_.size()) return -1;
    return (u8)data_[pos_++];
  }
  bool exhausted() const override { return pos_ >= data_.size(); }

 private:
  std::string data_;
  size_t pos_ = 0;
};

/* ---- the console facade --------------------------------------------------- */

class HostIo {
 public:
  static HostIo& instance();

  ByteSink& out() { return out_; }
  ByteSink& err() { return err_; }
  ByteSource& in() { return in_; }

 private:
  HostIo() = default;
  StdioSink out_{stdout};
  StdioSink err_{stderr};
  StdioSource in_;
};

/* ---- adapters: a sink/source as the plain function pointers a device takes -- */

struct SinkTx {
  static void put(void* user, u8 b) {
    static_cast<ByteSink*>(user)->put(b);
  }
};
struct SourceRx {
  static int get(void* user) {
    return static_cast<ByteSource*>(user)->get();
  }
};

}  // namespace brain::host

#endif  /* BRAINEMU_HOST_IO_H */
