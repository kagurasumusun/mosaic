/* brain-emu -- the command monitor: a debugger's console over the C API.
 * SPDX-License-Identifier: LicenseRef-brain-emu-1.0
 *
 * The monitor a user types into: `regs`, `x/4xw 0x1000`, `break`, `continue`,
 * `step`.  It works against `brain_emu_t*` - the same handle the CLI and a GUI
 * hold - so it drives any machine the core can build, and it knows no CPU, no
 * instruction set, no device: everything it can do is a C API call.
 *
 * `execute()` runs one line and returns its output, which is what makes the
 * monitor testable without a terminal; `run()` is the stdio REPL on top of it
 * (the CLI's `--monitor-cmd`), reading lines until `quit` or EOF.
 */
#ifndef BRAINEMU_HOST_MONITOR_COMMAND_MONITOR_H
#define BRAINEMU_HOST_MONITOR_COMMAND_MONITOR_H

#include <string>
#include <vector>

#include "brainemu/brainemu.h"
#include "brainemu/types.h"

namespace brain::host {

class CommandMonitor {
 public:
  explicit CommandMonitor(brain_emu_t* e) : e_(e) {}

  /* Run one command line; returns the text to show the user.  An empty line
   * returns an empty string, an unknown command says so.  `quit` (or the REPL's
   * EOF) is the only thing that sets quit(). */
  std::string execute(const std::string& line);

  /* The stdio REPL: prompt, read a line, execute, print, repeat.  Returns when
   * the user quits or stdin ends. */
  void run();

  bool quit() const { return quit_; }

 private:
  /* The commands, one function each; `execute` dispatches on the first word. */
  std::string cmd_help();
  std::string cmd_info();
  std::string cmd_regs();
  std::string cmd_devices();
  std::string cmd_stats();
  std::string cmd_mem(const std::string& line);
  std::string cmd_set(const std::vector<std::string>& args);
  std::string cmd_reg(const std::vector<std::string>& args);
  std::string cmd_break(const std::vector<std::string>& args);
  std::string cmd_delete(const std::vector<std::string>& args);
  std::string cmd_step(const std::vector<std::string>& args);
  std::string cmd_cont(const std::vector<std::string>& args);
  std::string cmd_reset();

  bool hit_break(u32 pc) const;
  /* Parse "0x1234"/"1234"; returns false when the text is not a number. */
  static bool parse_u32(const std::string& s, u32* out);
  static bool parse_u64(const std::string& s, u64* out);

  brain_emu_t* e_;
  bool quit_ = false;
  std::vector<u32> breaks_;
};

}  // namespace brain::host

#endif  /* BRAINEMU_HOST_MONITOR_COMMAND_MONITOR_H */
