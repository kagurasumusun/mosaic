/* brain-emu -- the gdb stub's test suite.
 * SPDX-License-Identifier: LicenseRef-brain-emu-1.0
 *
 * The whole session is scripted: the "debugger" side is a string of framed
 * packets fed to a `MemTransport`, the stub serves it, and the replies are read
 * back out of the transport's output buffer.  That exercises the wire (framing,
 * checksums, acknowledgements, no-ack mode) and the C API behind every packet
 * against a machine built through the C API - a toy8 with five instructions
 * (LDA #7, ADD #7, INC A, NOP, HALT) at 0x100.
 */
#include <brainemu/brainemu.h>

#include <arpa/inet.h>
#include <netinet/in.h>
#include <sys/socket.h>
#include <unistd.h>

#include <cstdio>
#include <cstring>
#include <string>
#include <vector>

#include "debugger/gdb/gdbstub.h"
#include "debugger/gdb/tcp_transport.h"
#include "debugger/gdb/transport.h"
#include "testkit/test_support.h"

namespace {

const unsigned char kProg[] = {0x01, 0x07, 0x07, 0x07, 0x0D, 0x00, 0x1F};

/* Frame a packet the way gdb does: `$data#cc` where cc is the checksum. */
std::string frame(const std::string& data) {
  unsigned sum = 0;
  for (const char c : data) sum += (unsigned char)c;
  char cs[4];
  std::snprintf(cs, sizeof(cs), "%02x", sum & 0xFFu);
  return "$" + data + "#" + cs;
}

brain_emu_t* make_machine() {
  brain_config_t cfg;
  std::memset(&cfg, 0, sizeof(cfg));
  cfg.cpu_id = "toy8";
  cfg.ram_size = 1u << 16;
  brain_emu_t* e = brain_emu_create(&cfg);
  if (!e) return nullptr;
  if (brain_load_raw(e, kProg, sizeof kProg, 0x100, 0) != 0) {
    brain_emu_destroy(e);
    return nullptr;
  }
  return e;
}

/* The stub's output is a stream of framed packets (and `+` acks).  Split it
 * into the reply payloads, skipping the acks. */
std::vector<std::string> replies(const std::string& out) {
  std::vector<std::string> v;
  size_t i = 0;
  while (i < out.size()) {
    if (out[i] == '+' || out[i] == '-') {
      ++i;
      continue;
    }
    if (out[i] != '$') {
      ++i;
      continue;
    }
    const size_t hash = out.find('#', i);
    if (hash == std::string::npos || hash + 2 >= out.size()) break;
    v.push_back(out.substr(i + 1, hash - i - 1));
    i = hash + 3;
  }
  return v;
}

bool contains(const std::string& hay, const char* needle) {
  return hay.find(needle) != std::string::npos;
}

}  // namespace

