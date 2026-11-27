# Zed for WaOS

The branch `waos` of `github.com/waos-dev/zed` is Zed ported to [WaOS](https://github.com/waos-dev/waos): Zed's workspace built for `wasm32-wasip3` and run as the WaOS program `dev.waos.zed`. Zed runs inside WaOS; the project, git, the terminal's shell and language servers are on a machine WaOS connects it to (this computer, or another), whose root the program mounts at `/`.

What is on top of upstream Zed:

- `crates/zed_waos`: the program, Zed's workspace with its panels, search, diagnostics and settings.
- Zed's crates built for WASI (`#[cfg(target_os = "wasi")]` paths, each marked `WaOS port:`), GPUI's WASI renderer and executor among them.
- `waos/patched/`: third-party crates changed for WASI, `[patch]`ed in the root `Cargo.toml` (`waos/patched/README.md` says what each is and what changed).
- The root `Cargo.toml` (WaOS's SDK from `github.com/waos-dev/waos` at a fixed revision, and the patches) and `rust-toolchain.toml` (WaOS's nightly, with `wasm32-wasip3`).
- `.github/workflows/waos-release.yml`, the release. Zed's own workflows are left as upstream has them; none runs on this branch or its tags.

## Building

Needed:

- Rust through rustup: `rustup toolchain install` in the checkout installs `rust-toolchain.toml`'s.
- [wasi-sdk 34](https://github.com/WebAssembly/wasi-sdk/releases/tag/wasi-sdk-34), unpacked anywhere: the C compiler for SQLite, aws-lc and tree-sitter's grammars. It must be 34 or newer: rustc's wasip3 links with `--cooperative-threading`, which takes no C object built by an older LLVM. With `SDK` its folder (`wasi-sdk-34.0-arm64-macos`, `wasi-sdk-34.0-x86_64-linux`, …):

  ```sh
  export CC_wasm32_wasip3="$SDK/bin/clang" CXX_wasm32_wasip3="$SDK/bin/clang++" AR_wasm32_wasip3="$SDK/bin/llvm-ar"
  export WASI_SYSROOT="$SDK/share/wasi-sysroot"
  export CFLAGS_wasm32_wasip3="--sysroot=$WASI_SYSROOT" CXXFLAGS_wasm32_wasip3="--sysroot=$WASI_SYSROOT"
  ```

- `waos`, of the WaOS release the root `Cargo.toml` takes the SDK at:

  ```sh
  cargo install waos-cli --version 0.8.1 --locked
  ```

Then, with the variables above set for every command (or cc-rs builds aws-lc with the system's compiler and the link fails):

```sh
export CARGO_PROFILE_RELEASE_STRIP=debuginfo   # the same every time, or cargo rebuilds every crate
waos run -p zed_waos --release                 # in the WaOS app installed on this computer
waos package -p zed_waos                       # target/waos/dev.waos.zed-<version>.wapk and index.json
waos package verify target/waos/dev.waos.zed-<version>.wapk
```

`waos package` signs with your own signing key, or `WAOS_SIGNING_KEY`'s when it is set.

## Releasing

1. Set the version in `crates/zed_waos/Cargo.toml` and commit it on `waos`.
2. Push a tag `waos-v<that version>`, e.g. `git tag waos-v0.1.0 && git push origin waos-v0.1.0`.

`waos-release.yml` checks the tag against the version, builds and packages the program on Linux, signed with the WaOS project's key (the repository's secret `WAOS_SIGNING_KEY`), and attaches the `.wapk` and its `index.json` to the tag's GitHub release, marked latest. WaOS reads a repository's latest release, so `github.com/waos-dev/zed` added as a source in WaOS installs and updates Zed. A failed release is run again from its run's page (Re-run jobs). The workflow can also be run by hand for a tag that exists, but GitHub offers that only for a workflow on the default branch, `main`, which this file is not on.

## Keeping up with upstream Zed

`main` follows `zed-industries/zed`; the port lives on `waos`. To take a newer Zed:

```sh
git fetch https://github.com/zed-industries/zed main
git merge FETCH_HEAD          # on waos; `git diff FETCH_HEAD...waos` is the whole port
```

- Conflicts are in the `WaOS port:` changes. `rust-toolchain.toml` stays WaOS's.
- Where upstream moves a crate `waos/patched` has a copy of (tree-sitter's or alacritty's revision, smol, blocking, dirs, getrandom 0.3, psm), the copy is redone from the new source with its change carried over, or its `[patch]` no longer applies and cargo says it is unused.
- New code in Zed's crates may need its own WASI path: `cargo check -p zed_waos --target wasm32-wasip3` with the variables above finds it.
- Taking a newer WaOS is the `tag` of `wasdk`, `wasdk-wgpu` and `wasdk-adapter-gpui` in the root `Cargo.toml`; the release installs `waos` from the revision in the lockfile. WaOS's adapter names GPUI by a revision of `github.com/waos-dev/zed`, which `[patch."https://github.com/waos-dev/zed"]` replaces with this checkout's `gpui`.
