# Patched third-party crates

Copies of crates Zed depends on, each changed so it builds and runs for `wasm32-wasip3` under WaOS, and `[patch]`ed in the root `Cargo.toml`. Each keeps its own license; only the files listed changed from the source named.

| Crate | Source | What changed |
|---|---|---|
| `alacritty_terminal` | `zed-industries/alacritty` at `4c129667ce56611becdc82de6e28218c80e2e88f` | `Cargo.toml`, `src/lib.rs`: no PTY and no event loop on WASI (Zed's terminal runs its command on a machine through `wasdk`). Its `tests/` (reference fixtures) are left out. |
| `blocking` | crates.io 1.6.2 | `src/lib.rs`: blocking work runs in place on WASI, with no thread pool to run it on. |
| `dirs` | crates.io 6.0.0 | `src/wasm.rs`: `home_dir` reads `$HOME` on WASI, which the port sets to the machine's home. |
| `getrandom-0.3` | crates.io 0.3.4 | `Cargo.toml`, `src/backends.rs`, `src/backends/wasi_p3.rs` (new): Preview 3 forwarded to getrandom 0.4. |
| `psm` | crates.io 0.1.30 | `build.rs`: no assembly on wasm32-wasip3, whose linker refuses the shipped `wasm32.o`. |
| `smol` | crates.io 2.0.2 | Rewritten: smol's API from the same crates, without async-io's reactor on WASI. |
| `tree-sitter` | `tree-sitter/tree-sitter` at `803c6ebaac27d69e1808b45058482e868d073ceb`, the whole repository (its workspace is needed to build `lib` and `crates/language`), less `docs/` and `test/` | `lib/binding_rust/lib.rs`: its `Send`/`Sync` impls kept on WASI. |

When upstream Zed moves one of these to a new version or revision, the copy is redone from that source and its change carried over.
