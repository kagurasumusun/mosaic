#!/usr/bin/env python3
"""Provenance tool: regenerate apps/sensorbus firmware words.

Run from the repository root with a venv that has keystone-engine and
capstone installed:

    python apps/sensorbus/tools/generate_firmware.py > tmp/fw_out.txt

Prints the verified capstone listing plus the `FIRMWARE_WORDS` Rust block
that must match apps/sensorbus/src/sensorbus.rs, and writes tmp/fw_asm.txt
(the apps/sensorbus/firmware.S listing) and tmp/sensorbus_fw.bin (the image
consumed by tools/simulate_peripherals.py).
"""
from keystone import Ks, KS_ARCH_ARM, KS_MODE_ARM
from capstone import Cs, CS_ARCH_ARM, CS_MODE_ARM

I2C_BASE = 0x10002000
GPIO_BASE = 0x101E4000
SSP_BASE = 0x1000D000
UART_BASE = 0x101F1000


class Emit:
    def __init__(self):
        self.lines = []
        self.pool_ldrs = []  # (word_index, pool_slot)
        self.n = 0  # instruction (word) count so far; labels take no word

    def raw(self, *ls):
        for line in ls:
            self.lines.append(line)
            if not line.strip().endswith(":"):
                self.n += 1

    def pool(self, slot):
        # record word index of this ldr for later patching
        self.pool_ldrs.append((self.n, slot))
        self.lines.append("ldr r2, [pc, #0]")
        self.n += 1


def sda(e, v, state):
    if state != v:
        e.raw(f"mov r0, #{v}", "str r0, [r2, #4]")
        return v
    return state


def pulse(e):
    e.raw("mov r0, #2", "str r0, [r2, #8]", "mov r0, #0", "str r0, [r2, #8]")


def byte_out(e, val, state):
    for i in range(7, -1, -1):
        state = sda(e, (val >> i) & 1, state)
        pulse(e)
    return state


def ack(e, state, v=1):
    state = sda(e, v, state)
    pulse(e)
    return state


def start(e, state):
    state = sda(e, 1, state)
    e.raw("mov r0, #2", "str r0, [r2, #8]")  # SCL High
    e.raw("mov r0, #0", "str r0, [r2, #0xC]")  # SDA+SCL Low -> START
    return 0


def stop(e, state):
    state = sda(e, 0, state)  # SDA Low while SCL Low
    e.raw("mov r0, #2", "str r0, [r2, #8]")  # SCL High
    e.raw("mov r0, #3", "str r0, [r2, #0xC]")  # SDA High while SCL High -> STOP
    return 1


def read_byte(e, state):
    state = sda(e, 1, state)  # release SDA
    for _ in range(8):
        # Sample while SCL is low (decoder shifts the target byte on the
        # rising edge; sda_readback reflects the CURRENT bit before it).
        e.raw("mov r5, r5, lsl #1", "ldr r0, [r2, #0x3FC]",
              "and r0, r0, #1", "orr r5, r5, r0")
        pulse(e)
    return state


e = Emit()

# ---------- pass 1: DW-APB-I2C controller path ----------
e.pool(0)  # I2C base -> r2
e.raw("mov r0, #1", "str r0, [r2, #0x6C]")       # IC_ENABLE = 1
e.raw("mov r0, #0x76", "str r0, [r2, #0x04]")    # IC_TAR = 0x76
e.raw("mov r0, #0xD0", "str r0, [r2, #0x10]")    # DATA_CMD: ptr write
e.raw("mov r0, #0x700", "str r0, [r2, #0x10]")   # DATA_CMD: read|restart|stop
e.raw("poll1:")
e.raw("ldr r0, [r2, #0x70]", "tst r0, #8", "beq poll1")  # wait RFNE
e.raw("ldr r4, [r2, #0x10]")                     # r4 = chip id
e.raw("mov r0, #0", "str r0, [r2, #0x6C]")       # disable

# ---------- pass 2: PL061 bit-bang path ----------
e.pool(1)  # GPIO base -> r2
e.raw("mov r0, #0xFF", "str r0, [r2, #0x400]")   # GPIODIR = outputs
st = 1  # nets idle High via pull-ups

# frame A: write ctrl_meas(0xF4) = 0x25 -> shared-state mutation proof
st = start(e, st)
st = byte_out(e, 0xEC, st); st = ack(e, st)      # addr 0x76 write
st = byte_out(e, 0xF4, st); st = ack(e, st)
st = byte_out(e, 0x25, st); st = ack(e, st)
st = stop(e, st)

# frame B: set pointer to chip id register
st = start(e, st)
st = byte_out(e, 0xEC, st); st = ack(e, st)
st = byte_out(e, 0xD0, st); st = ack(e, st)
st = stop(e, st)

# frame C: read chip id with NACK + STOP
st = start(e, st)
st = byte_out(e, 0xED, st); st = ack(e, st)
e.raw("mov r5, #0")
st = read_byte(e, st)
st = ack(e, st, 1)                               # NACK (SDA already high)
st = stop(e, st)
e.raw("mov r7, r5")                              # preserve bit-bang id

