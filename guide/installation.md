# Installation and the Docker toolchain

Run these commands from the root of the source checkout.
For a first run, use the files in the [quickstart](quickstart/README.md); no CN installation is needed for those supplied JSON specifications.

## Use the Dockerfile

The root [Dockerfile](../Dockerfile) provides the development toolchain: Rust, Clang, Z3, CN, and the external fuzzing tools.
Build it and mount the source checkout to work inside it:

```sh
docker build -t austen-dev .
docker run --rm -it --init --shm-size=1g \
  --security-opt seccomp=unconfined \
  --mount "type=bind,source=$PWD,target=/workspaces/austen" \
  --workdir /workspaces/austen austen-dev bash
```

The initial image build downloads toolchains and can take substantially longer than an incremental build.
The run command supplies shared-memory space for fuzzing and permits syscalls used by the supported harnesses and fuzzers.
The mounted checkout must be writable by the image's `ubuntu` user.
Check the CN export capability as described in the [new-target tutorial](new-target.md).

Inside the container:

```sh
cargo install --path crates/austen --locked --features bennet,darcy,wickham
austen --help
```

## Install without a container

Provide Rust 1.93.1 or newer (Cargo and `rustc`), a C compiler/linker, and Clang for coverage-enabled target builds.
Target preparation also needs `ar` and an object-copy tool on `PATH`: `objcopy` on Linux, or `llvm-objcopy` on macOS.
Set `AR` or `OBJCOPY` to select an alternative executable.
Darcy additionally needs Z3 5.1.0, its shared library, and matching `pkg-config` metadata:

```sh
rustc --version
cargo --version
clang --version
pkg-config --modversion z3
cargo install --path crates/austen --locked --features bennet,darcy,wickham
```

The linker and runtime loader must select the same Z3 installation reported by `pkg-config`.
For a custom Z3 prefix, set `PKG_CONFIG_PATH`, the native library search path, and the platform's runtime library path together.
The repository retains Homebrew paths for macOS in `.cargo/config.toml`; the Dockerfile supplies its Linux paths through the environment.
See the [platform limitations](limitations.md) before choosing a host.

For an installation that does not need Z3, select `--features bennet,wickham` instead.
Engine features enable adapters; they do not install external fuzzers.
The engine crate READMEs linked from the [project README](../README.md#core-concepts) describe native tool requirements and the optional vendoring features.

## Keep the complete installation together

Installing the CLI also builds the harness runtime using the supplied Cargo lockfile.
A new target needs all of the following:

| Component | Purpose |
| --- | --- |
| `austen` | CLI and campaign driver. Cargo installs it under its binary directory. |
| `libausten_harness.so` (Linux) or `.dylib` (macOS) | Shared input parser, oracle, and execution runtime. |
| `harness_main.rs.in` | Template used to compile the target-specific harness. |
| `RUNTIME` | Matching runtime provenance stamp. |
| `rustc`, a C compiler, and linker | Compile and link a harness for each new target. |
| `ar` and `objcopy` (`llvm-objcopy` on macOS) | Prepare target objects and static archives. |
| Native libraries such as Z3 | Required by the enabled engines and linked targets. |

By default the three runtime files live together at `~/.local/lib/austen/runtime/<checkout-key>/`.
The CLI records that absolute location during its build.
The checkout key keeps installations built from different source directories separate.
Copying only the executable does not copy the runtime or its native dependencies.

`AUSTEN_PREFIX` selects the runtime and vendored-tool installation prefix **at build time**.
Cargo's `--root` separately selects where Cargo installs the executable.
For example, install both beneath one prefix:

```sh
austen_prefix="$HOME/.local/austen"
AUSTEN_PREFIX="$austen_prefix" cargo install --path crates/austen \
  --root "$austen_prefix" --locked --features bennet,darcy,wickham
export PATH="$austen_prefix/bin:$PATH"
```

If you relocate an installation, copy the complete runtime directory and set `AUSTEN_RUNTIME` to its new **absolute directory**, containing all three runtime files.
Use the runtime built together with that CLI for the same host architecture.
`AUSTEN_RUNTIME` takes precedence over the embedded location; an incomplete directory or incompatible host is an error.
Changing `AUSTEN_PREFIX` only when running the CLI does not relocate an existing installation.
Runtime version/source warnings should be resolved before comparing or replaying results.
