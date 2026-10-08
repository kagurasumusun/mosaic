# `host/io/` — byte I/O

The plumbing the host services share: a guest's output bytes need somewhere to
go (a UART's TX line, the console, a log file) and its input bytes need
somewhere to come from (stdin, a script, a socket).

| file | what it holds |
| --- | --- |
| `host_io.h`, `host_io.cpp` | `ByteSink` (`put`/`flush`) and `ByteSource` (`get`, -1 = nothing waiting, `exhausted`); `StdioSink`/`StdioSource` (stdin polled, never blocking); `BufferSink`/`BufferSource` (a `std::string`, for tests and scripts); the `HostIo` facade (out/err/in singletons); and the `SinkTx`/`SourceRx` adapters that turn a sink/source into the plain function pointers a device takes. |

Why the adapters exist: a device stays host-free. `devices/uart/` declares
`set_tx_hook(void(*)(void*, u8), void*)` and `set_rx_hook(int(*)(void*), void*)`,
and the embedding code bridges them to whatever it wants — stdio, a socket, a
test buffer — with `SinkTx::put`/`SourceRx::get`. The device never includes
this directory.
