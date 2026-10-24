#!/usr/bin/env python3
"""Provenance tool: end-to-end simulation of the sensorbus machine.

Run from the repository root with a venv that has unicorn,
keystone-engine and capstone installed (generate_firmware.py first):

    python apps/sensorbus/tools/simulate_peripherals.py

Executes tmp/sensorbus_fw.bin on unicorn ARM with Python ports of every
model the Rust app composes (DW-APB-I2C, PL022, PL061, PL011, the I2C
bit-bang decoder, GpioNet pull-up nets, the shared I2C bus with BME280-A
and TCA9548A/BME280-C, the SPI bus with BME280-B) and asserts the golden
console output plus the P2 exit-criterion probes.
"""
import struct
from unicorn import (Uc, UC_ARCH_ARM, UC_MODE_ARM, UC_HOOK_MEM_READ,
                     UC_HOOK_MEM_WRITE, UC_PROT_ALL)

I2C_BASE, GPIO_BASE, SSP_BASE, UART_BASE = 0x10002000, 0x101E4000, 0x1000D000, 0x101F1000


class Nack(Exception):
    pass


# ---------------- BME280 ----------------
CHIP_ID = 0x60


class Bme280Core:
    def __init__(self):
        self.bank = bytearray(256)
        self.bank[0xD0] = CHIP_ID
        # scene raw values
        adc_p, adc_t, adc_h = 415148, 519888, 32768
        self.bank[0xF7:0xFA] = bytes([(adc_p >> 12) & 0xFF, (adc_p >> 4) & 0xFF, (adc_p << 4) & 0xF0])
        self.bank[0xFA:0xFD] = bytes([(adc_t >> 12) & 0xFF, (adc_t >> 4) & 0xFF, (adc_t << 4) & 0xF0])
        self.bank[0xFD:0xFF] = struct.pack(">H", adc_h)

    def reset(self):
        keep = bytes(self.bank)
        self.bank = bytearray(256)
        self.bank[:] = keep  # functional: NVM/scene survive soft reset
        for a in (0xF2, 0xF4, 0xF5):
            self.bank[a] = 0

    def read_byte(self, off):
        return self.bank[off & 0xFF]

    def write_byte(self, off, val):
        off &= 0xFF
        if off == 0xE0 and val == 0xB6:
            for a in (0xF2, 0xF4, 0xF5):
                self.bank[a] = 0
        elif off in (0xF2, 0xF4, 0xF5):
            self.bank[off] = val


class Bme280I2c:
    def __init__(self, addr, core=None):
        self.addr = addr
        self.core = core or Bme280Core()
        self.pointer = 0

    def matches(self, addr):
        return addr == self.addr

    def target_write(self, addr, data):
        if not self.matches(addr):
            raise Nack
        if not data:
            return
        self.pointer = data[0] & 0xFF
        for b in data[1:]:
            self.core.write_byte(self.pointer, b)
            self.pointer = (self.pointer + 1) & 0xFF

    def target_read(self, addr, n):
        if not self.matches(addr):
            raise Nack
        out = []
        for _ in range(n):
            out.append(self.core.read_byte(self.pointer))
            self.pointer = (self.pointer + 1) & 0xFF
        return out


class Bme280Spi:
    def __init__(self, core=None):
        self.core = core or Bme280Core()
        self.pointer = 0
        self.reading = False
        self.have_cmd = False

    def selected(self, assert_):
        if not assert_:
            self.have_cmd = False
            self.reading = False
            self.pointer = 0

    def exchange_byte(self, mosi):
        if not self.have_cmd:
            self.reading = (mosi & 0x80) != 0
            self.pointer = ((mosi & 0x7F) | 0x80) & 0xFF
            self.have_cmd = True
            return 0
        if self.reading:
            v = self.core.read_byte(self.pointer)
            self.pointer = (self.pointer + 1) & 0xFF
            return v
        self.core.write_byte(self.pointer, mosi)
        self.pointer = (self.pointer + 1) & 0xFF
        return 0


