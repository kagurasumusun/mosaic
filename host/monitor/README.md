# `host/monitor/` — the monitors

Two things a user drives a machine with, both over the C API
(`brain_emu_t*`) so they know no CPU and no device:

| file | what it holds |
| --- | --- |
| `command_monitor.{h,cpp}` | `CommandMonitor`: a debugger's console — `info`, `regs`, `reg`, `mem`/`x/Nf`, `set`, `break`/`delete`, `step`, `continue`, `devices`, `stats`, `reset`, `quit`. `execute()` runs one line and returns its output (which is what makes it testable without a terminal); `run()` is the stdio REPL on top of it, which is what the CLI's `--monitor-cmd` starts. `continue` with breakpoints steps and checks the PC after every instruction — a breakpoint is a stop, not a filter. |
| `stats_monitor.{h,cpp}` | `StatsMonitor`: a `Monitor` (the core's observer interface) that counts slices, retired instructions, the last slice and resets, and reports them as fields. Its module (id `stats`) is what the CLI's `--monitor stats` installs. |
| `monitor.h` | the `register_monitor_module()` declaration, called by `builtin/builtin.cpp`. |
| `tests/test_monitor.cpp` | the suite: every command against a machine built through the C API, and the stats monitor's fields read back through the C API. |

The `stats` monitor is the example of why the `Monitor` kind exists: a counter
a user can watch was added without a single core change, and it shows up in
`--stats` next to the machine's own counters.
