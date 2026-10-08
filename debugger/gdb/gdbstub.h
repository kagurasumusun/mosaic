/* brain-emu -- a gdb stub for any machine the core can build.
 * SPDX-License-Identifier: LicenseRef-brain-emu-1.0
 *
 * `GdbStub` speaks the gdb remote serial protocol to one `GdbTransport`, and it
 * drives the machine through the C API (`brain_emu_t*`) - the same handle the
 * CLI and a GUI hold - so it debugs any CPU module, JIT or interpreter, without
 * knowing a single instruction.  What it can do is what the C API can do: read
 * and write registers, read and write memory, single-step, run, and stop.
 *
 * Breakpoints are the stub's own: `Z0`/`z0` add and remove addresses from a
 * vector, and `c` (continue) steps the machine instruction by instruction while
 * the vector is non-empty, checking the PC each time - a breakpoint is a stop,
 * not a filter, so this is the only honest way to honour one at an address the
 * core's run loop does not know about.  With no breakpoints, `c` runs whole
 * slices and reports when the machine stops on its own (an exit device, a
 * fault): `brain_run_insns` returning 0 is how the caller learns.
 *
 * The register layout `g`/`G`/`p`/`P` use is the core's own order (the GPRs, then
 * the status word, then the saved status word when the CPU has one), and
 * `qXfer:features:read:target.xml` describes exactly that layout, so what the
 * stub sends and what gdb expects are the same list.  No FPU is modelled.
 */
#ifndef BRAINEMU_DEBUGGER_GDB_GDBSTUB_H
#define BRAINEMU_DEBUGGER_GDB_GDBSTUB_H

#include <string>
#include <vector>

#include "brainemu/brainemu.h"
#include "brainemu/types.h"
#include "debugger/gdb/transport.h"

namespace brain::gdb {

class GdbStub {
 public:
  GdbStub(brain_emu_t* e, GdbTransport* t) : e_(e), t_(t) {}

  /* Serve until the transport closes, the debugger detaches (`D`) or kills
   * the session (`k`). */
  void serve();
  /* One packet: read it, answer it.  Returns false when the session is over
   * (transport closed, detach, kill). */
  bool serve_one();

  bool detached() const { return detached_; }

 private:
  /* ---- the wire ---------------------------------------------------------- */
  /* Read one packet into `pkt` (without the `$...#cs` frame).  Returns false
   * when the transport closes or the session ends.  `+`/`-` acknowledgements
   * and the 0x03 interrupt byte are consumed here. */
  bool read_packet(std::string* pkt);
  /* Send `data` framed, and ack it in no-ack mode's absence. */
  void send_packet(const std::string& data);
  void send_raw(const char* p, u32 n) { t_->write((const u8*)p, n); }

  /* ---- packet handlers ---------------------------------------------------- */
  std::string on_q_supported(const std::string& args);
  std::string on_query(const std::string& name, const std::string& args);
  std::string on_reg_read_all();          /* g */
  std::string on_reg_write_all(const std::string& hex);   /* G */
  std::string on_reg_read(const std::string& args);       /* p n */
  std::string on_reg_write(const std::string& args);      /* P n=v */
  std::string on_mem_read(const std::string& args);       /* m addr,len */
  std::string on_mem_write_hex(const std::string& args);  /* M addr,len:data */
  std::string on_mem_write_bin(const std::string& args);  /* X addr,len:bin */
  std::string on_breakpoint(const std::string& args, bool set);  /* Z0/z0, Z1/z1 */
  std::string on_continue(const std::string& args);       /* c, vCont;c */
  std::string on_step(const std::string& args);           /* s, vCont;s */
  std::string on_target_xml(u32 offset, u32 len);

  /* The register layout the wire uses, as core register indices: -1 marks a
   * slot that exists on the wire but not in the core (none today - no FPU is
   * modelled, so the layout is exactly the core's). */
  std::vector<int> reg_layout() const;
  std::string reg_layout_xml() const;

  bool hit_break(u32 pc) const;
  static bool parse_u32(const std::string& s, u32* out);
  static bool parse_hex(const std::string& s, u32* out);
  static std::string to_hex(u32 v);
  static std::string hex_encode(const u8* p, u32 n);
  static bool hex_decode(const std::string& s, std::vector<u8>* out);

  brain_emu_t* e_;
  GdbTransport* t_;
  bool no_ack_ = false;
  bool detached_ = false;
  std::vector<u32> breaks_;
};

}  // namespace brain::gdb

#endif  /* BRAINEMU_DEBUGGER_GDB_GDBSTUB_H */
