# Integration tests

Tests that cross CPU, SoC, devices, buses, loaders or machine composition seams.

| suite | what it crosses |
| --- | --- |
| `test_host_stack.cpp` | the host stack working together: a hand-assembled ARM926 guest printing through a console and a UART, the UART's interrupt over a wire into the CPU's IRQ input; a gdb stub and the command monitor driving a machine through the C API; the log capture and both filesystems. |
