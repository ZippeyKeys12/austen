# Limits and interpretation

## Outcomes

A completed campaign without findings is evidence about the inputs it tried, not a proof of correctness.
Read the report's outcome counters and oracle judgments, not just the command's exit status.
By default, finding a bug still gives a successful CLI exit; `--fail-on-crash` makes a finding fail a CI job.

The original precondition is checked before a candidate is written to memory or passed to C.
A false precondition rejects the candidate without calling the target.
A failed postcondition is a finding, distinct from a target crash.
An unsupported judgment is inconclusive and is not a successful specification check.
Internal errors are also distinct from evidence that the target violates its specification.

Specification sanity checking is advisory.
It can identify likely omissions but cannot establish that a specification fully describes the intended behavior.

## Platform and target requirements

The full campaign workflow is supported and tested on Linux.
macOS has forkserver limitations; specified globals currently require ELF targets and are rejected on macOS.
This release does not provide a Windows campaign workflow.
Do not infer architecture portability for an existing binary, specification layout, or saved input from Rust source portability.

The JSON specification must match the target's C ABI, function signatures, struct layouts, globals, and intended behavior.
A C-compatible object or static library may be compiled independently of CN.
Internal-linkage functions in prebuilt objects need exported wrappers.
Prebuilt targets need compatible instrumentation for target coverage; plain uninstrumented objects remain usable with `--no-coverage`.

The shared input format has a one-byte function selector, a four-byte little-endian size-minus-one, and the choice tape.
The bytes are an input to the specification's decoder, not a portable serialization of arbitrary C values.

## Replay and state

Exact replay uses `--input`; it does not repeat the producer's search.
Use the same specification, compatible Austen build, target ABI/layout, allocator-checking mode, and heap/global addresses as the original run.
Keep custom generator sources when replaying entropy with `--record`/`--replay`.
Matching a random seed alone does not establish replay compatibility across builds or configurations.

Pointers may be encoded as absolute addresses.
The harness requests a fixed heap base and normally fails if it cannot use that address.
`AUSTEN_HEAP_BASE` selects another fixed address; `--allow-unstable-addresses` permits a fallback that can prevent reproducible replay.
Specified globals also require stable addresses.

The harness resets its input heap and specified globals between calls.
It does not restore arbitrary external state, untracked statics, files, time, or external randomness.
A saved input describes one call, so failures dependent on a history of calls may not replay independently.

## What the oracle checks

The oracle checks the selected function's entry and exit specifications.
It does not instrument individual loads/stores, enforce loop invariants, or check every nested function's specification.
An invalid intermediate access may leave an acceptable final state or corrupt a later call.

`--check-heap-ownership` enables allocation bounds, release tracking, and leaked-ownership checks.
It requires a readable native target whose selected link closure permits complete inspection of the supported allocator boundary.
Unsupported allocator APIs, custom allocator definitions, unknown external calls, or incomplete inspection can cause preparation to refuse this mode.
See the [ownership examples](ownership/README.md).

Without that option, target allocations and frees use libc directly, target allocation sizes and lifetimes are not recorded, and foreign post-state memory is checked for accessibility only.
Freeing a harness-materialized owned argument then passes its mapped address to libc and can crash.
Accessibility does not establish allocation bounds, lifetime, or absence of leaks.

## Generation and external tools

Generator and oracle support are separate capabilities.
Unsupported specifications or expressions can prevent useful generation or leave checks inconclusive.
Some arbitrary draws, including top-level owned arrays, datatype values, and mathematical integers, are not supported by the producers.
Darcy's solver queries have resource limits; an unknown or cancelled query rejects a candidate and is not a proof.

Tree generators do not use observed coverage to steer their search.
Delegated fuzzers use their own feedback loops.
External fuzzer, shrinking, and minimization features require the corresponding native tools and compatible instrumentation.
Shrinking confirms the resulting failure through checked replay but does not guarantee a globally minimal input or preservation of the same underlying defect.
