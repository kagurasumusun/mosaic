/* x86-64 machine code: the encoder the JIT backend appends to.
 * SPDX-License-Identifier: LicenseRef-brain-emu-1.0
 *
 * This is a *layer*, not a code generator: it knows how a ModRM/SIB byte and a
 * REX prefix are put together and nothing about ARM or about the IR.  Keeping it
 * separate is what lets tests/test_jit_x64.cpp check every emitted form against
 * the byte sequences an assembler produces (see the `--forms` mode of
 * tools/jit_emit), independently of the translation that uses it.
 *
 * Only 32-bit operand size is used for the guest's data path: the guest is a
 * 32-bit machine, so `mov eax, ebx` is the right width and the upper halves of
 * the host registers are never interesting.  Addresses (the shadow pointer, the
 * helper arguments) are 64-bit where they have to be.
 *
 * Conventions used by the emitters here:
 *   - every `*_rr` operand is a 32-bit register number 0..15 (see Reg below);
 *   - `rm` forms take a base register plus a signed displacement, and `sib`
 *     forms take base + index*scale + displacement (base = -1 for none);
 *   - a jump is emitted with a placeholder displacement and patched afterwards,
 *     which is how the emitters build forward branches without a second pass.
 *
 * Encodings: Intel SDM vol. 2 (the ModRM/SIB tables in 2.1.3-2.1.5 and the
 * one-byte/two-byte opcode maps), cross-checked against what `objdump -d` prints
 * for these bytes in tests/test_jit_x64.cpp.
 */
#ifndef BRAIN_JIT_X64_ASM_H
#define BRAIN_JIT_X64_ASM_H

#include <cstddef>
#include <cstdint>
#include <vector>

namespace brain::arm926 {
namespace x64 {

enum : int {
  RAX = 0, RCX = 1, RDX = 2, RBX = 3, RSP = 4, RBP = 5, RSI = 6, RDI = 7,
  R8 = 8, R9 = 9, R10 = 10, R11 = 11, R12 = 12, R13 = 13, R14 = 14, R15 = 15,
  NOREG = -1,
};

/* Condition codes, in the encoding order of the Jcc/CMOVcc/SETcc opcodes. */
enum : int {
  C_O = 0, C_NO, C_B, C_AE, C_E, C_NE, C_BE, C_A,
  C_S, C_NS, C_P, C_NP, C_L, C_GE, C_LE, C_G,
};

/* ALU operation selectors: the /n field of the 0x80-0x83 and 0xF6/0xF7 groups,
 * which also happens to be the opcode delta of the 8-bit reg/rm forms
 * (00/08/10/18/20/28/30/38) and of the 16/32-bit ones (01/09/11/19/21/29/31/39). */
enum : int {
  A_ADD = 0, A_OR = 1, A_ADC = 2, A_SBB = 3,
  A_AND = 4, A_SUB = 5, A_XOR = 6, A_CMP = 7,
};

/* Shift/rotate selectors (/n of the C0/C1/D0/D1/D2/D3 groups). */
enum : int {
  S_ROL = 0, S_ROR = 1, S_RCL = 2, S_RCR = 3,
  S_SHL = 4, S_SHR = 5, S_SAR = 7,
};

/* A memory operand: [base + index*scale + disp].  `base` or `index` may be
 * NOREG; a base of RSP/R12 or any index forces a SIB byte, and a base of
 * RBP/R13 with a zero displacement still needs one because `mod = 00, rm = 101`
 * means "disp32 only". */
struct Mem {
  int base = NOREG;
  int index = NOREG;
  int scale = 1;
  int32_t disp = 0;

  static Mem bx(int b, int32_t d = 0) { return Mem{b, NOREG, 1, d}; }
  static Mem bxi(int b, int i, int sc = 1, int32_t d = 0) { return Mem{b, i, sc, d}; }
  static Mem x(int i, int sc = 1, int32_t d = 0) { return Mem{NOREG, i, sc, d}; }
};

class Buf {
 public:
  std::vector<uint8_t> w;

  /* ---- raw emission ---------------------------------------------------- */
  void u8(uint32_t v) { w.push_back((uint8_t)v); }
  void u16(uint32_t v) { u8(v); u8(v >> 8); }
  void u32(uint32_t v) { u16(v); u16(v >> 16); }
  void u64(uint64_t v) { u32((uint32_t)v); u32((uint32_t)(v >> 32)); }
  size_t pos() const { return w.size(); }

