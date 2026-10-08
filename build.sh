#!/usr/bin/env bash
# Builds MiniToo Studio inside a container and runs the tests; nothing is installed on the host.
# The binary lands in ./build/minitoo-studio. Cargo's download cache stays in ./docker/cargo.
#   ./build.sh            release build + tests
#   ./build.sh --windows  also type-check the Windows target (cross, no linking)
set -euo pipefail
cd "$(dirname "$0")"

tag="minitoo-rust-build:$(sha1sum docker/Dockerfile.build | cut -c1-12)"
docker image inspect "$tag" >/dev/null 2>&1 || docker build -t "$tag" -f docker/Dockerfile.build docker
mkdir -p docker/cargo build

windows=""
[ "${1:-}" = "--windows" ] && windows='cargo check --target x86_64-pc-windows-gnu --target-dir target/docker &&'

docker run --rm -u "$(id -u):$(id -g)" -e CARGO_HOME=/src/docker/cargo -v "$PWD":/src -w /src "$tag" bash -c "
    $windows
    cargo build --release --target-dir target/docker &&
    cargo test --release --lib --target-dir target/docker"
cp target/docker/release/minitoo-studio build/minitoo-studio
echo "built: $PWD/build/minitoo-studio"
