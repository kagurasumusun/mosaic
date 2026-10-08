/* brain-emu -- host-side byte I/O: the stdio source and the console facade.
 * SPDX-License-Identifier: LicenseRef-brain-emu-1.0
 */
#include "host/io/host_io.h"

#include <poll.h>
#include <unistd.h>

namespace brain::host {

StdioSource::StdioSource() {
  /* The emulator reads stdin only when a guest asks for a byte, so the fd is
   * polled with a zero timeout and never allowed to block the run loop. */
}

int StdioSource::get() {
  if (eof_) return -1;
  struct pollfd pfd;
  pfd.fd = STDIN_FILENO;
  pfd.events = POLLIN;
  pfd.revents = 0;
  if (poll(&pfd, 1, 0) <= 0) return -1;      /* nothing waiting (or error) */
  u8 b;
  const ssize_t n = ::read(STDIN_FILENO, &b, 1);
  if (n <= 0) {                                /* EOF or error: done for good  */
    eof_ = true;
    return -1;
  }
  return b;
}

HostIo& HostIo::instance() {
  static HostIo io;
  return io;
}

}  // namespace brain::host