  /* REX: W (64-bit), R (ModRM.reg extension), X (SIB.index), B (ModRM.rm/SIB.base).
   * NOREG (-1) must not set a bit - `x & 8` would light up the index extension
   * for a memory operand that has no index at all. */
  void rex(int wide, int r, int x, int b, bool force = false) {
    const int rr = (r > 0) ? r : 0, xx = (x > 0) ? x : 0, bb = (b > 0) ? b : 0;
    const int v = 0x40 | ((wide & 1) << 3) | ((rr >> 3) << 2) | ((xx >> 3) << 1) |
                  (bb >> 3);
    if (v != 0x40 || force) u8(v);
  }

  /* ModRM + SIB + displacement for a memory operand whose reg field is `reg`. */
  void modrm_mem(int reg, const Mem& m) {
    const int rm_field = (m.base == NOREG) ? 5 : (m.base & 7);
    const bool need_sib = (m.index != NOREG) || rm_field == 4 || m.base == NOREG;
    int mod;
    if (m.base == NOREG) {
      mod = 0;                                  /* disp32, no base */
    } else if (m.disp == 0 && rm_field != 5) {
      mod = 0;
    } else if (m.disp >= -128 && m.disp <= 127) {
      mod = 1;
    } else {
      mod = 2;
    }
    u8((uint32_t)((mod << 6) | ((reg & 7) << 3) | (need_sib ? 4 : rm_field)));
    if (need_sib) {
      const int ss = m.scale == 8 ? 3 : m.scale == 4 ? 2 : m.scale == 2 ? 1 : 0;
      const int idx = (m.index == NOREG) ? 4 : (m.index & 7);
      const int bas = (m.base == NOREG) ? 5 : (m.base & 7);
      u8((uint32_t)((ss << 6) | (idx << 3) | bas));
    }
    if (mod == 1) u8((uint32_t)m.disp & 0xFF);
    else if (mod == 2 || m.base == NOREG) u32((uint32_t)m.disp);
  }

  /* ---- moves ------------------------------------------------------------ */
  /* mov r32, r/m32 */
  void mov_rr(int dst, int src) {
    rex(0, dst, 0, src);
    u8(0x8B);
    u8((uint32_t)(0xC0 | ((dst & 7) << 3) | (src & 7)));
  }
  /* mov r/m32, r32 */
  void mov_mr(int dst, int src) {
    rex(0, src, 0, dst);
    u8(0x89);
    u8((uint32_t)(0xC0 | ((src & 7) << 3) | (dst & 7)));
  }
  /* mov r32, imm32 */
  void mov_ri(int dst, uint32_t imm) {
    rex(0, 0, 0, dst);
    u8(0xB8u + (uint32_t)(dst & 7));
    u32(imm);
  }
  /* mov r64, imm32 (sign-extended): the helper pointers are 64-bit. */
  void mov_ri64(int dst, int32_t imm) {
    rex(1, 0, 0, dst);
    u8(0xC7);
    u8((uint32_t)(0xC0 | (dst & 7)));
    u32((uint32_t)imm);
  }
  void mov_rm(int dst, const Mem& m) {
    rex(0, dst, m.index, m.base);
    u8(0x8B);
    modrm_mem(dst, m);
  }
  void mov_mem_r(const Mem& m, int src) {
    rex(0, src, m.index, m.base);
    u8(0x89);
    modrm_mem(src, m);
  }
  /* mov r64, r/m64 (the shadow pointer lives in a 64-bit register) */
  void mov_rr64(int dst, int src) {
    rex(1, dst, 0, src);
    u8(0x8B);
    u8((uint32_t)(0xC0 | ((dst & 7) << 3) | (src & 7)));
  }
  void mov_rm64(int dst, const Mem& m) {
    rex(1, dst, m.index, m.base);
    u8(0x8B);
    modrm_mem(dst, m);
  }
  /* mov r/m32, imm32 */
  void mov_mem_i(const Mem& m, uint32_t imm) {
    rex(0, 0, m.index, m.base);
    u8(0xC7);
    modrm_mem(0, m);
    u32(imm);
  }
  /* push qword [base + disp]: FF /6, 64-bit operand size by default. */
  void push_mem(const Mem& m) {
    rex(0, 0, m.index, m.base);
    u8(0xFF);
    modrm_mem(6, m);
  }
  /* mov r32, r/m8 with zero-extension */
  void movzx_u8(int dst, int src) {
    rex(0, dst, 0, src);
    u8(0x0F);
    u8(0xB6);
    u8((uint32_t)(0xC0 | ((dst & 7) << 3) | (src & 7)));
  }
  /* movzx r32, byte [base + disp] */
  void movzx_u8_m(int dst, const Mem& m) {
    rex(0, dst, m.index, m.base);
    u8(0x0F);
    u8(0xB6);
    modrm_mem(dst, m);
  }
  /* mov r64, imm64 (only for constants; used by tests) */
  void movabs_r64(int dst, uint64_t imm) {
    rex(1, 0, 0, dst);
    u8(0xB8u + (uint32_t)(dst & 7));
    u64(imm);
  }
  /* mov r32, [base + disp] / [base + index*scale + disp] with a 64-bit address
   * register: the address size is 64-bit by default, so a 32-bit destination is
   * still addressed from the full 64-bit base. */
  void lea(int dst, const Mem& m) {
    rex(0, dst, m.index, m.base);
    u8(0x8D);
    modrm_mem(dst, m);
  }

