# Add a generation strategy

This standalone Rust project implements `zero-biased`, a strategy that chooses zero for half of its plain integer draws and otherwise uses Wickham's existing sampling policy.
It reuses Wickham's generator-language interpreter, constraint checking, rejection, pointer generation, and retry budget.
The example changes a search policy without implementing another decoder, oracle, or harness.

## Generate and execute inputs

Run these commands from the repository root after [installing Austen](../installation.md):

```sh
mkdir -p target
cargo run --locked --manifest-path guide/extension/Cargo.toml -- target/extension-inputs
austen test guide/extension/bias.json guide/extension/bias.c \
  --input=- --no-coverage --work=target/extension-replay \
  --report=target/extension-replay.json < target/extension-inputs/zero.input
```

The output directory must not exist when the example starts.
Choose a new directory to rerun it.
The program uses seed 7 and size 25, prints the decoded argument for each of 16 inputs, and saves their complete byte inputs.
It also saves one input with argument zero as `zero.input`.
The target promises `return == x` but returns 1 for zero, so replay reports a postcondition failure.
Austen normally exits successfully after recording a finding.
Inspect the postcondition-failure count in the report, or add `--fail-on-crash` when a finding should produce a nonzero exit.

The example's JSON is a hand-written one-argument specification matching the annotations in its [C file](bias.c).
It requires no CN installation to run.
The [new-target tutorial](../new-target.md) explains exporting other specifications.

## The input pipeline

Every engine meets the harness at one byte format, built and consumed by shared types in `austen_lib::gen_dsl` (and `austen_lib::wire` for the input itself):

```text
TreeProducer ─▶ ChoiceTree ─▶ TreeLinearizer ─▶ ChoiceTape ─▶ AustenInput
                                                                   │
                                     ParseOutput ◀── TapeParser ◀──┘
```

- **`TreeProducer`** is the trait a generation strategy implements. `produce_in` interprets the supplied generator term (a fully applied root call carrying the test-case size) and returns a `ChoiceTree`, or `Ok(None)` when interrupted. The producer owns its module, randomness, and search policy. Nothing about how it searched leaks into the result.
- **`ChoiceTree`** records each choice the producer made, keyed by a stable label within a named scope rather than by the order values were generated. Branch arms, calls, and map iterations each get their own scope, so a producer may generate values in any order.
- **`TreeLinearizer`** flattens a `ChoiceTree` into a `ChoiceTape` by walking the input-decoding definition, the generator retained unchanged before any optimization, and emitting each choice in that fixed order. It checks that tree and definition agree choice for choice, so an optimized producer cannot change what saved bytes mean.
- **`ChoiceTape`** is the flat payload: choices in decoding order with no length, checksum, or tags. Pointers are absolute 64-bit addresses, with zero for NULL.
- **`AustenInput`** is the complete input: a one-byte function ID, the test-case size minus one as a little-endian `u32`, and the tape. `AustenInput::from_parts` builds it. External fuzzers and `--input` supply these bytes directly.
- **`TapeParser`** decodes a tape against the same definition. A short tape is extended from a fixed-seed stream and trailing bytes are ignored, so payload length never invalidates an input. Checked parsing re-evaluates the generator's assertions and memory-write bounds. Inputs from external fuzzers or users are always parsed checked.
- **`ParseOutput`** is the decoded candidate: the returned argument record (`args()` gives the C arguments), proposed memory writes, and sampled defaults. It is not permission to execute. The harness checks the original precondition against it before writing memory or calling C, and checks the postcondition after the call returns.

## Producer and factory

[src/lib.rs](src/lib.rs) contains three small pieces:

1. `ZeroBiasedScalars` implements `PrimitiveGenerator::arbitrary`. Its extra coin comes from the supplied entropy stream. It never creates a second random-number generator. Integer zero draws use the requested exact scalar type. Other draws fall back to `WickhamPrimitiveGenerator`.
2. `ZeroBiased` implements `TreeProducer::produce_in` and forwards the supplied term and `ProduceCx` to its configured Wickham interpreter. It also forwards `RejectingProducer` statistics and `EntropyBacked` access, making it a `TreeEngine` through the library's blanket implementation.
3. `ZeroBiasedFactory` implements `TreeEngineFactory`. It selects Wickham's pass profile, obtains the matching production module, and builds the persistent producer using the supplied memory map, entropy, retry count, and timeout. The inherited `open` method validates the wire/production module pair before calling `open_validated`.

