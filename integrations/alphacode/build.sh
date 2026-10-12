#!/usr/bin/env bash
set -euo pipefail
root="$(realpath "$(dirname "$0")/../..")"
source_dir="${1:-$root/.sources/alphacode}"
mode="${2:-patched}"
commit=2a1d17156282e59fa811ceb58d9e28302d360fba
patch="$root/integrations/alphacode/patches/0001-ironbark-permission-boundary.patch"
[[ "$(git -C "$source_dir" rev-parse HEAD)" == "$commit" ]] || { echo 'Wrong AlphaCode source revision' >&2; exit 2; }
case "$mode" in
  pristine) git -C "$source_dir" diff --exit-code && git -C "$source_dir" apply --check "$patch" && git -C "$source_dir" apply "$patch" ;;
  patched) git -C "$source_dir" apply --reverse --check "$patch" ;;
  *) echo 'Mode must be pristine or patched' >&2; exit 2 ;;
esac
# Include new patch files in the source comparison without staging their content.
git -C "$source_dir" add --intent-to-add -- src/alphacode_app_core/ironbark_permissions.rs src/alphacode_app_core/tool/ironbark_permission_tests.rs src/cli/ironbark_managed.rs
cmp "$patch" <(git -C "$source_dir" -c core.autocrlf=false diff --binary --no-ext-diff) || { echo 'Unrecorded native source changes; regenerate/requalify provenance' >&2; exit 2; }
# This is a source/recipe identity, NOT the resulting executable checksum.
build_digest="$( { printf '%s' "$commit"; cat "$patch" "$root/integrations/alphacode/build.sh" "$source_dir/Cargo.lock" "$source_dir/rust-toolchain.toml"; } | sha256sum | cut -d ' ' -f 1 )"
export IRONBARK_ALPHACODE_BUILD_DIGEST="$build_digest"
export ALPHACODE_NO_TELEMETRY=1
export CARGO_TARGET_DIR="$root/qa/private/alphacode-target"
# A preinstalled pinned toolchain is required. No toolchain/package installation,
# extension activation or execution of the compiled AlphaCode occurs here.
(cd "$source_dir" && cargo +1.94.1 build --locked --release --no-default-features --bin alphacode)
printf 'source_recipe_digest=%s\n' "$build_digest"
sha256sum "$CARGO_TARGET_DIR/release/alphacode"