# ---------- pass 3: PL022 SPI path ----------
e.pool(2)  # SSP base -> r2
e.raw("mov r0, #0x07", "str r0, [r2, #0x00]")    # CR0: 8-bit Motorola
e.raw("mov r0, #0x02", "str r0, [r2, #0x04]")    # CR1: SSE -> CS assert
e.raw("mov r0, #0xD0", "str r0, [r2, #0x08]")    # DR: read-id command
e.raw("mov r0, #0", "str r0, [r2, #0x08]")       # DR: dummy -> rx=[00,id]
e.raw("ldr r0, [r2, #0x08]")                     # pop dummy
e.raw("ldr r6, [r2, #0x08]")                     # r6 = chip id
e.raw("mov r0, #0", "str r0, [r2, #0x04]")       # CS release

# ---------- print "ctl=XX bb=XX spi=XX\n" ----------
e.pool(3)  # UART base -> r2
e.raw("mov r0, #1", "orr r0, r0, #0x100", "str r0, [r2, #0x30]")  # CR
for ch in "ctl=":
    e.raw(f"mov r0, #{ord(ch)}", "bl putc")
e.raw("mov r5, r4", "bl putbyte")
for ch in " bb=":
    e.raw(f"mov r0, #{ord(ch)}", "bl putc")
e.raw("mov r5, r7", "bl putbyte")
for ch in " spi=":
    e.raw(f"mov r0, #{ord(ch)}", "bl putc")
e.raw("mov r5, r6", "bl putbyte")
e.raw(f"mov r0, #{ord(chr(10))}", "bl putc")
e.raw("b spin", "spin:", "b spin")
# putc: leaf. putnib tail-calls putc so LR survives. putbyte nests two BLs
# and therefore saves/restores LR with push {lr} / pop {pc} (SP is set by the
# harness; the RAM region backs the stack).
e.raw("putc:", "str r0, [r2, #0]", "bx lr")
e.raw("putbyte:", "push {lr}", "mov r0, r5, lsr #4", "bl putnib",
      "and r0, r5, #0xF", "bl putnib", "pop {pc}")
e.raw("putnib:", "cmp r0, #10", "addlt r0, r0, #0x30",
      "addge r0, r0, #0x57", "b putc")

# ---------- assemble ----------
src = "\n".join(e.lines)
ks = Ks(KS_ARCH_ARM, KS_MODE_ARM)
body, count = ks.asm(src, addr=0)
n_words = len(body) // 4
assert n_words == e.n, (n_words, e.n)
import struct
words = list(struct.unpack("<%dI" % n_words, bytes(body)))

pool = [I2C_BASE, GPIO_BASE, SSP_BASE, UART_BASE]
for ldr_index, slot in e.pool_ldrs:
    target = (n_words + slot) * 4
    pc = (ldr_index + 2) * 4
    off = target - pc
    assert 0 <= off < 4096, (ldr_index, slot, off)
    assert words[ldr_index] == 0xE59F2000, hex(words[ldr_index])
    words[ldr_index] |= off

image = struct.pack("<%dI" % n_words, *words) + struct.pack("<4I", *pool)
total_words = n_words + 4

# ---------- verify: disassemble ----------
cs = Cs(CS_ARCH_ARM, CS_MODE_ARM)
dis = list(cs.disasm(image, 0))
assert len(dis) == total_words, (len(dis), total_words)
print(f"; {total_words} words ({len(image)} bytes)")
for ins in dis:
    print(f"0x{ins.address:04x}: {ins.mnemonic} {ins.op_str}")

# ---------- verify: pool LDR targets ----------
for ldr_index, slot in e.pool_ldrs:
    ins = dis[ldr_index]
    assert ins.mnemonic == "ldr", ins
    # capstone renders: ldr r2, [pc, #off] ; resolve to pool word
    off = int(ins.op_str.split("#")[1].rstrip("]"), 0)
    target = (ldr_index + 2) * 4 + off
    value = struct.unpack("<I", image[target:target + 4])[0]
    assert value == pool[slot], (hex(value), hex(pool[slot]))
    print(f"; pool LDR @word {ldr_index} -> 0x{value:08X} OK")

# keystone round-trip of the body must match
body2, _ = ks.asm(src, addr=0)
assert bytes(body2) == bytes(body)

with open("tmp/fw_asm.txt", "w") as asm_file:
    asm_file.write("\n".join(e.lines) + "\n")

print("\npub static FIRMWARE: &[u32] = &[")
line = "   "
for i, w in enumerate(words + pool):
    line += f" 0x{w:08X},"
    if (i + 1) % 8 == 0:
        print(line)
        line = "   "
if line.strip():
    print(line)
print("];")

with open("tmp/sensorbus_fw.bin", "wb") as f:
    f.write(image)
print("wrote tmp/sensorbus_fw.bin")