class Tca9548a:
    def __init__(self, addr):
        self.addr = addr
        self.control = 0
        self.down = {}

    def matches(self, a):
        # Mirrors the Rust model: routed downstream addresses are visible.
        return a == self.addr or any(
            self.control & (1 << ch) and part.matches(a)
            for ch, part in self.down.items()
        )

    def _route(self, addr):
        for ch, part in self.down.items():
            if self.control & (1 << ch) and part.matches(addr):
                return ch
        return None

    def target_write(self, addr, data):
        if addr == self.addr:
            if data:
                self.control = data[0]
            return
        ch = self._route(addr)
        if ch is None:
            raise Nack
        self.down[ch].target_write(addr, data)

    def target_read(self, addr, n):
        if addr == self.addr:
            return [self.control] * n
        ch = self._route(addr)
        if ch is None:
            raise Nack
        return self.down[ch].target_read(addr, n)


# ---------------- I2C bus ----------------
class Bus:
    def __init__(self):
        self.targets = []
        self.log = []

    def add_target(self, t):
        self.targets.append(t)

    def _find(self, addr):
        for t in self.targets:
            if t.matches(addr):
                return t
        return None

    def write(self, addr, data):
        self.log.append(("w", addr, bytes(data)))
        t = self._find(addr)
        if t is None:
            raise Nack
        t.target_write(addr, data)

    def read(self, addr, n):
        self.log.append(("r", addr, n))
        t = self._find(addr)
        if t is None:
            raise Nack
        return t.target_read(addr, n)

    def write_read(self, addr, out, n):
        self.log.append(("wr", addr, bytes(out), n))
        t = self._find(addr)
        if t is None:
            raise Nack
        if out:
            t.target_write(addr, out)
        return t.target_read(addr, n)


# ---------------- decoder (bit-bang) ----------------
HIGH, LOW = 1, 0


class Decoder:
    def __init__(self, bus):
        self.bus = bus
        self.prev_sda = HIGH
        self.prev_scl = HIGH
        self.in_frame = False
        self.bit = 0
        self.shift = 0
        self.read = False
        self.addr = None
        self.addr_ack_pending = False
        self.write_bytes = []
        self.read_count = 0
        self.read_bits = 0
        self.read_byte = 0
        self.last_result = None

    def target_driven_slot(self):
        if not self.in_frame or self.addr is None:
            return False
        if self.addr_ack_pending:
            return True
        if self.read and self.bit < 8:
            return True
        return not self.read and self.bit == 8

    def sda_readback(self):
        if self.addr_ack_pending:
            return self.last_result == "ok"
        if self.read:
            m = 1 << (7 - min(self.read_bits, 7))
            return (self.read_byte & m) != 0
        return HIGH

    def on_sda(self, level):
        if level == LOW and self.prev_sda == HIGH and self.prev_scl == HIGH:
            self.in_frame = True
            self.bit = 0
            self.shift = 0
            self.addr = None
            self.addr_ack_pending = False
            self.write_bytes = []
            self.read_count = 0
            self.read_bits = 0
            self.read_byte = 0
        elif level == HIGH and self.prev_sda == LOW and self.prev_scl == HIGH and self.in_frame:
            self._stop()
        self.prev_sda = level

    def on_scl(self, level):
        # Rising edge only — mirrors the Rust decoder exactly.
        if level == HIGH and self.prev_scl == LOW:
            self._rising()
        self.prev_scl = level

    def _rising(self):
        if not self.in_frame:
            return
        if self.bit < 8:
            self.shift = ((self.shift << 1) | (1 if self.prev_sda else 0)) & 0xFF
            if self.read and self.addr is not None:
                self.read_bits += 1
            self.bit += 1
            if self.bit == 8 and self.addr is None:
                self._decode_address()
        else:
            self._complete_slot()

    def _decode_address(self):
        a = self.shift >> 1
        self.read = (self.shift & 1) == 1
        self.write_bytes = []
        self.read_count = 0
        self.read_bits = 0
        self.read_byte = 0
        if a <= 0x07 or a >= 0x78:
            self.addr = None
            self.last_result = "badaddr"
        elif any(t.matches(a) for t in self.bus.targets):
            self.addr = a
            self.addr_ack_pending = True
            self.last_result = "ok"
        else:
            self.addr = None
            self.last_result = "noaddr"

    def _complete_slot(self):
        # Mirrors the Rust decoder: the address-ACK slot prefetches the first
        # read byte; it never appends the address byte to write_bytes.
        ack_drive = self.prev_sda == LOW
        if self.addr_ack_pending:
            self.addr_ack_pending = False
            if self.read and self.addr is not None:
                try:
                    self.read_byte = self.bus.read(self.addr, 1)[0]
                except Nack:
                    self.read_byte = 0
                self.read_bits = 0
        elif self.read:
            if self.read_count > 0:
                self.write_bytes = []
            if ack_drive:
                self.read_count += 1
                try:
                    self.read_byte = self.bus.read(self.addr, 1)[0]
                except Nack:
                    self.read_byte = 0
                self.read_bits = 0
            else:
                self.last_result = "ok"
        elif self.addr is not None:
            self.write_bytes.append(self.shift)
        self.bit = 0
        self.shift = 0

    def _stop(self):
        if self.addr is None:
            self.in_frame = False
            self.addr_ack_pending = False
            return
        if self.write_bytes:
            try:
                self.bus.write(self.addr, self.write_bytes)
                self.last_result = "ok"
            except Nack:
                self.last_result = "nack"
        self.in_frame = False
        self.addr = None
        self.addr_ack_pending = False
        self.write_bytes = []
        self.read_count = 0
        self.read_bits = 0
        self.read_byte = 0


