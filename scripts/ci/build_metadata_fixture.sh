#!/usr/bin/env bash
set -euo pipefail

export GIT_CONFIG_NOSYSTEM=1
export GIT_CONFIG_GLOBAL=/dev/null

fixture_root=$(mktemp -d "${TMPDIR:-/tmp}/sagascript-build-metadata.XXXXXX")
fixture="$fixture_root/src-tauri/crates/sagascript-cli"
evidence_root=$(mktemp -d "${TMPDIR:-/tmp}/sagascript-build-metadata-evidence.XXXXXX")
trap 'rm -rf "$fixture_root" "$evidence_root"' EXIT
mkdir -p "$fixture/src"
mkdir -p "$fixture_root/src-tauri/crates/sagascript-core/src"
: > "$fixture_root/src-tauri/Cargo.lock"
: > "$fixture_root/src-tauri/crates/sagascript-core/Cargo.toml"
cp src-tauri/crates/sagascript-cli/build.rs "$fixture/build.rs"
cat > "$fixture/Cargo.toml" <<'EOF'
[package]
name = "build-metadata-fixture"
version = "0.1.0"
edition = "2021"
build = "build.rs"
EOF
: > "$fixture/src/lib.rs"
cat > "$fixture/src/main.rs" <<'EOF'
fn main() {
    println!("{} {}", env!("SAGASCRIPT_CLI_GIT_HASH"), env!("SAGASCRIPT_CLI_BUILD_DATE"));
}
EOF
cat > "$fixture/.gitignore" <<'EOF'
/target/
/Cargo.lock
EOF

git -C "$fixture" init -q
git -C "$fixture" config user.email fixture@example.invalid
git -C "$fixture" config user.name fixture
git -C "$fixture" config core.hooksPath /dev/null
git -C "$fixture" add .
git -C "$fixture" commit -qm initial
rm -f "$fixture/.git/packed-refs"

build_count() {
    local log=$1
    shift
    local elapsed_ms
    (cd "$fixture" && env -u SAGASCRIPT_GIT_HASH -u SAGASCRIPT_BUILD_DATE \
        GIT_CONFIG_NOSYSTEM=1 GIT_CONFIG_GLOBAL=/dev/null \
        CARGO_TARGET_DIR="$fixture/target" "$@" /usr/bin/time -p cargo build --offline -vv) > "$log" 2>&1
    local count
    count=$(rg -c 'Running `.*build-script-build' "$log" || true)
    elapsed_ms=$(awk '$1 == "real" { printf "%.0f", $2 * 1000 }' "$log")
    elapsed_ms=${elapsed_ms:-0}
    printf 'elapsed_ms=%s count=%s\n' "$elapsed_ms" "${count:-0}" > "${log%.log}.meta"
    (cd "$fixture" && ./target/debug/build-metadata-fixture) > "${log%.log}.identity"
    printf '%s\n' "${count:-0}"
}

assert_eq() {
    if [[ "$1" != "$2" ]]; then
        printf 'expected %s, got %s (%s)\n' "$2" "$1" "$3" >&2
        exit 1
    fi
}

clean_fixture() {
    (cd "$fixture" && CARGO_TARGET_DIR="$fixture/target" cargo clean >/dev/null)
}

assert_text() {
    if [[ "$1" != "$2" ]]; then
        printf 'expected %s, got %s (%s)\n' "$2" "$1" "$3" >&2
        exit 1
    fi
}