This is a bias on plain `arbitrary` integer draws.
Constraints and compiler specialization may fix a value without such a draw.
The bias does not override them.
Input defaults retain their shared per-input sampling policy.
Repeated zeros can make some constraints harder to satisfy, so this is an extension example rather than a claim of better bug finding.

When replacing the interpreter itself, preserve the same contract: interpret the supplied fully applied root call, retain its size across retries, use the supplied evaluator for shared defaults, poll cancellation, and produce choices with the wire module's labels and scopes.
Return `Ok(None)` for interruption and a structured `GenFailure` for a failed request.
Do not replace a completed result merely because cancellation arrived afterward.

## Registration and the shared input path

[src/main.rs](src/main.rs) registers the factory in an application-owned `EngineRegistry`:

```rust
static ENGINES: &[AnyEngine] = &[AnyEngine::AustenDriven(AnyGenerator::Tree(
    AnyTreeEngine::new(&ZeroBiasedFactory),
))];
static REGISTRY: EngineRegistry = EngineRegistry::new(ENGINES);
```

The program resolves that registered factory, converts the supplied specification using its pass profile, and opens one producer.
For each input it creates a fresh evaluator, passes a fully applied root call to the producer, and linearizes the resulting tree using the retained wire definition and the same size/default choices.
`TapeParser` checks the resulting tape and exposes the decoded argument.
`AustenInput::from_parts` adds the function selector and size header.
The ordinary `austen test --input` command then sends those bytes through the real harness and original-specification oracle.
Successful parsing alone is not an oracle verdict.

This demonstration has no pointers, globals, or proposed memory writes, so its local numeric heap description is never used to access memory.
A general campaign must use the actual incoming regions published by its harness.
Use the existing `TreeGenerator` campaign adapter for function/size selection, per-input defaults, linearization, and construction of `AustenInput`.
The explicit steps here are for understanding that boundary.

## Make the strategy selectable in your CLI build

Registration in this executable does not alter a separately installed Austen binary.
To include this factory in your own source build, add the following entries to the existing sections of `crates/austen/Cargo.toml`:

```toml
[features]
zero-biased = ["dep:austen-zero-biased-example", "wickham"]

[dependencies]
austen-zero-biased-example = { path = "../../guide/extension", optional = true }
```

Enabling `wickham` also enables the CLI's existing tree-engine support.
Then add this entry to the `ENGINES` slice in `crates/austen/src/registry.rs`:

```rust
#[cfg(feature = "zero-biased")]
AnyEngine::AustenDriven(AnyGenerator::Tree(AnyTreeEngine::new(
    &austen_zero_biased_example::ZeroBiasedFactory,
))),
```

Build and select the new engine explicitly:

```sh
cargo build -p austen --features zero-biased
./target/debug/austen test guide/extension/bias.json guide/extension/bias.c \
  --engine=zero-biased --duration=3 --seed=7 --no-coverage \
  --work=target/zero-biased-campaign --report=target/zero-biased-campaign.json
```

Adding a dependency changes the workspace lockfile, so the first build intentionally allows Cargo to update it.
Retain that updated lockfile and use `--locked` for subsequent builds.
The report's engine is `zero-biased`.
The common campaign path supplies generation statistics, checked replay, and findings.
These edits are a tutorial for your own build.
The released CLI does not include this example engine by default.

For a strategy that already produces flat bytes, implement the `Generator` interface and its `GeneratorFactory`/`PreparedGenerator` lifecycle instead.
Declare checked trust for externally produced inputs and register the factory through `AnyGenerator::Other`.
That route enters at `AustenInput` and does not use a `TreeProducer` or linearizer.
