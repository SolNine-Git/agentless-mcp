#!/usr/bin/env bash
# Build the Linux wheel inside a pypa manylinux image, for the policy that the image names. It installs
# rustup by hash, the toolchain that rust-toolchain.toml names, and maturin by hash, then writes the
# wheel to the directory in $1.
set -euo pipefail

out="$1"
root="$(cd "$(dirname "$0")/.." && pwd)"
rustup_version=1.29.1
arch="$(uname -m)"
case "$arch" in
    x86_64) rustup_sha256=dda7234360b7f578ca8b0ddcb80145646fa61a67c1720a5abc7051b35c9fcb71 ;;
    aarch64) rustup_sha256=15f6e4ce9f583b929c996c91562bad6d4454f3281de858b02cdfdef615fac433 ;;
    *)
        echo "manylinux.sh: no rustup-init hash for $arch" >&2
        exit 1
        ;;
esac

work="$(mktemp -d)"
curl --proto '=https' --tlsv1.2 --fail --silent --show-error --max-time 300 \
    --output "$work/rustup-init" \
    "https://static.rust-lang.org/rustup/archive/$rustup_version/$arch-unknown-linux-gnu/rustup-init"
echo "$rustup_sha256  $work/rustup-init" | sha256sum --check --strict -
chmod +x "$work/rustup-init"
"$work/rustup-init" -y --quiet --profile minimal --default-toolchain none --no-modify-path
export PATH="$HOME/.cargo/bin:$PATH"

/opt/python/cp313-cp313/bin/python -m venv "$work/build"
"$work/build/bin/pip" install --quiet --no-deps --require-hashes \
    --requirement "$root/packaging/build-requirements.txt"

cd "$root"
rustup toolchain install --no-self-update
cd "$root/packaging"
"$work/build/bin/maturin" build --release --locked --compatibility "$AUDITWHEEL_POLICY" --out "$out"