# The patched build keeps env invalidation but avoids nonexistent Git paths
# when CI supplied both identity values.
clean_fixture
ci_first=$(build_count "$evidence_root/ci-1.log" SAGASCRIPT_GIT_HASH=ci-hash SAGASCRIPT_BUILD_DATE=2026-09-08)
ci_same=$(build_count "$evidence_root/ci-2.log" SAGASCRIPT_GIT_HASH=ci-hash SAGASCRIPT_BUILD_DATE=2026-09-08)
ci_changed=$(build_count "$evidence_root/ci-3.log" SAGASCRIPT_GIT_HASH=ci-hash-2 SAGASCRIPT_BUILD_DATE=2026-09-08)
ci_date_changed=$(build_count "$evidence_root/ci-date.log" SAGASCRIPT_GIT_HASH=ci-hash-2 SAGASCRIPT_BUILD_DATE=2026-09-09)
assert_eq "$ci_first" 1 "first CI metadata build"
assert_eq "$ci_same" 0 "unchanged CI metadata build"
assert_eq "$ci_changed" 1 "changed CI metadata build"
assert_eq "$ci_date_changed" 1 "changed CI build date"
assert_text "$(cat "$evidence_root/ci-1.identity")" "ci-hash 2026-09-08" "CI identity output"
assert_text "$(cat "$evidence_root/ci-3.identity")" "ci-hash-2 2026-09-08" "changed CI identity output"
assert_text "$(cat "$evidence_root/ci-date.identity")" "ci-hash-2 2026-09-09" "changed CI date identity"

# A hosted checkout without packed-refs still reproduces the old perpetual
# rebuild for local fallback identity.
clean_fixture
first=$(build_count "$evidence_root/absent-1.log")
second=$(build_count "$evidence_root/absent-2.log")
assert_eq "$first" 1 "initial fallback build"
assert_eq "$second" 1 "fallback build with missing packed-refs"

# With all watched Git files present, an unchanged local build is clean.
git -C "$fixture" pack-refs --all --no-prune
clean_fixture
refs_first=$(build_count "$evidence_root/refs-1.log")
refs_same=$(build_count "$evidence_root/refs-2.log")
assert_eq "$refs_first" 1 "build after creating packed-refs"
assert_eq "$refs_same" 0 "unchanged build with packed-refs"

# Partial metadata still uses local Git fallback and therefore keeps the
# watches. Recreate the hosted missing-ref shape to verify that fallback
# remains conservative even when one CI value is supplied.
rm -f "$fixture/.git/packed-refs"
clean_fixture
partial_first=$(build_count "$evidence_root/partial-missing-1.log" SAGASCRIPT_GIT_HASH=ci-hash)
partial_same=$(build_count "$evidence_root/partial-missing-2.log" SAGASCRIPT_GIT_HASH=ci-hash)
assert_eq "$partial_first" 1 "first partial metadata build"
assert_eq "$partial_same" 1 "partial metadata fallback build"
partial_date_missing_first=$(build_count "$evidence_root/partial-date-missing-1.log" SAGASCRIPT_BUILD_DATE=2026-09-08)
partial_date_missing_same=$(build_count "$evidence_root/partial-date-missing-2.log" SAGASCRIPT_BUILD_DATE=2026-09-08)
assert_eq "$partial_date_missing_first" 1 "first date-only metadata build"
assert_eq "$partial_date_missing_same" 1 "date-only metadata fallback build"
initial_hash=$(git -C "$fixture" rev-parse --short HEAD)
initial_date=$(git -C "$fixture" show -s --format=%cs HEAD)
assert_text "$(cat "$evidence_root/partial-missing-1.identity")" "ci-hash $initial_date" "hash-only fallback identity"
assert_text "$(cat "$evidence_root/partial-date-missing-1.identity")" "$initial_hash 2026-09-08" "date-only fallback identity"

# With present Git metadata, partial CI values still use local fallback and
# an unchanged build is clean.
git -C "$fixture" pack-refs --all --no-prune
clean_fixture
partial_present_first=$(build_count "$evidence_root/partial-present-1.log" SAGASCRIPT_GIT_HASH=ci-hash)
partial_present_same=$(build_count "$evidence_root/partial-present-2.log" SAGASCRIPT_GIT_HASH=ci-hash)
assert_eq "$partial_present_first" 1 "first partial metadata build with refs"
assert_eq "$partial_present_same" 0 "unchanged partial metadata build with refs"
partial_date_present_first=$(build_count "$evidence_root/partial-date-present-1.log" SAGASCRIPT_BUILD_DATE=2026-09-08)
partial_date_present_same=$(build_count "$evidence_root/partial-date-present-2.log" SAGASCRIPT_BUILD_DATE=2026-09-08)
assert_eq "$partial_date_present_first" 1 "first date-only metadata build with refs"
assert_eq "$partial_date_present_same" 0 "unchanged date-only metadata build with refs"
assert_text "$(cat "$evidence_root/partial-present-1.identity")" "ci-hash $initial_date" "hash-only identity with refs"
assert_text "$(cat "$evidence_root/partial-date-present-1.identity")" "$initial_hash 2026-09-08" "date-only identity with refs"

