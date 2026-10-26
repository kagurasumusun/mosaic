# Machine-level boot tests

This directory is the Mosaic counterpart of QEMU's `tests/`: the place where
**real guest images** are booted on **real machines** and what the guest
actually printed is asserted. The full policy is
[ADR 0022](../docs/adr/0022-guest-image-testing.md); the one-line version:

> No synthetic images. No per-OS test code. One catalog (`images.toml`),
> one generic harness, honest milestones.

## Running

```sh
# Everything that is present locally. Missing images skip with a reason.
cargo test -p mosaic-tests-boot --features unicorn -- --nocapture

# Fetch the downloadable real images first (into test-images/, git-ignored):
cargo xtask images fetch
cargo xtask images list     # what is present / verified / missing
cargo xtask images verify   # re-check digests of what is present

# The same boot through the operator-facing entry point:
cargo run -p mosaic-apps-system --features unicorn -- \
    -M pc -cdrom tests/assets/tinycore-16-core.iso -m 64M -serial stdio -display text
```

CI runs without fetching: only the `committed` fixtures (currently the Tiny
Core ISO) execute, and every `download`/`manual` entry skips loudly.

## Adding a guest OS

1. Find the real image. If it is freely redistributable and small, commit it
   under `assets/` (rare). If it is downloadable, add `source = "download"`
   with the URL, the SHA-256 you verified locally, and (for zips) the
   `archive-member`/`archive-sha256` pair. If redistribution is restricted,
   add `source = "manual"` with a note saying how the operator gets it.
2. Boot it once by hand (`mosaic-system -cdrom …`) and set `expect` to the
   milestone the machine actually reached, with `status = "bring-up"` and a
   note naming the next blocker. Never delete the entry afterwards — move
   the milestone forward.
3. There is no step 3. There is no per-OS test to write.

## Layout

| Path | Role |
| --- | --- |
| `images.toml` | The image catalog: provisioning, boot plan, budget, expectations, status |
| `assets/` | Committed fixtures (verifiably redistributable images only) |
| `src/lib.rs` | The harness: resolve → boot → assert, for every entry alike |
| `src/sha256.rs` | SHA-256, validated against the NIST vectors |
| `tests/boot_matrix.rs` | Per-OS-family loops over the catalog |
| `tests/front_end.rs` | End-to-end tests of the `mosaic-system` command line itself |

The `test-images/` directory (fetched and operator-supplied images) is
git-ignored; nothing fetched ever lands in the repository.
