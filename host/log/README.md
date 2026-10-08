# `host/log/` — logging

The one logging facility for the whole tree. Every component reports through
it, so a run produces one consistent stream with levels instead of each
component inventing its own `fprintf(stderr)`.

| file | what it holds |
| --- | --- |
| `log.h`, `log.cpp` | `LogLevel` (`kDebug`/`kInfo`/`kWarn`/`kError`), the sink type (`LogSink`: where a formatted line goes), `log_set_sink`/`log_set_level`, the printf-style `log_msg`, and the `log_debug`/`log_info`/`log_warn`/`log_error` macros. The default sink writes `LEVEL message` to stderr; a test or a GUI replaces it, and `LogSinkScope` puts the previous sink back on destruction. |
| `tests/test_log.cpp` | the suite: capture via a sink, the level filter, formatting, the scope's restore. |

Design notes:

- A line is formatted into a fixed buffer and truncated, never overflowed, and
  nothing allocates after the line is formatted.
- The level filter drops a line *before* it is formatted, so a debug line in a
  hot path costs one comparison.
- The sink is a plain function pointer with a `user` — the same shape the C
  API's callbacks use — so a capture buffer, a file, or a GUI list are all
  three lines.