# ---------------- gpio net / ports ----------------
class Net:
    def __init__(self):
        self.drive = None  # None = released (pull-up wins)
        self.sinks = []

    def level(self):
        return LOW if self.drive == LOW else HIGH

    def set_drive(self, d):
        prev = self.level()
        self.drive = d
        now = self.level()
        if prev != now:
            for s in self.sinks:
                s(now)


class LineSink:
    def __init__(self, dec, which):
        self.dec, self.which = dec, which

    def __call__(self, level):
        if self.which == "sda":
            self.dec.on_sda(level)
        else:
            self.dec.on_scl(level)


class BitBangPort:
    def __init__(self, net, dec, is_sda):
        self.net, self.dec, self.is_sda = net, dec, is_sda

    def drive(self, level):
        if self.is_sda:
            self.dec.on_sda(level)
        else:
            self.dec.on_scl(level)
        self.net.set_drive(level)

    def current_level(self):
        if self.is_sda and self.dec.target_driven_slot():
            return HIGH if self.dec.sda_readback() else LOW
        return self.net.level()


# ---------------- PL061 ----------------
class Pl061:
    def __init__(self, pins):
        self.pins = pins  # {index: BitBangPort}
        self.dir = 0
        self.latch = 0
        self.afsel = 0

    def read(self, off):
        if off < 0x400:
            mask = (off >> 2) & 0xFF
            val = 0
            for p in range(8):
                bit = 1 << p
                if not mask & bit or self.afsel & bit:
                    continue
                port = self.pins.get(p)
                if port and port.current_level() == HIGH:
                    val |= bit
            return val
        return {0x400: self.dir, 0x420: self.afsel}.get(off, 0)

    def write(self, off, val):
        if off < 0x400:
            mask = (off >> 2) & 0xFF
            byte = val & 0xFF
            self.latch = (self.latch & ~mask) | (byte & mask)
            for p in range(8):
                bit = 1 << p
                if not mask & bit or self.afsel & bit:
                    continue
                port = self.pins.get(p)
                if port is None or not self.dir & bit:
                    continue
                port.drive(HIGH if self.latch & bit else LOW)
            return
        if off == 0x400:
            self.dir = val & 0xFF
        elif off == 0x420:
            self.afsel = val & 0xFF