  /* ---- ALU -------------------------------------------------------------- */
  void alu_rr(int op, int dst, int src) {
    rex(0, src, 0, dst);
    u8((uint32_t)(0x01 + (op << 3)));
    u8((uint32_t)(0xC0 | ((src & 7) << 3) | (dst & 7)));
  }
  void alu_rm(int op, int dst, const Mem& m) {
    rex(0, dst, m.index, m.base);
    u8((uint32_t)(0x03 + (op << 3)));
    modrm_mem(dst, m);
  }
  /* r/m32, imm32 - and the sign-extended imm8 form when it fits, which is what
   * an assembler emits too. */
  void alu_ri(int op, int dst, uint32_t imm) {
    if ((int32_t)imm >= -128 && (int32_t)imm <= 127) {
      rex(0, 0, 0, dst);
      u8(0x83);
      u8((uint32_t)(0xC0 | (op << 3) | (dst & 7)));
      u8(imm & 0xFF);
      return;
    }
    rex(0, 0, 0, dst);
    u8(0x81);
    u8((uint32_t)(0xC0 | (op << 3) | (dst & 7)));
    u32(imm);
  }
  /* The imm32-only form, for the cases where a sign-extended byte would be
   * wrong (none today, but the distinction is worth keeping explicit). */
  void alu_ri32(int op, int dst, uint32_t imm) {
    rex(0, 0, 0, dst);
    u8(0x81);
    u8((uint32_t)(0xC0 | (op << 3) | (dst & 7)));
    u32(imm);
  }
  void neg(int dst) { unary_group(3, dst); }
  void not_(int dst) { unary_group(2, dst); }
  void unary_group(int sub, int dst) {
    rex(0, 0, 0, dst);
    u8(0xF7);
    u8((uint32_t)(0xC0 | (sub << 3) | (dst & 7)));
  }
  /* imul r32, r/m32, imm32 */
  void imul_rri(int dst, int src, int32_t imm) {
    rex(0, dst, 0, src);
    u8(0x69);
    u8((uint32_t)(0xC0 | ((dst & 7) << 3) | (src & 7)));
    u32((uint32_t)imm);
  }

  /* test r/m32, r32 */
  void test_rr(int a, int b) {
    rex(0, b, 0, a);
    u8(0x85);
    u8((uint32_t)(0xC0 | ((b & 7) << 3) | (a & 7)));
  }
  void test_ri(int a, uint32_t imm) {
    rex(0, 0, 0, a);
    u8(0xF7);
    u8((uint32_t)(0xC0 | (a & 7)));
    u32(imm);
  }
  void test_mi(const Mem& m, uint32_t imm) {
    rex(0, 0, m.index, m.base);
    u8(0xF7);
    modrm_mem(0, m);
    u32(imm);
  }

