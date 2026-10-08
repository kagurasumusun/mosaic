/* Instruction formatting for the trace/log output.
 * SPDX-License-Identifier: LicenseRef-brain-emu-1.0
 *
 * Both formatters drive the *real* decoders (decode_arm.cpp / decode_thumb.cpp)
 * and print from the ArmDec record.  That keeps the trace honest -- if the
 * emulator executes an instruction, the trace names the same instruction -- and
 * avoids a second, drifting hand-written disassembler.  Anything the decoder
 * rejects prints as `.word`/`.short`, which is also the most useful thing to
 * see in a trace.
 */
#include <cstdarg>
#include <cstdio>
#include <cstring>

#include "arch/arm/cpu/classic/arm9/arm926ej-s/cpu/decode.h"
#include "arch/arm/cpu/classic/arm9/arm926ej-s/cpu/decode_thumb.h"
#include "arch/arm/cpu/classic/arm9/arm926ej-s/cpu/ops.h"

namespace brain::arm926 {
namespace {

struct Sink {
  char* p;
  char* end;

  void put(const char* s) {
    while (*s && p + 1 < end) *p++ = *s++;
    *p = 0;
  }
  void fmt(const char* f, ...) {
    char b[48];
    va_list ap;
    va_start(ap, f);
    std::vsnprintf(b, sizeof(b), f, ap);
    va_end(ap);
    put(b);
  }
};

const char* dp_mnemonic(u32 dp) {
  switch (dp) {
    case DP_AND: return "AND";
    case DP_EOR: return "EOR";
    case DP_SUB: return "SUB";
    case DP_RSB: return "RSB";
    case DP_ADD: return "ADD";
    case DP_ADC: return "ADC";
    case DP_SBC: return "SBC";
    case DP_RSC: return "RSC";
    case DP_TST: return "TST";
    case DP_TEQ: return "TEQ";
    case DP_CMP: return "CMP";
    case DP_CMN: return "CMN";
    case DP_ORR: return "ORR";
    case DP_MOV: return "MOV";
    case DP_BIC: return "BIC";
    default:     return "MVN";
  }
}

const char* shift_name(u32 sh) {
  static const char* n[4] = {"LSL", "LSR", "ASR", "ROR"};
  return n[sh & 3];
}

void reg_name(Sink& o, u32 n) {
  if (n == 13) o.put("SP");
  else if (n == 14) o.put("LR");
  else if (n == 15) o.put("PC");
  else o.fmt("R%u", n);
}

void list_name(Sink& o, u32 list) {
  o.put("{");
  bool first = true;
  u32 i = 0;
  while (i < 16) {
    if (!(list & (1u << i))) { ++i; continue; }
    u32 j = i;
    while (j + 1 < 16 && (list & (1u << (j + 1)))) ++j;
    if (!first) o.put(",");
    first = false;
    reg_name(o, i);
    if (j > i + 1) { o.put("-"); reg_name(o, j); i = j; }
    ++i;
  }
  o.put("}");
}

/* operand2 as text */
void op2_name(Sink& o, const ArmDec& d) {
  switch (d.op2) {
    case OP2_IMM:
      o.fmt("#0x%x", d.imm12 & 0xFFu);
      if (d.rot) o.fmt(", ROR #%u", d.rot * 2u);
      return;
    case OP2_REG:
      reg_name(o, d.rm);
      return;
    case OP2_REG_SHIFT:
      reg_name(o, d.rm);
      o.fmt(", %s #%u", shift_name(d.sh), d.amt);
      return;
    case OP2_REG_RS:
      reg_name(o, d.rm);
      o.fmt(", %s ", shift_name(d.sh));
      reg_name(o, d.rs);
      return;
    default:
      o.fmt("#0x%x", d.imm12);
      return;
  }
}

void mem_name(Sink& o, const ArmDec& d, bool thumb) {
  const char* m;
  if (d.op == AOP_LDRSTH)
    m = d.l ? (d.signed_ ? (d.h ? "LDRSH" : "LDRSB") : "LDRH") : "STRH";
  else if (d.op == AOP_LDRD)
    m = d.kind ? "STRD" : "LDRD";
  else
    m = d.l ? (d.b ? "LDRB" : "LDR") : (d.b ? "STRB" : "STR");
  o.put(m);
  o.put(" ");
  reg_name(o, d.rd);
  if (d.op == AOP_LDRD) { o.put(","); reg_name(o, d.rd + 1); }
  if (d.op == AOP_LDRSTH && !d.l) { o.put(","); reg_name(o, d.rd); }
  o.put(", [");
  reg_name(o, d.rn);
  if (d.p) {
    o.put(", ");
    if (d.op2 == OP2_NONE || d.op2 == OP2_IMM) {
      if (d.imm12) o.fmt("%s#0x%x", d.u ? "" : "-", d.imm12);
    } else {
      op2_name(o, d);
    }
    if (!thumb && !d.unaligned_ok && d.imm12 == 0 && d.op2 == OP2_NONE) {}
  }
  o.put("]");
  if (d.p && d.w) o.put("!");
  else if (!d.p) o.fmt(", %s#0x%x", d.u ? "" : "-", d.imm12);
}

}  // namespace

void format_arm_insn(u32 insn, char* out, size_t n) {
  if (n < 16) { if (n) out[0] = 0; return; }
  out[0] = 0;
  Sink o{out, out + n};

  const u32 cond = insn >> 28;
  ArmDec d;
  decode_arm(insn, &d);

  if (cond != 0xE && cond != 0xF) { o.put(cond_name(cond)); o.put(" "); }

  switch (d.op) {
    case AOP_DP: {
      o.put(dp_mnemonic(d.dp));
      if (d.s && !dp_flags_only(d.dp)) o.put("S");
      o.put(" ");
      if (!dp_flags_only(d.dp)) { reg_name(o, d.rd); o.put(", "); }
      reg_name(o, d.rn);
      if (d.dp != DP_MOV || d.op2 != OP2_REG || d.rm != d.rd) {
        o.put(", ");
        op2_name(o, d);
      }
      return;
    }
    case AOP_MRS: {
      o.put("MRS ");
      reg_name(o, d.rd);
      o.put(", ");
      o.put(d.psr ? "SPSR" : "CPSR");
      return;
    }
    case AOP_MSR: {
      static const char kField[4] = {'c', 'x', 's', 'f'};
      char fb[5];
      u32 fn = 0;
      for (u32 i = 0; i < 4; ++i)
        if (d.mask & (1u << i)) fb[fn++] = kField[i];
      fb[fn] = 0;
      o.put("MSR ");
      o.put(d.psr ? "SPSR_" : "CPSR_");
      o.put(fb);
      o.put(", ");
      op2_name(o, d);
      return;
    }
    case AOP_MUL: {
      o.put(d.kind ? "MLA" : "MUL");
      if (d.s) o.put("S");
      o.put(" ");
      reg_name(o, d.rd);
      o.put(", ");
      reg_name(o, d.rm);
      o.put(", ");
      reg_name(o, d.rs);
      if (d.kind) { o.put(", "); reg_name(o, d.rn); }
      return;
    }
    case AOP_MULL: {
      o.put(d.kind & 2u ? (d.kind & 1u ? "SMLAL" : "SMULL")
                        : (d.kind & 1u ? "UMLAL" : "UMULL"));
      if (d.s) o.put("S");
      o.put(" ");
      reg_name(o, d.rd);
      o.put(", ");
      reg_name(o, d.ra);
      o.put(", ");
      reg_name(o, d.rm);
      o.put(", ");
      reg_name(o, d.rs);
      return;
    }
    case AOP_MULH: case AOP_MULW: case AOP_MULHAL: {
      const char* m = d.op == AOP_MULHAL
          ? "SMLALxy"
          : (d.op == AOP_MULW ? (d.ra == 0x1F ? "SMULWy" : "SMLAWy")
                              : (d.ra == 0x1F ? "SMULxy" : "SMLAxy"));
      o.put(m);
      o.put(" ");
      reg_name(o, d.rd);
      if (d.op != AOP_MULW || d.ra != 0x1F) { o.put(", "); reg_name(o, d.ra); }
      o.put(", ");
      reg_name(o, d.rm);
      o.put(", ");
      reg_name(o, d.rs);
      o.fmt("%c%c", (d.kind & 1u) ? 'T' : 'B', (d.kind & 2u) ? 'T' : 'B');
      return;
    }
    case AOP_QD: {
      o.put(d.kind == 0 ? "QADD" : d.kind == 1 ? "QSUB" : d.kind == 2 ? "QDADD"
                                               : "QDSUB");
      o.put(" ");
      reg_name(o, d.rd);
      o.put(", ");
      reg_name(o, d.rn);
      o.put(", ");
      reg_name(o, d.rm);
      return;
    }
    case AOP_CLZ:
      o.put("CLZ ");
      reg_name(o, d.rd);
      o.put(", ");
      reg_name(o, d.rm);
      return;
    case AOP_SWP:
      o.put(d.b ? "SWPB " : "SWP ");
      reg_name(o, d.rd);
      o.put(", ");
      reg_name(o, d.rm);
      o.put(", [");
      reg_name(o, d.rn);
      o.put("]");
      return;
    case AOP_LDRST: case AOP_LDRSTH: case AOP_LDRD:
      mem_name(o, d, false);
      return;
    case AOP_LDM: case AOP_STM: {
      o.put(d.op == AOP_LDM ? "LDM" : "STM");
      o.fmt("%c%c", d.p ? 'I' : 'D', d.u ? 'A' : 'B');
      if (d.kind) o.put("S");
      o.put(" ");
      reg_name(o, d.rn);
      if (d.w) o.put("!");
      o.put(", ");
      list_name(o, d.list);
      return;
    }
    case AOP_B:
      o.put(d.kind == 1u ? "BL" : d.kind == 2u ? "BLX" : "B");
      o.fmt(" 0x%x", d.imm24);
      return;
    case AOP_BX:
      o.put(d.kind == 1u ? "BLX " : d.kind == 2u ? "BXJ " : "BX ");
      reg_name(o, d.rm);
      return;
    case AOP_SWI:
      o.fmt("SWI 0x%x", d.imm24);
      return;
    case AOP_BKPT:
      o.fmt("BKPT 0x%x", d.imm24);
      return;
    case AOP_MCR: case AOP_MRC: {
      o.put(d.op == AOP_MCR ? "MCR p15, " : "MRC p15, ");
      o.fmt("%u, ", d.opc1);
      reg_name(o, d.rd);
      o.fmt(", c%u, c%u, %u", d.crn, d.crm, d.opc2);
      return;
    }
    default:
      o.fmt(".word 0x%x", insn);
      return;
  }
}

void format_thumb_insn(u32 hw, char* out, size_t n) {
  if (n < 16) { if (n) out[0] = 0; return; }
  out[0] = 0;
  Sink o{out, out + n};

  ArmDec d;
  decode_thumb(hw, &d);
  switch (d.op) {
    case AOP_DP: {
      o.put(dp_mnemonic(d.dp));
      if (d.s) o.put("S");
      o.put(" ");
      if (!dp_flags_only(d.dp)) { reg_name(o, d.rd); o.put(", "); }
      reg_name(o, d.rn);
      if (d.op2 != OP2_NONE) { o.put(", "); op2_name(o, d); }
      return;
    }
    case AOP_MUL:
      o.put("MULS ");
      reg_name(o, d.rd);
      o.put(", ");
      reg_name(o, d.rs);
      return;
    case AOP_LDRST: case AOP_LDRSTH:
      mem_name(o, d, true);
      return;
    case AOP_LDM: case AOP_STM: {
      if (d.rn == 13) o.put(d.op == AOP_LDM ? "POP " : "PUSH ");
      else { o.put(d.op == AOP_LDM ? "LDMIA " : "STMIA "); reg_name(o, d.rn); o.put("!, "); }
      list_name(o, d.list);
      return;
    }
    case AOP_B: {
      if (d.kind == 4u) { o.fmt("BL-setup 0x%x", d.imm24); return; }
      o.put(d.kind == 5u ? "BL " : d.kind == 6u ? "BLX.ARM " : "B");
      if (d.kind == 3u) { o.put(cond_name(d.cond)); o.put(" "); }
      o.fmt("0x%x", d.imm24);
      return;
    }
    case AOP_BX:
      o.put(d.kind == 1u ? "BLX " : "BX ");
      reg_name(o, d.rm);
      return;
    case AOP_SWI:
      o.fmt("SVC 0x%x", d.imm24);
      return;
    case AOP_BKPT:
      o.fmt("BKPT 0x%x", d.imm24);
      return;
    default:
      o.fmt(".short 0x%x", hw);
      return;
  }
}

}  // namespace brain::arm926