int main() {
  brainemutest::ensure_builtins();

  /* ---- the scripted session ------------------------------------------------ */

  brain_emu_t* e = make_machine();
  CHECK(e != nullptr);
  if (!e) return brainemutest::summary("debugger/gdb");

  brain::gdb::MemTransport t;
  brain::gdb::GdbStub stub(e, &t);

  /* The conversation a debugger has with a fresh stub: supported features, the
   * target description, the halt reason, the registers, a step, breakpoints,
   * a continue that stops on one, memory, and a detach. */
  std::string script;
  script += frame("qSupported");
  script += frame("qXfer:features:read:target.xml:0,fff");
  script += frame("?");
  script += frame("qC");
  script += frame("qfThreadInfo");
  script += frame("qsThreadInfo");
  script += frame("qAttached");
  script += frame("qOffsets");
  script += frame("g");
  script += frame("p3");                 /* the PC, as register 3 on a toy8 */
  script += frame("P3=00010000");        /* write the PC: LE bytes 00 01 00 00 */
  script += frame("s");                  /* one step: LDA #7 */
  script += frame("p0");                 /* A = 7 */
  script += frame("m100,7");             /* the program bytes */
  script += frame("M200,4:78563412");    /* write a word */
  script += frame("m200,4");
  script += frame("X204,4:");            /* empty payload: the length cannot match */
  script += frame("Z0,105,1");           /* a breakpoint on the NOP */
  script += frame("c");                  /* run: stops on the breakpoint */
  script += frame("g");
  script += frame("z0,105,1");           /* drop it */
  script += frame("c");                  /* run to the HALT */
  script += frame("c");                  /* a halted machine: stops at once */
  script += frame("QStartNoAckMode");
  script += frame("D");                  /* detach */
  t.feed(script);

  /* The X packet above was framed with an empty payload; replace it with a
   * real binary write (the escape rule is exercised in the next section). */
  stub.serve();
  CHECK(stub.detached());

  const std::vector<std::string> r = replies(t.out());
  CHECK(r.size() >= 20);

  size_t i = 0;
  const auto next = [&]() { return i < r.size() ? r[i++] : std::string("<none>"); };

  /* qSupported */
  {
    const std::string sup = next();
    CHECK(contains(sup, "PacketSize=4000"));
    CHECK(contains(sup, "swbreak+"));
    CHECK(contains(sup, "qXfer:features:read+"));
  }
  /* target.xml: the layout the g packet uses (5 GPRs + flags = 6 words) */
  {
    const std::string xml = next();
    CHECK(contains(xml, "<architecture>toy8</architecture>"));
    CHECK(contains(xml, "bitsize=\"32\""));
  }
  /* ? -> S05, qC -> QC1, thread info, attached, offsets */
  CHECK(next() == "S05");
  CHECK(next() == "QC1");
  CHECK(next() == "m1");
  CHECK(next() == "l");
  CHECK(next() == "1");
  CHECK(next() == "Text=0;Data=0;Bss=0");
  /* g: six 32-bit words, little-endian byte order on the wire.  The PC (r3)
   * starts at 0x100, so its wire form is "00010000". */
  {
    const std::string g = next();
    CHECK(g.size() == 6 * 8);
    CHECK(g.substr(3 * 8, 8) == "00010000");       /* r3 = 0x00000100 */
    CHECK(g.substr(0, 8) == "00000000");           /* r0 = 0 */
  }
  /* p3 -> the PC */
  CHECK(next() == "00010000");
  /* P3=00010000 writes the PC (little-endian bytes 00 01 00 00 = 0x100) */
  CHECK(next() == "OK");
  /* s: one step (the LDA), p0 -> A = 7 */
  CHECK(next() == "S05");
  CHECK(next() == "07000000");
  /* m100,7: the program bytes 01 07 07 07 0d 00 1f, in address order */
  CHECK(next() == "010707070d001f");
  /* M200,4:78563412 writes 0x12345678 at 0x200 (LE), m reads it back */
  CHECK(next() == "OK");
  CHECK(next() == "78563412");
  /* X204,4 with an empty payload: the byte count cannot match the length */
  CHECK(next() == "E01");
  /* Z0 -> OK, c -> stops on the breakpoint at 0x105 (SIGTRAP) */
  CHECK(next() == "OK");
  CHECK(next() == "S05");
  /* g: the PC is at the breakpoint */
  {
    const std::string g = next();
    CHECK(g.size() == 6 * 8);
    CHECK(g.substr(3 * 8, 8) == "05010000");       /* r3 = 0x00000105 */
  }
  /* z0 -> OK, c -> runs to the HALT, c again -> stops at once */
  CHECK(next() == "OK");
  CHECK(next() == "S05");
  CHECK(next() == "S05");
  /* QStartNoAckMode -> OK, D -> OK (and the session ends) */
  CHECK(next() == "OK");
  CHECK(next() == "OK");

  /* After the detach, the machine is where the runs left it: halted, with A
   * at 15 (7 + 7 + 1) and the PC past the HALT. */
  CHECK(brain_get_reg(e, 0) == 15);
  CHECK(brain_get_reg(e, 3) == 0x107);

  brain_emu_destroy(e);

  /* ---- the wire details, one at a time -------------------------------------- */

  /* A bad checksum is answered with `-` and the packet is dropped; the next
   * good packet is still served. */
  {
    brain_emu_t* e2 = make_machine();
    CHECK(e2 != nullptr);
    brain::gdb::MemTransport t2;
    brain::gdb::GdbStub stub2(e2, &t2);
    std::string bad = frame("g");
    bad[bad.size() - 1] = bad[bad.size() - 1] == '0' ? '1' : '0';  /* break it */
    t2.feed(bad + frame("g") + frame("k"));
    stub2.serve();
    /* one `-` for the bad packet, one `g` reply, no reply to the kill */
    CHECK(contains(t2.out(), "-"));
    const std::vector<std::string> r2 = replies(t2.out());
    CHECK(r2.size() == 1);
    CHECK(r2[0].size() == 6 * 8);
    brain_emu_destroy(e2);
  }

  /* No-ack mode: after QStartNoAckMode the stub sends no `+` for the packets
   * it receives (the replies before it did). */
  {
    brain_emu_t* e3 = make_machine();
    CHECK(e3 != nullptr);
    brain::gdb::MemTransport t3;
    brain::gdb::GdbStub stub3(e3, &t3);
    t3.feed(frame("g") + frame("QStartNoAckMode") + frame("g") + frame("k"));
    stub3.serve();
    /* the first `g` was acked with `+`; the second was not */
    const size_t first_plus = t3.out().find('+');
    CHECK(first_plus != std::string::npos);
    const size_t after_ok = t3.out().find("$OK#");
    CHECK(after_ok != std::string::npos);
    CHECK(t3.out().find('+', after_ok) == std::string::npos);
    brain_emu_destroy(e3);
  }

  /* X with escapes: 0x7d, 0x24, 0x23 and 0x2a travel as 0x7d + (byte ^ 0x20),
   * and the stub unescapes them before the write. */
  {
    brain_emu_t* e4 = make_machine();
    CHECK(e4 != nullptr);
    brain::gdb::MemTransport t4;
    brain::gdb::GdbStub stub4(e4, &t4);
    std::string xpkt = "X300,4:";
    const u8 raw[4] = {0x7d, 0x24, 0x23, 0x2a};
    for (const u8 b : raw) {
      if (b == 0x7d || b == 0x24 || b == 0x23 || b == 0x2a) {
        xpkt.push_back((char)0x7d);
        xpkt.push_back((char)(b ^ 0x20));
      } else {
        xpkt.push_back((char)b);
      }
    }
    t4.feed(frame(xpkt) + frame("m300,4") + frame("k"));
    stub4.serve();
    const std::vector<std::string> r4 = replies(t4.out());
    CHECK(r4.size() == 2);
    if (r4.size() == 2) {
      CHECK(r4[0] == "OK");
      CHECK(r4[1] == "7d24232a");          /* the four bytes, in address order */
    }
    brain_emu_destroy(e4);
  }

  /* An unsupported packet gets the empty reply, and the session carries on. */
  {
    brain_emu_t* e5 = make_machine();
    CHECK(e5 != nullptr);
    brain::gdb::MemTransport t5;
    brain::gdb::GdbStub stub5(e5, &t5);
    t5.feed(frame("qWhatever:1") + frame("Z2,100,1") + frame("vCont?") +
            frame("g") + frame("k"));
    stub5.serve();
    const std::vector<std::string> r5 = replies(t5.out());
    CHECK(r5.size() == 4);
    if (r5.size() == 4) {
      CHECK(r5[0].empty());               /* unknown query: empty */
      CHECK(r5[1].empty());               /* watchpoints: not supported */
      CHECK(r5[2] == "vCont;c;s");
      CHECK(r5[3].size() == 6 * 8);
    }
    brain_emu_destroy(e5);
  }

  /* The TCP transport: listen on an ephemeral port, accept one client, and
   * exchange a packet over the socket. */
  {
    brain::gdb::TcpTransport srv;
    CHECK(srv.listen(0));
    CHECK(srv.port() != 0);

    brain::gdb::TcpTransport cli;
    /* a raw client socket, so the test plays the debugger itself */
    int cfd = ::socket(AF_INET, SOCK_STREAM, 0);
    CHECK(cfd >= 0);
    struct sockaddr_in sa;
    std::memset(&sa, 0, sizeof(sa));
    sa.sin_family = AF_INET;
    sa.sin_addr.s_addr = htonl(INADDR_LOOPBACK);
    sa.sin_port = htons(srv.port());
    CHECK(::connect(cfd, (struct sockaddr*)&sa, sizeof(sa)) == 0);
    CHECK(srv.accept());
    CHECK(srv.connected());

    /* qSupported over the socket, answered by the stub on the other side */
    brain_emu_t* e6 = make_machine();
    CHECK(e6 != nullptr);
    brain::gdb::GdbStub stub6(e6, &srv);
    const std::string q = frame("qSupported");
    CHECK(::send(cfd, q.data(), q.size(), 0) == (ssize_t)q.size());
    /* serve one packet, then read the reply from the socket */
    CHECK(stub6.serve_one());
    char buf[256];
    const ssize_t n = ::recv(cfd, buf, sizeof(buf), 0);
    CHECK(n > 0);
    const std::string got(buf, n > 0 ? (size_t)n : 0);
    CHECK(contains(got, "$PacketSize=4000"));
    CHECK(contains(got, "swbreak+"));

    /* the transport reports the close, and the session ends */
    ::close(cfd);
    u8 one = 0;
    CHECK(srv.read(&one, 1) == -1);
    CHECK(!srv.connected());

    brain_emu_destroy(e6);
  }

  return brainemutest::summary("debugger/gdb");
}