  /* ---- shifts ----------------------------------------------------------- */
  void shift_ri(int op, int dst, uint32_t amt) {
    if (amt == 1) {                        /* D1 /n: the short form, 2 bytes */
      rex(0, 0, 0, dst);
      u8(0xD1);
      u8((uint32_t)(0xC0 | (op << 3) | (dst & 7)));
      return;
    }
    rex(0, 0, 0, dst);
    u8(0xC1);
    u8((uint32_t)(0xC0 | (op << 3) | (dst & 7)));
    u8(amt & 0xFF);
  }
  void shift_cl(int op, int dst) {
    rex(0, 0, 0, dst);
    u8(0xD3);
    u8((uint32_t)(0xC0 | (op << 3) | (dst & 7)));
  }
  void shift1(int op, int dst) {
    rex(0, 0, 0, dst);
    u8(0xD1);
    u8((uint32_t)(0xC0 | (op << 3) | (dst & 7)));
  }

  /* ---- flags, bit tests, byte sets ------------------------------------- */
  /* setcc r/m8.  The low byte of RSP/RBP/RSI/RDI is only reachable with a REX
   * prefix, and R8-R15 need one to be named at all: emission forces it. */
  void setcc(int cc, int dst) {
    rex(0, 0, 0, dst, /*force=*/true);
    u8(0x0F);
    u8((uint32_t)(0x90 + cc));
    u8((uint32_t)(0xC0 | (dst & 7)));
  }
  void bt_ri(int reg, uint32_t bit) {
    rex(0, 4, 0, reg);
    u8(0x0F);
    u8(0xBA);
    u8((uint32_t)(0xC0 | (4 << 3) | (reg & 7)));
    u8(bit & 31);
  }
  void bt_rr(int a, int b) {           /* CF = bit b of a */
    rex(0, b, 0, a);
    u8(0x0F);
    u8(0xA3);
    u8((uint32_t)(0xC0 | ((b & 7) << 3) | (a & 7)));
  }
  /* stc / clc / cmc: CF only, SF/ZF/OF untouched. */
  void stc() { u8(0xF9); }                    /* CF = 1 */
  void clc() { u8(0xF8); }                    /* CF = 0 */
  /* Complement CF.  x86 subtracts with CF as a *borrow*, ARM sets C as the
   * inverse of one - the one place the two flag models differ on the carry of a
   * subtraction, and the reason `subs` needs an instruction ARM's own `SUBS`
   * does not.  Touches no other flag. */
  void cmc() { u8(0xF5); }
  void pushfq() { u8(0x9C); }
  void popfq() { u8(0x9D); }
  /* push/pop of a 64-bit register: the operand size is 64-bit by default in
   * long mode, so only REX.B is needed for R8-R15. */
  void push64(int reg) {
    if (reg & 8) u8(0x41);
    u8(0x50u + (uint32_t)(reg & 7));
  }
  void pop64(int reg) {
    if (reg & 8) u8(0x41);
    u8(0x58u + (uint32_t)(reg & 7));
  }
  void sub_rsp(int32_t n) { alu_ri(A_SUB, RSP, (uint32_t)n); }
  void add_rsp(int32_t n) { alu_ri(A_ADD, RSP, (uint32_t)n); }

  /* ---- control ---------------------------------------------------------- */
  void ret() { u8(0xC3); }
  void nop() { u8(0x90); }
  void int3() { u8(0xCC); }
  void ud2() { u8(0x0F); u8(0x0B); }

  /* Jcc/JMP/CALL with a 32-bit displacement; the displacement is patched by
   * patch_rel32() once the target address is known. */
  size_t jcc32(int cc) {
    u8(0x0F);
    u8((uint32_t)(0x80 + cc));
    const size_t at = pos();
    u32(0);
    return at;
  }
  size_t jmp32() {
    u8(0xE9);
    const size_t at = pos();
    u32(0);
    return at;
  }
  size_t call32() {
    u8(0xE8);
    const size_t at = pos();
    u32(0);
    return at;
  }
  void patch_rel32(size_t at, size_t target) {
    const int64_t rel = (int64_t)target - (int64_t)(at + 4);
    w[at + 0] = (uint8_t)rel;
    w[at + 1] = (uint8_t)(rel >> 8);
    w[at + 2] = (uint8_t)(rel >> 16);
    w[at + 3] = (uint8_t)(rel >> 24);
  }
  void call_abs(void* fn) {
    /* movabs rax, imm64 ; call rax  -- the code arena is not guaranteed to be
     * within a 32-bit displacement of the helper. */
    movabs_r64(RAX, (uint64_t)(uintptr_t)fn);
    rex(0, 2, 0, 0);
    u8(0xFF);
    u8(0xD0);
  }
};

}  // namespace x64
}  // namespace brain::arm926

#endif  /* BRAIN_JIT_X64_ASM_H */
