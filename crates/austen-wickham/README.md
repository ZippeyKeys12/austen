# `austen-wickham`: Rejection sampling

Wickham generates test inputs for C functions from their JSON specifications using rejection sampling.
It provides AustenTest with a simple baseline for comparing input-generation strategies.

Wickham draws values and checks constraints as it builds an input.
If a check fails, it discards that attempt and starts again.
Pointer values can be NULL, refer to an existing allocation, or point to newly allocated memory.
Unlike Bennet, Wickham restarts the whole input instead of going back to repair individual choices.

## Use it

To use Wickham with Austen, install the `austen` CLI with the `wickham` feature:

```sh
cargo install austen --locked --features wickham
austen test spec.json target.c --engine=wickham --duration=30
```

Here, `spec.json` is the JSON specification for your `target.c`.
See the [build setup](https://github.com/ZippeyKeys12/austen/blob/master/README.md#building-from-source) for prerequisites and the [user guide](https://github.com/ZippeyKeys12/austen/blob/master/README.md#try-it) for specification export and campaigns.

Add this crate with `cargo add austen-wickham` only when building your own Rust tooling on the generator.
