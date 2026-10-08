/* brain-emu -- host-side logging: the default sink and the level filter.
 * SPDX-License-Identifier: LicenseRef-brain-emu-1.0
 */
#include "host/log/log.h"

#include <cstdarg>
#include <cstdio>

namespace brain::host {

namespace {

LogLevel g_level = LogLevel::kInfo;
LogSink g_sink = nullptr;
void* g_sink_user = nullptr;

void stderr_sink(void* user, LogLevel lvl, const char* message) {
  (void)user;
  std::fprintf(stderr, "%s %s\n", log_level_name(lvl), message);
}

}  // namespace

const char* log_level_name(LogLevel lvl) {
  switch (lvl) {
    case LogLevel::kDebug: return "DEBUG";
    case LogLevel::kInfo:  return "INFO";
    case LogLevel::kWarn:  return "WARN";
    case LogLevel::kError: return "ERROR";
  }
  return "?";
}

void log_set_sink(LogSink fn, void* user) {
  g_sink = fn;
  g_sink_user = user;
}
LogSink log_sink() { return g_sink; }
void* log_sink_user() { return g_sink_user; }

void log_set_level(LogLevel lvl) { g_level = lvl; }
LogLevel log_level() { return g_level; }

void log_msg(LogLevel lvl, const char* fmt, ...) {
  if (lvl < g_level) return;
  char buf[1024];
  va_list ap;
  va_start(ap, fmt);
  std::vsnprintf(buf, sizeof(buf), fmt, ap);
  va_end(ap);
  buf[sizeof(buf) - 1] = 0;
  LogSink fn = g_sink ? g_sink : stderr_sink;
  fn(g_sink_user, lvl, buf);
}

}  // namespace brain::host
