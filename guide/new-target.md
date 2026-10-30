# Add a target

Austen accepts a JSON specification separately from the target's implementation.
The types, function signatures, struct layouts, and global declarations in that specification must describe the target's actual ABI.

Run this tutorial from the repository root after [installation](installation.md).
It uses the supplied [annotated identity function](quickstart/correct.c), so the complete input is available to copy and adapt.

## Export the specification

The function's annotation is:

```c
int identity(int x)
/*@ requires true;
    ensures return == x;
@*/
{
    return x;
}
```

Use a CN installation supporting `--export-spec-json` and the flat `external`/`internal` function-linkage format.
The supplied Dockerfile builds `rems-project/cn` at commit `720f556d821284afe67e0a4842def65ada74f255`; that revision exports the examples in this guide.
Check `cn --version` and `cn test bennet --help` before using another CN installation.
An arbitrary older CN release may lack this exporter.

```sh
mkdir -p target/guide/new-target
cp guide/quickstart/correct.c target/guide/new-target/identity.c
cn test bennet --no-run \
  --output-dir=target/guide/new-target/cn-output \
  --export-spec-json=target/guide/new-target/spec.json \
  --allow-split-magic-comments target/guide/new-target/identity.c
austen sanity target/guide/new-target/spec.json
```

CN parses the annotation and emits the JSON; Austen retains its original precondition and postcondition for runtime checking.
Every function must have an `ensures` entry in JSON, including a trivial `true` postcondition when the source has no clause.
Keep the exported specification with the exact C declarations it describes.
Re-export after changing signatures or specifications.

The checked-in [quickstart specification](quickstart/spec.json) lets you use the remaining steps without CN by substituting that path.

## Compile and test an object

```sh
clang -c target/guide/new-target/identity.c -o target/guide/new-target/identity.o
austen test target/guide/new-target/spec.json target/guide/new-target/identity.o \
  --input=0x00ffffffff07000000 --no-coverage \
  --work=target/guide/new-target/object-run \
  --report=target/guide/new-target/object.json
```

Austen links a generated calling shim against the object.
It does not need CN to parse the implementation at this point.
The report should show one dispatched input and no crash or postcondition failure.
The object has no coverage instrumentation, so a coverage warning is expected even though the specification checks run.

## Test a static archive

```sh
ar rcs target/guide/new-target/libidentity.a target/guide/new-target/identity.o
austen test target/guide/new-target/spec.json target/guide/new-target/libidentity.a \
  --input=0x00ffffffff07000000 --no-coverage \
  --work=target/guide/new-target/archive-run \
  --report=target/guide/new-target/archive.json
```

The expected verdict is the same as for the object.
Ordinary C sources, native objects, and static archives are supported target forms; arbitrary shared libraries and executables are not interchangeable with them.
Specified functions with internal linkage need exported wrappers when supplied as prebuilt objects or archives.

## Generate and adapt

Replace `--input=...` with `--engine=bennet --seed=7 --duration=2` to generate inputs for either target form.
For a source build with coverage, run:

```sh
austen test target/guide/new-target/spec.json target/guide/new-target/identity.c \
  --engine=bennet --seed=7 --duration=2 --work=target/guide/new-target/source-run
```

To try a new function, change the declaration, body, and specification in `identity.c`, then repeat export and compilation.
Use `--function=<C-name>` for one function from a multi-function specification.
For targets that allocate, release, or resize owned memory, read the [ownership example](ownership/README.md) and enable its checking mode consistently.
See [limitations](limitations.md) for inconclusive judgments, replay compatibility, and platform support.