# ---------------- DW APB I2C ----------------
class Dw:
    def __init__(self, bus):
        self.bus = bus
        self.con = 0x65
        self.tar = 0x55
        self.enable = 0
        self.pending = []
        self.rx = []
        self.raw = 1 << 4  # TX_EMPTY

    def read(self, off):
        if off == 0x010:
            if not self.rx:
                self.raw |= 1  # RX_UNDER
                return 0
            b = self.rx.pop(0)
            self._refresh()
            return b
        if off == 0x070:
            v = 0x06  # TFNF | TFE
            if self.pending:
                v |= 1 | (1 << 5)
            if self.rx:
                v |= 1 << 3
                if len(self.rx) >= 16:
                    v |= 1 << 4
            return v
        if off == 0x078:
            return len(self.rx)
        if off in (0x000, 0x004, 0x06C):
            return {0x000: self.con, 0x004: self.tar, 0x06C: self.enable & 1}[off]
        return 0

    def write(self, off, val):
        if off == 0x000:
            self.con = val & 0xFFFF
        elif off == 0x004:
            self.tar = val & 0x3FF
        elif off == 0x010:
            if self.enable & 1:
                self.pending.append((val & 0xFF, val & 0x100 != 0,
                                     val & 0x200 != 0, val & 0x400 != 0))
                if val & 0x200:
                    self._execute()
        elif off == 0x06C:
            self.enable = val & 1
            if not self.enable:
                self.pending = []
                self.rx = []
                self._refresh()

    def _refresh(self):
        self.raw &= ~((1 << 2) | (1 << 4))
        if len(self.rx) > 0:
            self.raw |= 1 << 2  # RX_FULL (tl=0)
        if self.enable & 1:
            self.raw |= 1 << 4

    def _execute(self):
        q = self.pending
        self.pending = []
        if not q:
            return
        if not self.con & 1:
            return
        addr = self.tar & 0x7F
        if addr <= 0x07 or addr >= 0x78:
            return
        writes, reads, restarts, malformed = [], 0, False, False
        seen_read = False
        for dat, is_read, stop, restart in q:
            if is_read:
                reads += 1
                seen_read = True
                if restart:
                    restarts = True
            else:
                if seen_read:
                    malformed = True
                writes.append(dat)
        if malformed or (restarts and not self.con & (1 << 5)):
            return
        try:
            if writes and reads == 0:
                self.bus.write(addr, writes)
            elif writes and restarts:
                self.rx.extend(self.bus.write_read(addr, writes, reads))
            elif writes:
                self.bus.write(addr, writes)
                self.rx.extend(self.bus.read(addr, reads))
            else:
                self.rx.extend(self.bus.read(addr, reads))
        except Nack:
            return
        self._refresh()


# ---------------- PL022 + SPI bus ----------------
class SpiBus:
    def __init__(self):
        self.targets = {}

    def add_target(self, cs, t):
        self.targets[cs] = t

    def select(self, cs, assert_):
        t = self.targets.get(cs)
        if t:
            t.selected(assert_)

    def exchange(self, cs, mosi):
        t = self.targets.get(cs)
        if t is None:
            raise Nack
        return t.exchange_byte(mosi)


class Pl022:
    def __init__(self, spi):
        self.spi = spi
        self.cr0 = 0
        self.cr1 = 0
        self.rx = []

    def read(self, off):
        if off == 0x008:
            return self.rx.pop(0) if self.rx else 0
        if off == 0x00C:
            v = 0x3
            if self.rx:
                v |= 1 << 2
            if len(self.rx) >= 8:
                v |= 1 << 3
            return v
        if off == 0x000:
            return self.cr0
        if off == 0x004:
            return self.cr1
        return 0

    def write(self, off, val):
        if off == 0x000:
            self.cr0 = val & 0xFFFF
        elif off == 0x004:
            was = self.cr1 & 2
            self.cr1 = val & 0xF
            now = self.cr1 & 2
            if bool(was) != bool(now) and not self.cr1 & 4:
                self.spi.select(0, bool(now))
        elif off == 0x008:
            if self.cr1 & 2 and not self.cr1 & 4:
                bits = (self.cr0 & 0xF) + 1
                mask = 0xFF if bits >= 8 else (1 << bits) - 1
                miso = self.spi.exchange(0, val & mask & 0xFF)
                if len(self.rx) < 8:
                    self.rx.append(miso & mask)


