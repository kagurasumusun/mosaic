# `host/fs/` — the filesystem service

The one abstraction a guest's file access and the host's own tooling share. A
guest-side call (a semihosting `SYS_OPEN`, a linux-user `open`) asks for "open
this path, read these bytes"; the host answers with bytes from wherever files
actually live. The interface is the same for both implementations, which is
what makes the guest-facing wiring independent of where the files are.

| file | what it holds |
| --- | --- |
| `fs.h` | the contract: open flags (`kFsRead`/`kFsWrite`/`kFsCreate`/`kFsTruncate`/`kFsAppend`), seek whence, `FsStat`, and `class Fs` — `open`/`close`/`read`/`write`/`seek`/`stat`/`unlink` over small integer handles, with errors as negative errno-style codes and `Fs::err_text` for their text. |
| `host_fs.cpp` | `HostFs`: a filesystem rooted at a host directory. Guest paths are resolved component by component against the root; `..` that would climb above the root is refused with `-EACCES`, and a leading `/` is relative to the root. The file layer is POSIX fds, not `FILE*`, because the flag set has no exact `fopen` spelling. |
| `mem_fs.cpp` | `MemFs`: files as strings in a map — what a test uses for a guest that reads a "file", and what the host uses to serve a firmware blob without touching disk. A handle works on its own copy of the bytes; a writable handle commits it back on close. |
| `tests/test_fs.cpp` | one shared contract driven against both implementations (`HostFs` in a `mkdtemp` scratch directory, `MemFs` in memory), so the two cannot drift apart. |

Paths are slash-separated and relative to the filesystem's own root. Handles
are 1-based; 0 is never valid. Errors are negative `<cerrno>` values, so a
caller reports them without a second error channel.

A guest-facing bridge (wiring the ARM926's semihosting/syscall glue to an
`Fs`) is a follow-up: the `Cpu` syscall fields exist for it but are not wired
yet, so today the service is exercised host-side, in the tests.
