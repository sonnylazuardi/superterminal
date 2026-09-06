# superterminal task runner. See docs/plan/07-milestones.md for task ids.
set shell := ["bash", "-uc"]

default:
    @just --list

# --- build ---------------------------------------------------------------
build:                      # build all non-GPU crates
    cargo build --workspace

build-native:               # build the gpuix-backed native module (needs GPU toolchain)
    #!/usr/bin/env bash
    set -euo pipefail
    source scripts/env.sh
    # `+$ST_NATIVE_TOOLCHAIN` is required: rustup picks rust-toolchain.toml by
    # INVOCATION directory, and the nearest file to crates/st-native is the
    # root's `stable` — vendor/gpuix is not an ancestor of it.
    (cd crates/st-native && cargo "+$ST_NATIVE_TOOLCHAIN" build --release)
    mkdir -p packages/native
    for ext in dylib so; do
        src="crates/st-native/target/release/libst_native.$ext"
        [ -f "$src" ] && cp "$src" "packages/native/superterminal-native.$ST_TRIPLE.node" && break
    done
    ls -l "packages/native/superterminal-native.$ST_TRIPLE.node"

vendor-patch:               # apply every gpuix patch, idempotently
    #!/usr/bin/env bash
    set -euo pipefail
    cd vendor/gpuix
    for p in ../../patches/*.patch; do
        if git apply --reverse --check "$p" 2>/dev/null; then
            echo "already applied: $(basename "$p")"
        elif git apply --check "$p" 2>/dev/null; then
            git apply "$p" && echo "applied: $(basename "$p")"
        else
            echo "ERROR: $(basename "$p") does not apply cleanly (re-pin or refresh it)" >&2
            exit 1
        fi
    done

clean-vendor:
    cd vendor/gpuix && git checkout . && git clean -fd

# --- run -----------------------------------------------------------------
server *ARGS:               # run the daemon in the foreground
    cargo run -p st-server -- --foreground {{ARGS}}

cli *ARGS:
    cargo run -p st-cli -- {{ARGS}}

dev:                        # run the GUI client
    # NOT `bun --hot`: re-evaluated component modules bind a second copy of
    # React while globalThis.__stRoot still holds the old reconciler, so the
    # first edit blanks the window with "Invalid hook call". Restarting is cheap
    # — the terminals live in the daemon (invariant I1).
    bun packages/app/src/app.tsx

# --- quality -------------------------------------------------------------
test:
    cargo test --workspace
    bun test

fmt:
    cargo fmt --all
    bun x prettier --write "packages/**/*.{ts,tsx,json}"

lint:
    cargo clippy --workspace --all-targets -- -D warnings
    bun run typecheck

check: fmt lint test