# ---------------- PL011 ----------------
class Pl011:
    def __init__(self, out):
        self.out = out
        self.cr = 0

    def read(self, off):
        if off == 0x018:
            return 1 << 4  # RXFE only
        if off == 0x030:
            return self.cr
        return 0

    def write(self, off, val):
        if off == 0x030:
            self.cr = val
        elif off == 0x000:
            if self.cr & 1 and self.cr & 0x100:
                self.out.append(val & 0xFF)


# ---------------- machine ----------------
bus = Bus()
bme_a = Bme280I2c(0x76)
mux = Tca9548a(0x70)
mux.down[3] = Bme280I2c(0x77)
bus.add_target(bme_a)
bus.add_target(mux)
dec = Decoder(bus)

sda_net, scl_net = Net(), Net()
sda_net.sinks.append(LineSink(dec, "sda"))
scl_net.sinks.append(LineSink(dec, "scl"))
sda_port = BitBangPort(sda_net, dec, True)
scl_port = BitBangPort(scl_net, dec, False)
gpio = Pl061({0: sda_port, 1: scl_port})

dw = Dw(bus)
spi = SpiBus()
spi.add_target(0, Bme280Spi())
ssp = Pl022(spi)
out = bytearray()
uart = Pl011(out)

models = {
    I2C_BASE: dw,
    GPIO_BASE: gpio,
    SSP_BASE: ssp,
    UART_BASE: uart,
}

image = open("tmp/sensorbus_fw.bin", "rb").read()
uc = Uc(UC_ARCH_ARM, UC_MODE_ARM)
uc.mem_map(0, 1024 * 1024, UC_PROT_ALL)
uc.mem_write(0, image)
for base in models:
    uc.mem_map(base, 0x1000, UC_PROT_ALL)


def hook_read(u, access, address, size, value, user_data):
    base = address & ~0xFFF
    model = models[base]
    word = model.read(address - base)
    u.mem_write(address & ~3, struct.pack("<I", word))


def hook_write(u, access, address, size, value, user_data):
    base = address & ~0xFFF
    models[base].write(address - base, value)


for base in models:
    uc.hook_add(UC_HOOK_MEM_READ, hook_read, begin=base, end=base + 0xFFF)
    uc.hook_add(UC_HOOK_MEM_WRITE, hook_write, begin=base, end=base + 0xFFF)

from unicorn.arm_const import UC_ARM_REG_SP
uc.reg_write(UC_ARM_REG_SP, 0x00100000)

# stop when the firmware reaches its final `b .` spin (encoding 0xEAFFFFFE)
words = struct.unpack("<%dI" % (len(image) // 4), image)
spin_addr = words.index(0xEAFFFFFE) * 4
uc.emu_start(0, spin_addr, count=4096)

print("output:", bytes(out).decode(errors="replace"))
assert bytes(out) == b"ctl=60 bb=60 spi=60\n", f"MISMATCH: {bytes(out)!r}"

# ---- harness-level probes through the shared bus ----
# bit-bang frame A must have mutated the SAME BME280-A the controller read.
assert bus.write_read(0x76, [0xF4], 1) == [0x25], "ctrl_meas not shared!"
# mux: hidden until channel 3 is enabled
try:
    bus.read(0x77, 1)
    raise AssertionError("0x77 visible before mux enable")
except Nack:
    pass
bus.write(0x70, [0x08])
assert bus.write_read(0x77, [0xD0], 1) == [CHIP_ID], "mux routing broken"
assert dec.last_result == "ok"
assert dw.rx == [] and dw.pending == []
print("decoder last_result:", dec.last_result)
print("bus ops logged:", len(bus.log))
print("ALL SENSORBUS CHECKS PASS")
