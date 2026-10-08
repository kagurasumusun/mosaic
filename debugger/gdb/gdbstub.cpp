/* brain-emu -- the gdb stub: the remote serial protocol over the C API.
 * SPDX-License-Identifier: LicenseRef-brain-emu-1.0
 *
 * One packet in, one packet out, all of it C API calls.  The wire details that
 * are easy to get wrong and are pinned down here:
 *
 *   - registers travel as hex of their bytes in target byte order, which for
 *     this project's little-endian targets means a 32-bit value 0x12345678 goes
 *     on the wire as "78563412";
 *   - memory (`m`) is hex of the bytes in address order - no swapping;
 *   - `X` (binary write) escapes 0x7d, 0x24, 0x23 and 0x2a as 0x7d followed by
 *     the byte XOR 0x20, and the stub unescapes on the way in;
 *   - an unsupported packet gets an empty reply (`$#00`), which is how RSP says
 *     "no", and never an error string;
 *   - `+`/`-` acknowledgements from the debugger are consumed while waiting
 *     for a packet, and the stub stops acknowledging once `QStartNoAckMode` is
 *     accepted.
 */
#include "debugger/gdb/gdbstub.h"

#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <string>
#include <vector>

namespace brain::gdb {

namespace {

constexpr u32 kMaxMemBytes = 1024;   /* one m/M reply: hex doubles the size */

int hexval(char c) {
  if (c >= '0' && c <= '9') return c - '0';
  if (c >= 'a' && c <= 'f') return c - 'a' + 10;
  if (c >= 'A' && c <= 'F') return c - 'A' + 10;
  return -1;
}

}  // namespace

/* ---- numbers -------------------------------------------------------------- */

bool GdbStub::parse_hex(const std::string& s, u32* out) {
  if (s.empty()) return false;
  size_t i = 0;
  if (s.size() > 2 && s[0] == '0' && (s[1] == 'x' || s[1] == 'X')) i = 2;
  if (i >= s.size()) return false;
  u64 v = 0;
  for (; i < s.size(); ++i) {
    const int d = hexval(s[i]);
    if (d < 0) return false;
    v = v * 16 + (u64)d;
    if (v > 0xFFFFFFFFull) return false;
  }
  *out = (u32)v;
  return true;
}

bool GdbStub::parse_u32(const std::string& s, u32* out) {
  return parse_hex(s, out);
}

/* A register on the wire is eight hex characters: four bytes in target byte
 * order, so "00010000" is the value 0x00000100, not 0x10000. */
static bool reg_from_hex(const std::string& s, u32* out) {
  if (s.size() != 8) return false;
  u32 v = 0;
  for (int b = 0; b < 4; ++b) {
    const int hi = hexval(s[2 * b]);
    const int lo = hexval(s[2 * b + 1]);
    if (hi < 0 || lo < 0) return false;
    v |= (u32)((hi << 4) | lo) << (8 * b);
  }
  *out = v;
  return true;
}

std::string GdbStub::to_hex(u32 v) {
  /* little-endian byte order on the wire */
  char buf[9];
  for (int i = 0; i < 4; ++i)
    std::snprintf(buf + 2 * i, 3, "%02x", (v >> (8 * i)) & 0xFFu);
  return std::string(buf, 8);
}

std::string GdbStub::hex_encode(const u8* p, u32 n) {
  std::string out;
  out.reserve(n * 2);
  static const char* d = "0123456789abcdef";
  for (u32 i = 0; i < n; ++i) {
    out.push_back(d[p[i] >> 4]);
    out.push_back(d[p[i] & 0xF]);
  }
  return out;
}

bool GdbStub::hex_decode(const std::string& s, std::vector<u8>* out) {
  out->clear();
  if (s.size() % 2) return false;
  out->reserve(s.size() / 2);
  for (size_t i = 0; i < s.size(); i += 2) {
    const int hi = hexval(s[i]), lo = hexval(s[i + 1]);
    if (hi < 0 || lo < 0) return false;
    out->push_back((u8)((hi << 4) | lo));
  }
  return true;
}

/* ---- the wire ------------------------------------------------------------- */

bool GdbStub::read_packet(std::string* pkt) {
  pkt->clear();
  u8 b = 0;
  for (;;) {
    const int r = t_->read(&b, 1);
    if (r <= 0) return false;                 /* closed (or nothing: session over) */
    if (b == '+' || b == '-') continue;       /* the debugger's acknowledgements */
    if (b == 0x03) {                          /* Ctrl-C: the interrupt byte */
      send_packet("S02");
      continue;
    }
    if (b != '$') continue;                   /* noise between packets */
    std::string data;
    for (;;) {
      if (t_->read(&b, 1) <= 0) return false;
      if (b == '#') break;
      data.push_back((char)b);
    }
    char cs[2] = {0, 0};
    if (t_->read((u8*)cs, 1) <= 0) return false;
    if (t_->read((u8*)cs + 1, 1) <= 0) return false;
    u32 sum = 0;
    for (const char c : data) sum += (u8)c;
    char want[3];
    std::snprintf(want, sizeof(want), "%02x", sum & 0xFFu);
    if (cs[0] != want[0] || cs[1] != want[1]) {
      if (!no_ack_) send_raw("-", 1);         /* bad checksum: ask for a resend */
      continue;
    }
    if (!no_ack_) send_raw("+", 1);           /* good packet: acknowledged */
    *pkt = data;
    return true;
  }
}

void GdbStub::send_packet(const std::string& data) {
  u32 sum = 0;
  for (const char c : data) sum += (u8)c;
  char tail[4];
  std::snprintf(tail, sizeof(tail), "#%02x", sum & 0xFFu);   /* "#xx" + NUL */
  send_raw("$", 1);
  if (!data.empty()) send_raw(data.data(), (u32)data.size());
  send_raw(tail, 3);
}

/* ---- registers ------------------------------------------------------------ */

std::vector<int> GdbStub::reg_layout() const {
  /* The core's own order: the GPRs, then the status word, then the saved status
   * word when the CPU has one.  target.xml describes this same list. */
  std::vector<int> v;
  const u32 n = brain_cpu_reg_count(e_);
  for (u32 i = 0; i < n; ++i) v.push_back((int)i);
  const int st = brain_cpu_status_reg(e_);
  if (st >= 0) v.push_back(st);
  const int sv = brain_cpu_saved_status_reg(e_);
  if (sv >= 0) v.push_back(sv);
  return v;
}

std::string GdbStub::on_reg_read_all() {
  const std::vector<int> layout = reg_layout();
  std::string out;
  out.reserve(layout.size() * 8);
  for (const int idx : layout) out += to_hex(brain_get_reg(e_, idx));
  return out;
}

std::string GdbStub::on_reg_write_all(const std::string& hex) {
  const std::vector<int> layout = reg_layout();
  if (hex.size() != layout.size() * 8) return "E01";
  for (size_t i = 0; i < layout.size(); ++i) {
    u32 v = 0;
    if (!reg_from_hex(hex.substr(i * 8, 8), &v)) return "E01";
    if (brain_set_reg(e_, layout[i], v) != 0) return "E01";
  }
  return "OK";
}

std::string GdbStub::on_reg_read(const std::string& args) {
  u32 n;
  if (!parse_hex(args, &n)) return "E01";
  const std::vector<int> layout = reg_layout();
  if (n >= layout.size()) return "E01";
  return to_hex(brain_get_reg(e_, layout[n]));
}

std::string GdbStub::on_reg_write(const std::string& args) {
  const size_t eq = args.find('=');
  if (eq == std::string::npos) return "E01";
  u32 n, v;
  if (!parse_hex(args.substr(0, eq), &n) ||
      !reg_from_hex(args.substr(eq + 1), &v))
    return "E01";
  const std::vector<int> layout = reg_layout();
  if (n >= layout.size()) return "E01";
  return brain_set_reg(e_, layout[n], v) == 0 ? "OK" : "E01";
}

std::string GdbStub::reg_layout_xml() const {
  const bool arm = std::strstr(brain_cpu_isa(e_), "ARM") != nullptr;
  const std::vector<int> layout = reg_layout();
  const int st = brain_cpu_status_reg(e_);
  const int sv = brain_cpu_saved_status_reg(e_);
  const u32 gprs = brain_cpu_reg_count(e_);

  std::string x =
      "<?xml version=\"1.0\"?>\n<!DOCTYPE target SYSTEM \"gdb-target.dtd\">\n<target>\n";
  if (arm) {
    x += "  <architecture>arm</architecture>\n";
    x += "  <feature name=\"org.gnu.gdb.arm.core\">\n";
  } else {
    x += "  <architecture>";
    x += brain_cpu_id(e_);
    x += "</architecture>\n  <feature name=\"brain.core\">\n";
  }
  for (u32 i = 0; i < layout.size(); ++i) {
    const char* name = nullptr;
    char rname[16];
    if (arm && i < 16) {
      std::snprintf(rname, sizeof(rname), "r%u", i);
      name = rname;
    } else if (arm && st >= 0 && layout[i] == st) {
      name = "cpsr";
    } else if (arm && sv >= 0 && layout[i] == sv) {
      name = "spsr";
    } else if (i < gprs) {
      std::snprintf(rname, sizeof(rname), "r%u", i);
      name = rname;
    } else if (st >= 0 && layout[i] == st) {
      name = "status";
    } else {
      name = "saved_status";
    }
    char line[128];
    std::snprintf(line, sizeof(line),
                  "    <reg name=\"%s\" bitsize=\"32\" regnum=\"%u\"/>\n", name,
                  (u32)i);
    x += line;
  }
  x += "  </feature>\n</target>\n";
  return x;
}

std::string GdbStub::on_target_xml(u32 offset, u32 len) {
  const std::string xml = reg_layout_xml();
  if (offset >= xml.size()) return "l";
  const u32 avail = (u32)xml.size() - offset;
  const u32 take = len < avail ? len : avail;
  const std::string chunk = xml.substr(offset, take);
  return (offset + take >= xml.size() ? "l" : "m") + chunk;
}

/* ---- memory --------------------------------------------------------------- */

std::string GdbStub::on_mem_read(const std::string& args) {
  const size_t comma = args.find(',');
  if (comma == std::string::npos) return "E01";
  u32 addr, len;
  if (!parse_hex(args.substr(0, comma), &addr) ||
      !parse_hex(args.substr(comma + 1), &len))
    return "E01";
  if (len > kMaxMemBytes) len = kMaxMemBytes;
  if (len == 0) return "";
  std::vector<u8> bytes(len);
  if (brain_read_mem(e_, addr, bytes.data(), len) != 0) {
    /* Not RAM/ROM: read it the way the guest does, so a device's registers
     * answer.  A read of an unmapped address takes a data abort in the machine,
     * exactly as a guest read would - the stub does not hide that. */
    for (u32 i = 0; i < len; i += 4) {
      const u32 w = brain_read32(e_, addr + i);
      for (u32 b = 0; b < 4 && i + b < len; ++b)
        bytes[i + b] = (u8)(w >> (8 * b));
    }
  }
  return hex_encode(bytes.data(), len);
}

std::string GdbStub::on_mem_write_hex(const std::string& args) {
  const size_t colon = args.find(':');
  const size_t comma = args.find(',');
  if (colon == std::string::npos || comma == std::string::npos ||
      comma > colon)
    return "E01";
  u32 addr, len;
  if (!parse_hex(args.substr(0, comma), &addr) ||
      !parse_hex(args.substr(comma + 1, colon - comma - 1), &len))
    return "E01";
  std::vector<u8> bytes;
  if (!hex_decode(args.substr(colon + 1), &bytes) || bytes.size() != len)
    return "E01";
  return brain_write_mem(e_, addr, bytes.data(), len) == 0 ? "OK" : "E01";
}

std::string GdbStub::on_mem_write_bin(const std::string& args) {
  const size_t colon = args.find(':');
  const size_t comma = args.find(',');
  if (colon == std::string::npos || comma == std::string::npos ||
      comma > colon)
    return "E01";
  u32 addr, len;
  if (!parse_hex(args.substr(0, comma), &addr) ||
      !parse_hex(args.substr(comma + 1, colon - comma - 1), &len))
    return "E01";
  /* Binary data may contain ':' bytes, so the frame is split at the first one;
   * the payload is the rest, with 0x7d escapes undone. */
  std::vector<u8> bytes;
  const std::string payload = args.substr(colon + 1);
  bytes.reserve(payload.size());
  for (size_t i = 0; i < payload.size(); ++i) {
    if (payload[i] == '}' && i + 1 < payload.size()) {
      bytes.push_back((u8)(payload[++i] ^ 0x20));
    } else {
      bytes.push_back((u8)payload[i]);
    }
  }
  if (bytes.size() != len) return "E01";
  return brain_write_mem(e_, addr, bytes.data(), len) == 0 ? "OK" : "E01";
}

/* ---- breakpoints, running -------------------------------------------------- */

bool GdbStub::hit_break(u32 pc) const {
  for (const u32 b : breaks_)
    if (b == pc) return true;
  return false;
}

std::string GdbStub::on_breakpoint(const std::string& args, bool set) {
  /* Z0/z0 software, Z1/z1 hardware: both are honoured the same way (the stub's
   * own vector, checked by the continue loop).  Types 2-4 are not supported.
   * The packet is "TYPE,ADDR,KIND": the address sits between the first and the
   * second comma. */
  const size_t c1 = args.find(',');
  if (c1 == std::string::npos || c1 + 1 >= args.size()) return "E01";
  const size_t c2 = args.find(',', c1 + 1);
  if (c2 == std::string::npos) return "E01";
  u32 addr;
  if (!parse_hex(args.substr(c1 + 1, c2 - c1 - 1), &addr)) return "E01";
  if (set) {
    for (const u32 b : breaks_)
      if (b == addr) return "OK";
    breaks_.push_back(addr);
    return "OK";
  }
  for (size_t i = 0; i < breaks_.size(); ++i)
    if (breaks_[i] == addr) {
      breaks_.erase(breaks_.begin() + i);
      return "OK";
    }
  return "OK";   /* removing one that is not set is not an error */
}

std::string GdbStub::on_continue(const std::string& args) {
  (void)args;   /* a continue address (`c addr`) is accepted and ignored */
  if (breaks_.empty()) {
    /* No breakpoints: run slices and report when the machine stops on its own
     * (an exit device, a fault - `brain_run_insns` returning 0 is how the
     * caller learns). */
    for (;;) {
      if (brain_run_insns(e_, 1024) == 0) return "S05";
      if (detached_) return "S05";
    }
  }
  /* Breakpoints set: step and check the PC after every instruction - a
   * breakpoint is a stop, not a filter. */
  const int pci = brain_cpu_pc_reg(e_);
  for (;;) {
    if (pci >= 0 && hit_break(brain_get_reg(e_, pci))) return "S05";
    if (brain_step(e_) != 0) return "S05";
    if (detached_) return "S05";
  }
}

std::string GdbStub::on_step(const std::string& args) {
  (void)args;
  if (brain_step(e_) != 0) return "S05";
  return "S05";
}

/* ---- queries --------------------------------------------------------------- */

std::string GdbStub::on_q_supported(const std::string& args) {
  (void)args;
  return "PacketSize=4000;swbreak+;hwbreak+;QStartNoAckMode+;qXfer:features:read+";
}

std::string GdbStub::on_query(const std::string& name, const std::string& args) {
  if (name == "Supported") return on_q_supported(args);
  if (name == "Xfer") {
    /* qXfer:OBJECT:OPERATION:ANNEX:OFFSET,LENGTH - the object and the operation
     * come as two more colon-separated fields before the annex. */
    const size_t c1 = args.find(':');
    if (c1 == std::string::npos) return "";
    const std::string object = args.substr(0, c1);        /* "features" */
    const std::string rest = args.substr(c1 + 1);         /* "read:ANNEX:OFF,LEN" */
    const size_t c2 = rest.find(':');
    if (c2 == std::string::npos) return "";
    const std::string operation = rest.substr(0, c2);     /* "read" */
    const std::string rest2 = rest.substr(c2 + 1);        /* "ANNEX:OFF,LEN" */
    const size_t c3 = rest2.find(':');
    if (c3 == std::string::npos) return "";
    const std::string annex = rest2.substr(0, c3);
    const std::string off_len = rest2.substr(c3 + 1);
    const size_t comma = off_len.find(',');
    if (comma == std::string::npos) return "";
    u32 off, len;
    if (!parse_hex(off_len.substr(0, comma), &off) ||
        !parse_hex(off_len.substr(comma + 1), &len))
      return "";
    if (object == "features" && operation == "read" && annex == "target.xml")
      return on_target_xml(off, len);
    return "l";                        /* an annex the stub does not carry: end */
  }
  if (name == "C") return "QC1";                       /* the one thread */
  if (name == "fThreadInfo") return "m1";
  if (name == "sThreadInfo") return "l";
  if (name == "Attached") return "1";
  if (name == "Offsets") return "Text=0;Data=0;Bss=0";
  if (name == "Symbol") return "OK";
  return "";                            /* unsupported: the empty packet says so */
}

/* ---- the session ------------------------------------------------------------ */

bool GdbStub::serve_one() {
  std::string pkt;
  if (!read_packet(&pkt)) return false;
  if (detached_) return false;

  std::string reply;
  bool reply_expected = true;
  if (pkt.empty()) {
    reply = "";
  } else if (pkt[0] == '?') {
    reply = "S05";                       /* why halted: SIGTRAP */
  } else if (pkt[0] == 'g') {
    reply = on_reg_read_all();
  } else if (pkt[0] == 'G') {
    reply = on_reg_write_all(pkt.substr(1));
  } else if (pkt[0] == 'p') {
    reply = on_reg_read(pkt.substr(1));
  } else if (pkt[0] == 'P') {
    reply = on_reg_write(pkt.substr(1));
  } else if (pkt[0] == 'm') {
    reply = on_mem_read(pkt.substr(1));
  } else if (pkt[0] == 'M') {
    reply = on_mem_write_hex(pkt.substr(1));
  } else if (pkt[0] == 'X') {
    reply = on_mem_write_bin(pkt.substr(1));
  } else if (pkt[0] == 'c') {
    reply = on_continue(pkt.substr(1));
  } else if (pkt[0] == 's') {
    reply = on_step(pkt.substr(1));
  } else if (pkt[0] == 'k') {
    detached_ = true;                    /* kill: no reply, the session is over */
    reply_expected = false;
  } else if (pkt[0] == 'D') {
    reply = "OK";                        /* detach: say so, then the session ends */
    detached_ = true;
  } else if (pkt[0] == 'H') {
    reply = "OK";                        /* thread selection: one thread, always */
  } else if (pkt[0] == 'T') {
    reply = "OK";                        /* is the thread alive: yes */
  } else if (pkt[0] == 'Q') {
    if (pkt == "QStartNoAckMode") {
      no_ack_ = true;
      reply = "OK";
    } else {
      reply = "";
    }
  } else if (pkt[0] == 'q') {
    const size_t colon = pkt.find(':');
    if (colon == std::string::npos)
      reply = on_query(pkt.substr(1), "");
    else
      reply = on_query(pkt.substr(1, colon - 1), pkt.substr(colon + 1));
  } else if (pkt.rfind("vCont", 0) == 0) {
    if (pkt == "vCont?") {
      reply = "vCont;c;s";
    } else if (pkt.size() > 6 && pkt[5] == ';' && pkt[6] == 'c') {
      reply = on_continue(pkt.substr(7));
    } else if (pkt.size() > 6 && pkt[5] == ';' && pkt[6] == 's') {
      reply = on_step(pkt.substr(7));
    } else {
      reply = "";
    }
  } else if (pkt[0] == 'Z' || pkt[0] == 'z') {
    const u32 type = pkt.size() > 1 ? (u32)(pkt[1] - '0') : 9;
    if (type <= 1)
      reply = on_breakpoint(pkt.substr(1), pkt[0] == 'Z');
    else
      reply = "";                        /* watchpoints: not supported */
  } else {
    reply = "";                          /* unknown packet: the empty reply */
  }

  if (reply_expected) send_packet(reply);
  return !detached_;
}

void GdbStub::serve() {
  while (!detached_ && serve_one()) {
  }
}

}  // namespace brain::gdb
