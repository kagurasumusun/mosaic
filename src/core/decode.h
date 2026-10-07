/* brain-emu: ARM (32-bit) instruction decoder -- ARMv5TEJ.
 * SPDX-License-Identifier: LicenseRef-brain-emu-1.0
 *
 * One decoder serves both engines: the interpreter executes what it returns
 * and the JIT translates it (or bails out to the interpreter for the few
 * classes it does not inline).  Keeping a single decoder is what makes the
 * two paths agree by construction.
 *
 * Field layout follows ARM DDI 0100I chapter A5 (ARM instruction set).
 */
#ifndef BRAINEMU_CORE_DECODE_H
#define BRAINEMU_CORE_DECODE_H

#include "core/state.h"

namespace brain {

enum ArmOp : u32 {
  AOP_UNDEF = 0,      /* not a valid ARM926EJ-S encoding                 */
  AOP_SATISFY,        /* satisfied: conditional instruction, cond false   */
  AOP_DP,             /* data processing (opcode in .dp)                  */
  AOP_MRS,
  AOP_MSR,
  AOP_MUL,            /* MUL/MLA                                          */
  AOP_MULL,           /* UMULL/SMULL/UMLAL/SMLAL                          */
  AOP_MULH,           /* SMLAxy/SMULxy                                    */
  AOP_MULHAL,         /* SMLALxy                                          */
  AOP_MULW,           /* SMLAWy/SMULWy                                    */
  AOP_QD,             /* QADD/QDADD/QSUB/QDSUB                            */
  AOP_CLZ,
  AOP_BKPT,
  AOP_SWP,
  AOP_LDRST,          /* LDR/STR/LDRB/STRB (register or immediate offset) */
  AOP_LDRSTH,         /* LDRH/STRH/LDRSB/LDRSH                            */
  AOP_LDRD,           /* LDRD/STRD                                        */
  AOP_LDM,
  AOP_STM,
  AOP_B,              /* B/BL/BLX(immediate)                              */
  AOP_BX,             /* BX/BLX(register)/BXJ                             */
  AOP_SWI,
  AOP_LDC,
  AOP_STC,
  AOP_MCR,
  AOP_MRC,
  AOP_MCR2,
  AOP_MRC2,
  AOP_CDPS,           /* CDP/LDC-style coprocessor reg-ops: unused here   */
  AOP_COUNT,
};

/* operand2 kinds for AOP_DP */
enum : u32 {
  OP2_NONE = 0,
  OP2_IMM,        /* rotated 8-bit immediate: .rot/.imm8   */
  OP2_REG,        /* plain register: .rm                   */
  OP2_REG_SHIFT,  /* register shifted by immediate: .rm .sh .amt */
  OP2_REG_RS,     /* register shifted by register: .rm .rs .sh */
};

/* AOP_LDRST / LDRSTH / LDRD addressing modes */
enum : u32 {
  AM_NONE = 0,
  AM_IMM_PRE,     /* [Rn, #imm]            */
  AM_IMM_PRE_WB,  /* [Rn, #imm]!           */
  AM_IMM_POST,    /* [Rn, #imm]            (writeback) */
  AM_REG_PRE,     /* [Rn, Rm, shift]       */
  AM_REG_PRE_WB,
  AM_REG_POST,
  AM_NONE_ADDR,   /* [Rn]                  */
};

struct ArmDec {
  u32 op;
  u32 insn;
  /* registers */
  u32 rd, rn, rm, rs, ra;
  /* flags/fields */
  u32 s;          /* S bit                                            */
  u32 dp;         /* AOP_DP: 0..15 opcode                             */
  u32 op2;        /* AOP_DP: OP2_*                                    */
  u32 rot;        /* immediate rotate field (0..15)                   */
  u32 sh;         /* shift type (SH_*)                                */
  u32 amt;        /* shift amount (immediate) or 0                    */
  u32 am;         /* load/store addressing mode (AM_*)                */
  u32 u;          /* up/down                                            */
  u32 b;          /* byte                                               */
  u32 h;          /* halfword select (LDRH/STRH) vs signed            */
  u32 signed_;    /* LDRSB/LDRSH                                        */
  u32 w;          /* writeback                                          */
  u32 l;          /* load (1) / store (0)                             */
  u32 p;          /* pre-index                                          */
  u32 imm12;      /* immediate offset / rotated for reg?              */
  u32 imm24;      /* branch offset (already scaled & sign-extended)   */
  u32 list;       /* LDM/STM register bitmask                         */
  u32 psr;        /* MRS/MSR: 0 = CPSR, 1 = SPSR                      */
  u32 mask;       /* MSR field mask (bits 19:16)                      */
  u32 cp;         /* coprocessor number                               */
  u32 crn, crm;   /* coprocessor registers                            */
  u32 opc1, opc2; /* coprocessor opcodes (opc1 = MRC 'U'/MCR 'O')     */
  u32 offset8;    /* LDC/STC word offset (already x4)                 */
  u32 kind;       /* AOP_MUL: MLA bit; AOP_MULL: U|S; AOP_QD: op      */
  u32 unaligned_ok; /* ARMv5 allows an unaligned address for this form  */
  u32 cond;         /* Thumb state: condition code of a conditional branch */
};

/* Decode `insn`.  Returns the AOP_* code and fills `d`.  AOP_UNDEF means the
 * encoding is not a defined ARM926EJ-S instruction (the caller raises the
 * Undefined Instruction exception).  AOP_SATISFY means the condition failed. */
u32 decode_arm(u32 insn, ArmDec* d);

}  // namespace brain

#endif  /* BRAINEMU_CORE_DECODE_H */
