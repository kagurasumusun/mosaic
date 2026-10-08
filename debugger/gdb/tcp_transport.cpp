/* brain-emu -- the gdb transport over TCP: POSIX sockets, one client.
 * SPDX-License-Identifier: LicenseRef-brain-emu-1.0
 */
#include "debugger/gdb/tcp_transport.h"

#include <cerrno>
#include <cstring>

#include <arpa/inet.h>
#include <netinet/in.h>
#include <netinet/tcp.h>
#include <sys/socket.h>
#include <unistd.h>

namespace brain::gdb {

TcpTransport::~TcpTransport() {
  disconnect();
  if (listen_fd_ >= 0) ::close(listen_fd_);
}

bool TcpTransport::listen(u16 port) {
  if (listen_fd_ >= 0) return false;
  listen_fd_ = ::socket(AF_INET, SOCK_STREAM, 0);
  if (listen_fd_ < 0) return false;
  int one = 1;
  ::setsockopt(listen_fd_, SOL_SOCKET, SO_REUSEADDR, &one, sizeof(one));
  struct sockaddr_in sa;
  std::memset(&sa, 0, sizeof(sa));
  sa.sin_family = AF_INET;
  sa.sin_addr.s_addr = htonl(INADDR_ANY);       /* any interface: the debugger
                                                * may be another machine */
  sa.sin_port = htons(port);
  if (::bind(listen_fd_, (struct sockaddr*)&sa, sizeof(sa)) != 0) {
    ::close(listen_fd_);
    listen_fd_ = -1;
    return false;
  }
  if (::listen(listen_fd_, 1) != 0) {
    ::close(listen_fd_);
    listen_fd_ = -1;
    return false;
  }
  /* The port actually bound (matters when the caller asked for 0). */
  socklen_t len = sizeof(sa);
  if (::getsockname(listen_fd_, (struct sockaddr*)&sa, &len) == 0)
    port_ = ntohs(sa.sin_port);
  else
    port_ = port;
  return true;
}

bool TcpTransport::accept() {
  if (listen_fd_ < 0 || fd_ >= 0) return false;
  fd_ = ::accept(listen_fd_, nullptr, nullptr);
  if (fd_ < 0) return false;
  int one = 1;
  ::setsockopt(fd_, IPPROTO_TCP, TCP_NODELAY, &one, sizeof(one));
  return true;
}

int TcpTransport::read(u8* buf, u32 n) {
  if (fd_ < 0) return -1;
  const ssize_t r = ::recv(fd_, buf, n, 0);
  if (r <= 0) {                                 /* 0 = orderly close, <0 = error */
    disconnect();
    return -1;
  }
  return (int)r;
}

void TcpTransport::write(const u8* buf, u32 n) {
  if (fd_ < 0) return;
  u32 sent = 0;
  while (sent < n) {
    const ssize_t w = ::send(fd_, buf + sent, n - sent, MSG_NOSIGNAL);
    if (w <= 0) {
      disconnect();
      return;
    }
    sent += (u32)w;
  }
}

void TcpTransport::disconnect() {
  if (fd_ >= 0) {
    ::close(fd_);
    fd_ = -1;
  }
}

}  // namespace brain::gdb
