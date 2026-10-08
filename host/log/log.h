/* brain-emu -- host-side logging.
 * SPDX-License-Identifier: LicenseRef-brain-emu-1.0
 *
 * The one logging facility for the whole tree: core, devices, CPU modules and
 * host services all report through it, so a run produces one consistent stream
 * with levels instead of every component inventing its own `fprintf(stderr)`.
 *
 * It is deliberately tiny: a level, a sink (where lines go) and a printf-style
 * entry point.  The default sink writes "LEVEL message" to stderr; a test or a
 * GUI replaces it with a capture sink; a file sink is three lines with the same
 * signature.  Nothing here allocates after the line is formatted, and a line
 * longer than the buffer is truncated, never overflowed.
 */
#ifndef BRAINEMU_HOST_LOG_H
#define BRAINEMU_HOST_LOG_H

#include "brainemu/types.h"

namespace brain::host {

enum class LogLevel : u32 {
  kDebug = 0,
  kInfo,
  kWarn,
  kError,
};

const char* log_level_name(LogLevel lvl);

/* Where a formatted line goes.  `message` is NUL-terminated and already carries
 * the level prefix; the sink owns no state beyond `user`. */
using LogSink = void (*)(void* user, LogLevel lvl, const char* message);

/* Replace the sink (null restores the default stderr sink). */
void log_set_sink(LogSink fn, void* user);
LogSink log_sink();
void* log_sink_user();

/* Save/restore for tests and nested scopes: installs `fn` on construction and
 * puts the previous sink back on destruction, so a suite can capture the log
 * around one section and leave the world as it found it. */
class LogSinkScope {
 public:
  LogSinkScope(LogSink fn, void* user)
      : prev_fn_(log_sink()), prev_user_(log_sink_user()) {
    log_set_sink(fn, user);
  }
  ~LogSinkScope() { log_set_sink(prev_fn_, prev_user_); }
  LogSinkScope(const LogSinkScope&) = delete;
  LogSinkScope& operator=(const LogSinkScope&) = delete;

 private:
  LogSink prev_fn_;
  void* prev_user_;
};

/* Lines below this level are dropped before they are formatted. */
void log_set_level(LogLevel lvl);
LogLevel log_level();

/* Format and emit one line.  A trailing newline is added when the format does
 * not end with one. */
void log_msg(LogLevel lvl, const char* fmt, ...)
    __attribute__((format(printf, 2, 3)));

}  // namespace brain::host

/* The four entry points, spelled as functions so a call site reads like a
 * sentence: log_info("uart: %u bytes\n", n); */
#define log_debug(...) ::brain::host::log_msg(::brain::host::LogLevel::kDebug, __VA_ARGS__)
#define log_info(...)  ::brain::host::log_msg(::brain::host::LogLevel::kInfo, __VA_ARGS__)
#define log_warn(...)  ::brain::host::log_msg(::brain::host::LogLevel::kWarn, __VA_ARGS__)
#define log_error(...) ::brain::host::log_msg(::brain::host::LogLevel::kError, __VA_ARGS__)

#endif  /* BRAINEMU_HOST_LOG_H */
