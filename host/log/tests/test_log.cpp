/* brain-emu -- host/log tests: the sink, the level filter, the formatting.
 * SPDX-License-Identifier: LicenseRef-brain-emu-1.0
 */
#include <cstdio>
#include <cstring>
#include <string>
#include <vector>

#include "host/log/log.h"
#include "testkit/test_support.h"

namespace {

struct Capture {
  std::vector<std::string> lines;
  std::vector<brain::host::LogLevel> levels;
};

void capture_sink(void* user, brain::host::LogLevel lvl, const char* msg) {
  Capture* c = static_cast<Capture*>(user);
  c->lines.emplace_back(msg);
  c->levels.push_back(lvl);
}

}  // namespace

int main() {
  using brain::host::LogLevel;
  using brain::host::LogSinkScope;

  Capture cap;
  {
    LogSinkScope scope(capture_sink, &cap);

    brain::host::log_set_level(LogLevel::kDebug);
    log_debug("dbg %d", 1);
    log_info("inf");
    log_warn("wrn");
    log_error("err");

    /* Below the level: dropped before formatting. */
    brain::host::log_set_level(LogLevel::kWarn);
    log_info("hidden");
    log_debug("hidden too");
    log_warn("shown");

    /* Formatting: printf-style, and a trailing newline is not doubled (the
     * sink adds the line break; the message itself is the text). */
    log_error("fmt %s %08x", "hex", 0xABCDu);
  }

  CHECK(cap.lines.size() == 6);
  if (cap.lines.size() == 6) {
    CHECK(cap.lines[0] == "dbg 1");
    CHECK(cap.lines[1] == "inf");
    CHECK(cap.lines[2] == "wrn");
    CHECK(cap.lines[3] == "err");
    CHECK(cap.lines[4] == "shown");
    CHECK(cap.lines[5] == "fmt hex 0000abcd");
    CHECK(cap.levels[0] == LogLevel::kDebug);
    CHECK(cap.levels[1] == LogLevel::kInfo);
    CHECK(cap.levels[2] == LogLevel::kWarn);
    CHECK(cap.levels[3] == LogLevel::kError);
    CHECK(cap.levels[4] == LogLevel::kWarn);
    CHECK(cap.levels[5] == LogLevel::kError);
  }

  CHECK(std::strcmp(brain::host::log_level_name(LogLevel::kInfo), "INFO") == 0);
  CHECK(std::strcmp(brain::host::log_level_name(LogLevel::kError), "ERROR") == 0);

  /* The scope put the default sink back: logging now goes to stderr again,
   * which the capture must not see. */
  CHECK(cap.lines.size() == 6);
  log_info("after scope");
  CHECK(cap.lines.size() == 6);

  /* A null sink restores the default. */
  brain::host::log_set_sink(nullptr, nullptr);
  CHECK(brain::host::log_sink() == nullptr);

  return brainemutest::summary("host/log");
}
