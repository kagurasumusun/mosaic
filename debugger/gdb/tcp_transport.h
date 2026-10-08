/* brain-emu -- the gdb transport over TCP.
 * SPDX-License-Identifier: LicenseRef-brain-emu-1.0
 *
 * `TcpTransport` is the `--gdb PORT` half of the stub: it listens on a port,
 * accepts one debugger (the CLI waits for the connection before it serves), and
 * then reads and writes that one socket.  One client at a time is the honest
 * scope for a stub that owns a machine - a second connection is refused while
 * the first is served, and a disconnect ends the session, which is how the CLI
 * knows to go on with a normal run.
 */
#ifndef BRAINEMU_DEBUGGER_GDB_TCP_TRANSPORT_H
#define BRAINEMU_DEBUGGER_GDB_TCP_TRANSPORT_H

#include "debugger/gdb/transport.h"

namespace brain::gdb {

class TcpTransport final : public GdbTransport {
 public:
  TcpTransport() = default;
  ~TcpTransport() override;

  TcpTransport(const TcpTransport&) = delete;
  TcpTransport& operator=(const TcpTransport&) = delete;

  /* Listen on `port` (0 = any free port; `port()` then says which).  Returns
   * false when the socket cannot be created or bound. */
  bool listen(u16 port);
  /* Block until a debugger connects.  Returns false on error. */
  bool accept();

  u16 port() const { return port_; }
  bool listening() const { return listen_fd_ >= 0; }

  int read(u8* buf, u32 n) override;
  void write(const u8* buf, u32 n) override;
  bool connected() const override { return fd_ >= 0; }

  /* Close the client socket (the listener stays, for a later session). */
  void disconnect();

 private:
  int listen_fd_ = -1;
  int fd_ = -1;
  u16 port_ = 0;
};

}  // namespace brain::gdb

#endif  /* BRAINEMU_DEBUGGER_GDB_TCP_TRANSPORT_H */
