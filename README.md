# Austen

Austen provides tools for C code with separation-logic specifications supplied as JSON.
Its main tool, **AustenTest** (`austen test`), generates inputs, checks their preconditions before calling your code, and reports crashes and postcondition violations.
AustenTest is a Rust reimplementation of [Bennet](https://zain-k-aamer.com/papers/bennet.pdf) and [Darcy](https://zain-k-aamer.com/papers/codeSpecifyTestDebugProve.pdf), with additional generators and integrations for external fuzzers.

**[Start with the quickstart](guide/quickstart/README.md)**: run the supplied examples, find a postcondition failure, and replay it.
The examples include ready-made JSON specifications, so no CN installation is needed.

- **Generate from specifications:** derive generators from specifications describing values, pointers, and owned memory.
- **Check the original specifications:** evaluate `requires` before calling C and `ensures` after it returns.
- **Choose a testing strategy:** use rejection sampling, backtracking, SMT solving, byte mutation, or an external fuzzer with coverage feedback.
- **Replay and reduce findings:** save binary inputs, reproduce failures, and reduce findings with supported external tools.

## Core concepts

AustenTest separates input generation from target execution.
Specification-based engines produce a choice tree, which is linearized into bytes.
Byte generators and external fuzzers supply bytes directly.
The harness parses each input, checks its precondition, calls C, and checks the postcondition.
Retained inputs pass through shared validation, replay, and reporting.

Three crates provide the common components:

- [`austen`](https://crates.io/crates/austen): the CLI, target builds, and engine selection.
- [`austen-lib`](https://crates.io/crates/austen-lib): the specification language, generator DSL, choice trees and tapes, and shared campaign support.
- [`austen-harness`](https://crates.io/crates/austen-harness): the target runtime and specification oracle.

Each generator or fuzzer integration has its own crate:

| Engine | Approach | Crate |
| --- | --- | --- |
| `bennet` | Backtracking and allocation repair. | [`austen-bennet`](https://crates.io/crates/austen-bennet) |
| `darcy` | SMT constraint solving. | [`austen-darcy`](https://crates.io/crates/austen-darcy) |
| `wickham` | Rejection sampling. | [`austen-wickham`](https://crates.io/crates/austen-wickham) |
| `radamsa` | Raw byte mutation with Radamsa 0.7. | [`austen-radamsa`](https://crates.io/crates/austen-radamsa) |
| `afl++` | Coverage-guided campaigns with AFL++. | [`austen-aflpp`](https://crates.io/crates/austen-aflpp) |
| `honggfuzz` | Coverage-guided campaigns with honggfuzz. | [`austen-honggfuzz`](https://crates.io/crates/austen-honggfuzz) |
| `libfuzzer` | Coverage-guided campaigns with libFuzzer. | [`austen-libfuzzer`](https://crates.io/crates/austen-libfuzzer) |

## Install

Install the command-line application from crates.io.
You need:

- Rust 1.93.1 or newer, including Cargo and `rustc`.
- Z3 5.1.0 with its native library and matching `pkg-config` metadata when enabling Darcy or SMT support.
- A C compiler and linker. Use Clang for the normal coverage-enabled workflow.

```sh
cargo install austen --locked --features bennet,darcy
austen --help
```

Engines are opt-in [Cargo features](https://github.com/ZippeyKeys12/austen/blob/master/crates/austen/README.md#choosing-engines).
The command above enables the recommended Bennet and Darcy engines.
For Bennet alone without Z3, use `--features bennet`.

Keep Cargo's binary directory on `PATH` and `rustc` available to build new target harnesses.
Installation also stages the harness runtime under `~/.local/lib/austen/runtime/`.
Set `AUSTEN_PREFIX` at build time to install the runtime under another prefix.
CN is only needed to export a specification from CN-annotated C.
A JSON specification obtained any other way works without it.
The [installation guide](guide/installation.md) describes the complete runtime installation, relocation, and the included Dockerfile.

## Building from source

Clone the public source repository and install from its workspace:

```sh
git clone https://github.com/ZippeyKeys12/austen.git
cd austen
cargo install --path crates/austen --locked --features bennet,darcy
```

Use `cargo build --workspace --locked --features austen/all-engines` to build every adapter without installing the CLI.
The resulting executable is `target/debug/austen`.
Building the CLI also builds and stages the harness runtime.
Subsequent target builds compile only their generated shim.

Darcy and the `smt` feature link the system Z3 shared library.
Check that `pkg-config --modversion z3` describes the required Z3 5.1.0 installation and that the linker and runtime loader find that same library.
For a custom prefix, configure `PKG_CONFIG_PATH`, the native library search path, and the platform's runtime library path together.
On macOS, `brew install z3 pkg-config` supplies both the library and the metadata that locates it.
A build with `--features bennet` needs no Z3.

Engine features enable adapters and discover tools already installed on the system.
The optional `aflpp-vendored`, `honggfuzz-vendored`, `radamsa-vendored`, and `libfuzzer-vendored` features instead build supported pinned tools or runtimes.
They require additional native dependencies, described in the corresponding [AFL++](crates/austen-aflpp/README.md), [honggfuzz](crates/austen-honggfuzz/README.md), [Radamsa](crates/austen-radamsa/README.md), and [libFuzzer](crates/austen-libfuzzer/README.md) READMEs.
Enable the specific features you need.
`--all-features` also requests tool provisioning.

## Export a specification

AustenTest takes a JSON specification and its corresponding C source, object file, or static archive.
The specification can be written by hand or produced by any tool emitting that JSON shape.
For CN-annotated C, export it using a CN build that supports `--export-spec-json`:

```sh
cn test bennet --no-run --output-dir=/tmp/cn-export \
  --export-spec-json=spec.json --allow-split-magic-comments target.c
```

This exporter is a CN-side addition required by AustenTest.
An arbitrary CN installation may not include it.
Check `cn test bennet --help` for the export option before using the command.
Keep the exported specification together with the C source and any custom generator files it references.
Each function's JSON specification must include `ensures`.
The exporter supplies `true` when the source has no postcondition clause.

## Try it

Start with the included [quickstart](guide/quickstart/README.md) to run a correct target, find a postcondition failure, and replay it without installing CN.
The [new-target tutorial](guide/new-target.md) then walks through specification export and independently compiled objects and archives.

Given a JSON specification `spec.json` and its C target `target.c`:

```sh
austen sanity spec.json
austen test spec.json target.c --duration=30 --work=austen-run
```

The sanity check reports potentially incomplete specifications without compiling or running the target.
AustenTest builds a harness, generates inputs for thirty seconds, then validates retained inputs and reports coverage.
Compilation and final analysis take additional time.
Without `--duration`, testing continues until you press Ctrl-C.

Findings are saved under `austen-run/crashes/`.

Common options:

| Option | Purpose |
| --- | --- |
| `--function=my_function` | Test one specified function by its original C name. |
| `--seed=7` | Use a reproducible generator seed. Otherwise AustenTest chooses and prints one. |
| `--max-generator-size=100` | Set the maximum test-case size for tree generators. |
| `--cflag=-Iinclude --cflag=-O2` | Pass compiler arguments for a C-source target. |
| `--build-only` | Prepare the harness without running a campaign. |
| `--check-heap-ownership` | Track target allocations and releases, enforce ownership bounds, and check for leaked ownership. |

A C-source target lets AustenTest add coverage instrumentation and wrappers for specified static functions.
Prebuilt objects and archives need compatible instrumentation for target coverage and existing wrappers for specified static functions.

Use `--check-heap-ownership` when your target frees or resizes generated owned arguments, or when you want allocation bounds and leak checks.
It requires a readable native target whose allocator calls can be fully inspected.
Unsupported allocator APIs and external calls can make a target ineligible.
Without it, target allocations and leaks are not tracked, and freeing a generated owned argument passes harness-mapped storage to libc and can crash.

## Choose an engine

Select an enabled engine with `--engine=<name>`.
`austen --help` lists the engines in your build.
With Bennet and Darcy enabled, the default is `bennet/darcy`: each function starts with Bennet and switches to Darcy after 10 cumulative discarded generation requests.
Use `--chain-discard-limit=N` to change that threshold.
With only one of Bennet and Darcy enabled, that engine runs alone.
Wickham is the fallback when neither is enabled.

```sh
austen test spec.json target.c --engine=bennet --duration=30
austen test spec.json target.c --engine=wickham/bennet/darcy --duration=30
austen test spec.json target.c --engine=bennet:20,afl++:40
```

Slashes select a tree-generator chain within a campaign.
An explicit chain without a discard limit moves on after 100 exhausted requests for a function only if its current member has produced no input for that function.
Commas select separate timed stages.
Later byte-mutating engines reuse retained non-crashing inputs as seeds.

Radamsa requires Radamsa 0.7.
AFL++ and honggfuzz require their executables and matching instrumentation tools or runtimes.
libFuzzer requires Clang and its compiler-rt runtime.
Its vendored runtime currently requires Clang 18 on Linux.
The engine crate READMEs describe discovery overrides and native dependencies.
Exact-input execution and specification sanity checking also work without engine features.

## Understand results

A crash or failed postcondition is a finding.
A false precondition rejects a candidate without calling C.
An unsupported check is inconclusive.
After independent validation and replay, retained inputs are organized under the work directory:

| Directory | Contents |
| --- | --- |
| `corpus/passing/` | Inputs that returned from C with successful checks. |
| `corpus/precondition-failing/` | Inputs independently confirmed to violate a precondition. |
| `crashes/crash/` | Reproduced target crashes. |
| `crashes/postcondition-failing/` | Reproduced postcondition violations. |

Archives accumulate across runs.
Without `--work`, AustenTest uses a run directory under `~/.cache/austen`, or `$AUSTEN_CACHE_DIR` when set.
Use `--crashes=<directory>` to move the findings archive.
Coverage evaluation is enabled by default.
`--no-coverage` skips it while retaining candidate validation and any engine feedback.

## Replay and reduce findings

Replay a saved binary input by passing its bytes on stdin:

```sh
austen test spec.json target.c --work=austen-run --input=- < saved-input
```

`--input=0x...` accepts the same bytes as hex.
The input contains the function selection and test-case size, so omit generation options such as `--engine`, `--function`, `--seed`, and `--duration`.
Use the same specification, target build, and ownership-checking mode as the original run.
Inputs can contain absolute pointers and need a compatible layout and the same heap base.
`AUSTEN_HEAP_BASE` selects that address.
The default base is fixed.
`--allow-unstable-addresses` permits a fallback that can prevent reproducible replay.

```sh
austen test spec.json target.c --duration=30 --shrink
```

`--shrink` reduces eligible crash and postcondition-failure inputs using an enabled AFL++ or libFuzzer reducer and its native tools.
Use `--shrink=<name>` to select the reducer.
`--minimize-suite` selects coverage-preserving subsets and requires `afl-cmin` and `afl-showmap`.
The two flags can be combined.
For tree generators, `--record=inputs.tape` and `--replay=inputs.tape` save and replay generation decisions, which require the matching generator configuration and build.

## Reports and CI

Finding a bug does not change the command's successful exit status by default.
Use `--fail-on-crash` to make findings fail a CI job:

```sh
austen test spec.json target.c --duration=30 \
  --fail-on-crash --report=run.json --progress=never
```

`--report` writes a JSON summary of outcomes, coverage, effective configuration, and tool provenance.
An interrupted run exits with status 130 and still writes its report.
Progress and diagnostics go to stderr.
`--events=events.jsonl` records structured lifecycle events.
Use `--stats=all` for detailed generator statistics and `austen --help` for the complete option list.

## Documentation

- [Practical guide and examples](guide/README.md): installation, new targets, ownership checking, and extending generation.
- [Generator extension](guide/extension/README.md): a runnable producer using the shared tree, tape, and harness interfaces.
- [Limits and interpretation](guide/limitations.md): supported environments, oracle scope, and replay compatibility.
- [CLI README](crates/austen/README.md): installation and engine features.
- [CN](https://github.com/rems-project/cn): the specification language and upstream tools.

## Current limits

Testing explores inputs.
A run without findings is not a proof of correctness.
Unsupported specification features can leave checks inconclusive.
The full campaign workflow is tested on Linux.
macOS has forkserver limitations, and targets with tracked globals currently require ELF and are rejected on macOS.
Tree generators do not use observed coverage to steer generation.
Delegated fuzzers use their own feedback loops.
Top-level owned-array, datatype-valued, and mathematical-integer arbitrary draws are not currently supported by the producers.
Darcy solver queries have time and memory limits.
An unknown or cancelled query is a rejected candidate, never a proof.

## License

Austen is distributed under the [MIT license](LICENSE).
Dependencies retain their own licenses and notices.
