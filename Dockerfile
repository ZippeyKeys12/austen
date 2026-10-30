FROM ubuntu:24.04

ARG DEBIAN_FRONTEND=noninteractive

RUN apt-get update && apt-get install -y --no-install-recommends \
        build-essential ca-certificates curl gh git gnupg gzip just less libclang-dev \
        libgmp-dev libmpfr-dev m4 opam openssh-client pkg-config procps python3 sudo \
        time unzip valgrind wget zsh \
        clang-18 clang-format-18 clang-tidy-18 lld-18 lldb-18 llvm-18-dev \
    && ln -sf "$(command -v clang-18)" /usr/local/bin/clang \
    && ln -sf "$(command -v clang++-18)" /usr/local/bin/clang++ \
    && ln -sf "$(command -v clang-format-18)" /usr/local/bin/clang-format \
    && ln -sf "$(command -v clang-tidy-18)" /usr/local/bin/clang-tidy \
    && ln -sf "$(command -v lldb-18)" /usr/local/bin/lldb \
    && rm -rf /var/lib/apt/lists/*

ARG Z3_VERSION=5.1.0
RUN set -eux; \
    arch="$(dpkg --print-architecture)"; \
    case "$arch" in \
        amd64) plat="x64-glibc-2.39" ;; \
        arm64) plat="arm64-glibc-2.38" ;; \
        *) echo "unsupported architecture: $arch" >&2; exit 1 ;; \
    esac; \
    curl -fsSL -o /tmp/z3.zip \
        "https://github.com/Z3Prover/z3/releases/download/z3-${Z3_VERSION}/z3-${Z3_VERSION}-${plat}.zip"; \
    unzip -q /tmp/z3.zip -d /tmp; \
    cp    "/tmp/z3-${Z3_VERSION}-${plat}/bin/z3"       /usr/local/bin/; \
    cp    "/tmp/z3-${Z3_VERSION}-${plat}"/bin/libz3.*  /usr/local/lib/; \
    cp -r "/tmp/z3-${Z3_VERSION}-${plat}"/include/.    /usr/local/include/; \
    mkdir -p /usr/local/lib/pkgconfig; \
    printf '%s\n' \
        'prefix=/usr/local' \
        'exec_prefix=${prefix}' \
        'libdir=${exec_prefix}/lib' \
        'includedir=${prefix}/include' \
        '' \
        'Name: z3' \
        'Description: The Z3 Theorem Prover' \
        "Version: ${Z3_VERSION}" \
        'Libs: -L${libdir} -lz3' \
        'Cflags: -I${includedir}' \
        > /usr/local/lib/pkgconfig/z3.pc; \
    rm -rf /tmp/z3.zip "/tmp/z3-${Z3_VERSION}-${plat}"; \
    ldconfig; \
    z3 --version; \
    test "$(PKG_CONFIG_PATH=/usr/local/lib/pkgconfig pkg-config --modversion z3)" = "${Z3_VERSION}"

ENV RUSTFLAGS="-L native=/usr/local/lib" \
    LIBRARY_PATH=/usr/local/lib \
    LD_LIBRARY_PATH=/usr/local/lib \
    PKG_CONFIG_PATH=/usr/local/lib/pkgconfig

ARG AFL_GIT_REF=v5.03c
RUN set -eux; \
    gcc_major="$(gcc -dumpversion | cut -d. -f1)"; \
    apt-get update; \
    apt-get install -y --no-install-recommends \
        automake bison cmake flex python3-dev \
        "gcc-${gcc_major}-plugin-dev" "libstdc++-${gcc_major}-dev"; \
    git clone https://github.com/AFLplusplus/AFLplusplus.git /tmp/aflplusplus; \
    git -C /tmp/aflplusplus checkout "${AFL_GIT_REF}"; \
    cd /tmp/aflplusplus; \
    export LLVM_CONFIG=llvm-config-18; \
    make source-only NO_NYX=1 -j"$(nproc)"; \
    make install NO_NYX=1; \
    cd /; \
    rm -rf /tmp/aflplusplus /var/lib/apt/lists/*; \
    afl-clang-fast --version; \
    afl-clang-lto --version; \
    afl-gcc-fast --version

ARG SYMCC_GIT_REF=master
ARG SYMCC_LLVM_VERSION=18
RUN set -eux; \
    apt-get update; \
    apt-get install -y --no-install-recommends \
        cmake ninja-build zlib1g-dev libzstd-dev; \
    git clone https://github.com/eurecom-s3/symcc.git /opt/symcc; \
    git -C /opt/symcc checkout "${SYMCC_GIT_REF}"; \
    git -C /opt/symcc submodule update --init runtime; \
    git -C /opt/symcc/runtime submodule update --init src/backends/qsym/qsym; \
    cmake -S /opt/symcc -B /opt/symcc/build -G Ninja \
        -DCMAKE_BUILD_TYPE=RelWithDebInfo \
        -DSYMCC_RT_BACKEND=qsym \
        -DZ3_TRUST_SYSTEM_VERSION=on \
        -DLLVM_DIR="/usr/lib/llvm-${SYMCC_LLVM_VERSION}/lib/cmake/llvm"; \
    cmake --build /opt/symcc/build; \
    ln -sf /opt/symcc/build/symcc /usr/local/bin/symcc; \
    ln -sf /opt/symcc/build/sym++ /usr/local/bin/sym++; \
    printf '%s\n' '#include <unistd.h>' \
        'int main() { int x; if (read(0, &x, sizeof x) != sizeof x) return 1;' \
        '                    if (2 * x < 7) { return 0; } return 1; }' \
        > /tmp/symcc_smoke.c; \
    symcc /tmp/symcc_smoke.c -o /tmp/symcc_smoke; \
    mkdir /tmp/symcc_smoke_out; \
    SYMCC_OUTPUT_DIR=/tmp/symcc_smoke_out sh -c 'echo aaaa | /tmp/symcc_smoke' || true; \
    test -n "$(ls -A /tmp/symcc_smoke_out)"; \
    rm -rf /tmp/symcc_smoke.c /tmp/symcc_smoke /tmp/symcc_smoke_out \
        /var/lib/apt/lists/*

ARG HONGGFUZZ_GIT_REF=cf8b66a4d09f4d4d786d96e3c46d9141fb4e98e2
RUN set -eux; \
    apt-get update; \
    apt-get install -y --no-install-recommends \
        binutils-dev libblocksruntime-dev libclang-rt-18-dev libunwind-dev; \
    git clone https://github.com/google/honggfuzz.git /tmp/honggfuzz; \
    git -C /tmp/honggfuzz checkout "${HONGGFUZZ_GIT_REF}"; \
    make -C /tmp/honggfuzz -j"$(nproc)"; \
    make -C /tmp/honggfuzz install PREFIX=/usr/local; \
    ln -sfn libhnetdriver /usr/local/include/libhfnetdriver; \
    printf '%s\n' '#include <stdint.h>' '#include <stdlib.h>' '#include <string.h>' \
        'int LLVMFuzzerTestOneInput(const uint8_t *d, size_t s) {' \
        '  if (s >= 4 && !memcmp(d, "AUST", 4)) abort();' \
        '  return 0; }' \
        > /tmp/hf_smoke.c; \
    hfuzz-clang /tmp/hf_smoke.c -o /tmp/hf_smoke; \
    mkdir /tmp/hf_smoke_in /tmp/hf_smoke_out; \
    printf 'x' > /tmp/hf_smoke_in/seed; \
    honggfuzz -q -n 1 --run_time 60 --exit_upon_crash \
        -i /tmp/hf_smoke_in -W /tmp/hf_smoke_out -- /tmp/hf_smoke > /dev/null 2>&1; \
    ls /tmp/hf_smoke_out | grep -q '^SIG'; \
    rm -rf /tmp/honggfuzz /tmp/hf_smoke.c /tmp/hf_smoke \
        /tmp/hf_smoke_in /tmp/hf_smoke_out /var/lib/apt/lists/*; \
    honggfuzz --help > /dev/null; \
    hfuzz-clang --version

ARG RADAMSA_VERSION=0.7
ARG RADAMSA_SOURCE_SHA=40d5dec416fb5277dbbd72c04b82ba2ae039778a
ARG RADAMSA_C_GZIP_SHA256=79d90cb6290bfeda51d42c049841788a5e9c0d74c7b7b0766f97394a5b0c0055
ARG RADAMSA_LICENCE_SHA256=4f31f940cb888ab4f6519774ee220f8c1b27871f8d278812b2572185e0013d96
ARG OWL_LICENCE_SHA256=9b8546be23fd7b6c022298e72e55d74a46acee15548196a4f8b2f86da32b80ed
RUN set -eux; \
    asset_url="https://gitlab.com/akihe/radamsa/uploads/d774a42f7893012d0a56c490a75ae12b/radamsa-${RADAMSA_VERSION}.c.gz"; \
    curl -fsSL -o /tmp/radamsa.c.gz "${asset_url}"; \
    echo "${RADAMSA_C_GZIP_SHA256}  /tmp/radamsa.c.gz" | sha256sum -c -; \
    gzip -dc /tmp/radamsa.c.gz | cc -O2 -x c -o /usr/local/bin/radamsa -; \
    install -d /usr/local/share/licenses/radamsa; \
    curl -fsSL -o /usr/local/share/licenses/radamsa/RADAMSA-LICENCE \
        "https://gitlab.com/akihe/radamsa/-/raw/${RADAMSA_SOURCE_SHA}/LICENCE"; \
    curl -fsSL -o /usr/local/share/licenses/radamsa/OWL-LICENCE \
        "https://gitlab.com/owl-lisp/owl/-/raw/2f7e001540485c77b5f744081e2f5f121fa7eb3a/LICENCE"; \
    echo "${RADAMSA_LICENCE_SHA256}  /usr/local/share/licenses/radamsa/RADAMSA-LICENCE" \
        | sha256sum -c -; \
    echo "${OWL_LICENCE_SHA256}  /usr/local/share/licenses/radamsa/OWL-LICENCE" \
        | sha256sum -c -; \
    { \
        printf '%s\n' \
            "Radamsa version: ${RADAMSA_VERSION}" \
            "Radamsa tag: v${RADAMSA_VERSION}" \
            "Radamsa source commit: ${RADAMSA_SOURCE_SHA}" \
            "Generated-C asset: ${asset_url}" \
            "Generated-C gzip SHA-256: ${RADAMSA_C_GZIP_SHA256}" \
            "Build compiler: $(cc --version | head -n 1)" \
            "Build platform: $(uname -m)-linux" \
            "Embedded Owl version/license commit: 0.2.2 / 2f7e001540485c77b5f744081e2f5f121fa7eb3a" \
            "hex extraction: https://gitlab.com/owl-lisp/hex/-/commit/68fd717471ef063910161ee3e03609a910ca1314" \
            "MIT-covered Owl codec source: https://gitlab.com/owl-lisp/owl/-/blob/a7f7df5ff958d68731dcd715a61878193a15ff63/owl/codec.scm"; \
    } > /usr/local/share/licenses/radamsa/PROVENANCE; \
    rm -f /tmp/radamsa.c.gz; \
    test "$(radamsa --version)" = "Radamsa ${RADAMSA_VERSION}"

RUN echo 'ubuntu ALL=(ALL) NOPASSWD:ALL' > /etc/sudoers.d/ubuntu \
    && chmod 0440 /etc/sudoers.d/ubuntu \
    && chsh -s /usr/bin/zsh ubuntu

USER ubuntu
WORKDIR /home/ubuntu

RUN mkdir -p /home/ubuntu/.codex /home/ubuntu/.claude /home/ubuntu/.history \
        /home/ubuntu/.config/gh

RUN sh -c "$(curl -fsSL https://raw.githubusercontent.com/ohmyzsh/ohmyzsh/master/tools/install.sh)" "" --unattended \
    && { echo 'export PATH="$HOME/.local/bin:$PATH"'; \
         echo 'export CLAUDE_CONFIG_DIR="$HOME/.claude"'; } \
        | tee -a /home/ubuntu/.bashrc >> /home/ubuntu/.zshrc

ARG OCAML_VERSION=5.2.0
RUN opam init --disable-sandboxing --shell-setup --yes --compiler=${OCAML_VERSION} \
    && echo 'test -r "$HOME/.opam/opam-init/init.sh" && . "$HOME/.opam/opam-init/init.sh" > /dev/null 2>&1 || true' \
        | tee -a /home/ubuntu/.bashrc >> /home/ubuntu/.zshrc \
    && echo '[[ ! -r "$HOME/.opam/opam-init/init.zsh" ]] || source "$HOME/.opam/opam-init/init.zsh" > /dev/null 2>&1' \
        >> /home/ubuntu/.zshenv

RUN { echo 'export HISTFILE="$HOME/.history/.zsh_history"'; \
         echo 'export HISTSIZE=50000'; \
         echo 'export SAVEHIST=10000'; } >> /home/ubuntu/.zshenv

ARG CN_GIT_REF=720f556d821284afe67e0a4842def65ada74f255
RUN git clone https://github.com/rems-project/cn.git /home/ubuntu/cn \
    && git -C /home/ubuntu/cn checkout "${CN_GIT_REF}"
WORKDIR /home/ubuntu/cn
RUN OPAMCONFIRMLEVEL=unsafe-yes opam install --deps-only --with-test ./cn.opam
RUN OPAMCONFIRMLEVEL=unsafe-yes opam install dune ocamlformat.0.27.0 ocaml-lsp-server
RUN eval "$(opam env)" && make install PROFILE=release && cn --version
WORKDIR /home/ubuntu

RUN curl --proto '=https' --tlsv1.2 -fsSL https://sh.rustup.rs \
    | sh -s -- -y --profile minimal --default-toolchain stable \
        --component rustfmt --component clippy --component rust-src \
    && mkdir -p /home/ubuntu/.cargo/registry

RUN CARGO_TARGET_DIR=/tmp/symcc_helper_target \
        /home/ubuntu/.cargo/bin/cargo install \
        --path /opt/symcc/util/symcc_fuzzing_helper \
        --root /tmp/symcc_helper \
    && sudo install -m 0755 /tmp/symcc_helper/bin/symcc_fuzzing_helper \
        /usr/local/bin/ \
    && rm -rf /tmp/symcc_helper /tmp/symcc_helper_target \
        /home/ubuntu/.cargo/registry \
    && mkdir -p /home/ubuntu/.cargo/registry \
    && symcc_fuzzing_helper --help > /dev/null

ARG HYPERFINE_VERSION=1.19.0
ARG SAMPLY_VERSION=0.13.1
RUN CARGO_TARGET_DIR=/tmp/bench_target \
        /home/ubuntu/.cargo/bin/cargo install --locked --root /tmp/bench \
        hyperfine --version ${HYPERFINE_VERSION} \
    && CARGO_TARGET_DIR=/tmp/bench_target \
        /home/ubuntu/.cargo/bin/cargo install --locked --root /tmp/bench \
        samply --version ${SAMPLY_VERSION} \
    && sudo install -m 0755 /tmp/bench/bin/hyperfine /tmp/bench/bin/samply \
        /usr/local/bin/ \
    && rm -rf /tmp/bench /tmp/bench_target \
        /home/ubuntu/.cargo/registry \
    && mkdir -p /home/ubuntu/.cargo/registry \
    && hyperfine --version > /dev/null \
    && samply --version > /dev/null

COPY --from=ghcr.io/astral-sh/uv:0.11.32 /uv /uvx /usr/local/bin/

ENV PATH=/home/ubuntu/.cargo/bin:/home/ubuntu/.opam/${OCAML_VERSION}/bin:$PATH