# Establish a clean local warm build before changing source, so the source
# case below measures the source watcher rather than a cold target.
clean_fixture
local_warm_first=$(build_count "$evidence_root/local-warm-1.log")
local_warm_same=$(build_count "$evidence_root/local-warm-2.log")
assert_eq "$local_warm_first" 1 "local warm baseline build"
assert_eq "$local_warm_same" 0 "local unchanged warm build"

printf '// source change\n' >> "$fixture/src/lib.rs"
source_changed=$(build_count "$evidence_root/source.log")
assert_eq "$source_changed" 1 "source change"
expected_hash=$(git -C "$fixture" rev-parse --short HEAD)
expected_date=$(git -C "$fixture" show -s --format=%cs HEAD)
assert_text "$(cat "$evidence_root/source.identity")" "$expected_hash-dirty $expected_date" "dirty source identity"

# Complete CI identity must still rebuild for a source change while retaining
# the explicit identity values.
ci_source_warm=$(build_count "$evidence_root/ci-source-warm.log" SAGASCRIPT_GIT_HASH=ci-hash SAGASCRIPT_BUILD_DATE=2026-09-08)
ci_source_same=$(build_count "$evidence_root/ci-source-same.log" SAGASCRIPT_GIT_HASH=ci-hash SAGASCRIPT_BUILD_DATE=2026-09-08)
assert_eq "$ci_source_same" 0 "complete CI source baseline is fresh"
printf '// complete CI source change\n' >> "$fixture/src/lib.rs"
ci_source_changed=$(build_count "$evidence_root/ci-source.log" SAGASCRIPT_GIT_HASH=ci-hash SAGASCRIPT_BUILD_DATE=2026-09-08)
assert_eq "$ci_source_changed" 1 "source change with complete CI metadata"
assert_text "$(cat "$evidence_root/ci-source.identity")" "ci-hash 2026-09-08" "complete CI source identity"

local_before_commit=$(build_count "$evidence_root/local-before-commit.log")
local_before_commit_same=$(build_count "$evidence_root/local-before-commit-same.log")
assert_eq "$local_before_commit_same" 0 "local dirty source baseline is fresh"
git -C "$fixture" add src/lib.rs
git -C "$fixture" commit -qm source-change
commit_changed=$(build_count "$evidence_root/commit.log")
assert_eq "$commit_changed" 1 "commit change"
new_hash=$(git -C "$fixture" rev-parse --short HEAD)
new_date=$(git -C "$fixture" show -s --format=%cs HEAD)
assert_text "$(cat "$evidence_root/commit.identity")" "$new_hash $new_date" "committed source identity"
git -C "$fixture" checkout -qb fixture-branch
branch_changed=$(build_count "$evidence_root/branch.log")
assert_eq "$branch_changed" 1 "branch change"
assert_text "$(cat "$evidence_root/branch.identity")" "$new_hash $new_date" "branch source identity"

printf 'build metadata fixture passed: missing/present refs, complete/partial CI metadata, source, branch, and commit cases\n'
for log in ci-1 ci-2 ci-3 ci-date absent-1 absent-2 refs-1 refs-2 partial-missing-1 partial-missing-2 partial-date-missing-1 partial-date-missing-2 partial-present-1 partial-present-2 partial-date-present-1 partial-date-present-2 local-warm-1 local-warm-2 source ci-source commit branch; do
    printf '%s: ' "$log"
    cat "$evidence_root/$log.meta"
done
