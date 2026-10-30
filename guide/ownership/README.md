# Track allocation and release

The [specification](spec.json) requires ownership of one initialized `int` through argument `p`.
Its postcondition is `true`: the function must return no owned heap memory.
In ownership-checking mode, any remaining owned bytes therefore constitute a leak.

[correct.c](correct.c) allocates a temporary integer, copies `*p` into it, frees the temporary, and frees `p`.
[buggy.c](buggy.c) omits the temporary's release.
The volatile store keeps the temporary's use observable to the compiler.
Neither implementation deliberately accesses freed memory.

Run these commands from the repository root after [installation](../installation.md):

```sh
AUSTEN_HEAP_BASE=1073741824 austen test \
  guide/ownership/spec.json guide/ownership/correct.c \
  --check-heap-ownership --no-coverage \
  --input=0x00ffffffff00000040000000002a000000 \
  --work=target/guide/ownership-good --report=target/guide/ownership-good.json

AUSTEN_HEAP_BASE=1073741824 austen test \
  guide/ownership/spec.json guide/ownership/buggy.c \
  --check-heap-ownership --no-coverage \
  --input=0x00ffffffff00000040000000002a000000 \
  --work=target/guide/ownership-bug --report=target/guide/ownership-bug.json
```

The input encodes `p = 0x40000000` and an initial value of 42.
The environment variable requests that same fixed heap base.
Startup fails if the address is unavailable.
These supplied bytes assume the 64-bit little-endian platforms supported by this example.

| Report field | Correct target | Buggy target |
| --- | --- | --- |
| `check_heap_ownership` | `true` | `true` |
| `dispatched` | `1` | `1` |
| `crashes` | `0` | `1` |
| `findings.post_failed` | `0` | `1` |
| `oracle.post_unsupported` | `0` | `0` |

The failure expectation assumes the four-byte temporary allocation succeeds.
The faulty path leaves that allocation live, and the oracle reports leaked ownership at return.
The harness reclaims surviving tracked allocations after checking.

`p` points into memory materialized by the harness rather than an ordinary libc allocation.
With `--check-heap-ownership`, target preparation redirects allocator references and the harness recognizes the exact generated allocation base when `free(p)` executes.
Keep this flag for both targets and their replays.
Without it, passing the harness mapping to libc `free` can crash.

The mode requires a target whose native link closure can be inspected and whose allocator operations can be tracked.
An unsupported external call or allocator API can make preparation refuse the target.
It does not instrument individual target loads and stores.

To generate inputs instead, replace `--input=...` with `--engine=bennet --seed=7 --duration=2`, retaining `--check-heap-ownership`.
To replay a saved failure, retain the same heap base, specification, target, and ownership flag, and use `--input=-` with the saved file on stdin, as in the [quickstart](../quickstart/README.md#replay-the-retained-input).

Running `austen sanity guide/ownership/spec.json` reports an informational `ASC_TRIVIAL_POSTCONDITION` finding.
Here that postcondition is intentional: the example demonstrates consuming all owned heap memory.

These targets were written for this guide and are distributed under the repository's [MIT license](../../LICENSE).
`spec.json` was exported from `correct.c` using CN commit `720f556d821284afe67e0a4842def65ada74f255` from `rems-project/cn`.
