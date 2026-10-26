#!/usr/bin/env bash
set -euo pipefail

export GIT_CONFIG_NOSYSTEM=1
export GIT_CONFIG_GLOBAL=/dev/null

fixture_root=$(mktemp -d "${TMPDIR:-/tmp}/sagascript-build-metadata.XXXXXX")
fixture="$fixture_root/src-tauri/crates/sagascript-cli"
evidence_root=$(mktemp -d "${TMPDIR:-/tmp}/sagascript-build-metadata-evidence.XXXXXX")
archive_root=$(mktemp -d "${TMPDIR:-/tmp}/sagascript-build-metadata-archive.XXXXXX")
trap 'rm -rf "$fixture_root" "$evidence_root" "$archive_root"' EXIT
mkdir -p "$fixture/src" \
    "$fixture_root/src-tauri/src" \
    "$fixture_root/src-tauri/crates/sagascript-core/src" \
    "$fixture_root/src-tauri/crates/sagascript-engine-protocol/src" \
    "$fixture_root/src-tauri/crates/sagascript-testsupport/src" \
    "$fixture_root/src-tauri/gen/schemas" \
    "$fixture_root/src" \
    "$fixture_root/scripts"
cp src-tauri/crates/sagascript-cli/build.rs "$fixture/build.rs"
cp src-tauri/build_identity.rs "$fixture_root/src-tauri/build_identity.rs"
cp scripts/build-identity-generated-paths.txt "$fixture_root/scripts/build-identity-generated-paths.txt"
cat > "$fixture_root/src-tauri/Cargo.toml" <<'EOF'
[workspace]
members = ["crates/*"]
resolver = "2"
EOF
cat > "$fixture_root/src-tauri/crates/sagascript-core/Cargo.toml" <<'EOF'
[package]
name = "sagascript-core"
version = "0.1.0"
edition = "2021"
EOF
cat > "$fixture_root/src-tauri/crates/sagascript-engine-protocol/Cargo.toml" <<'EOF'
[package]
name = "sagascript-engine-protocol"
version = "0.1.0"
edition = "2021"
EOF
cat > "$fixture_root/src-tauri/crates/sagascript-testsupport/Cargo.toml" <<'EOF'
[package]
name = "sagascript-testsupport"
version = "0.1.0"
edition = "2021"
EOF
cat > "$fixture_root/src-tauri/crates/sagascript-core/src/lib.rs" <<'EOF'
pub fn fixture_value() -> u8 { 1 }
EOF
cat > "$fixture_root/src-tauri/crates/sagascript-engine-protocol/src/lib.rs" <<'EOF'
pub fn fixture_value() -> u8 { 1 }
EOF
cat > "$fixture_root/src-tauri/crates/sagascript-testsupport/src/lib.rs" <<'EOF'
pub fn fixture_value() -> u8 { 1 }
EOF
cat > "$fixture_root/src-tauri/src/lib.rs" <<'EOF'
pub fn desktop_fixture_value() -> u8 { 1 }
EOF
cat > "$fixture_root/src/main.ts" <<'EOF'
export const frontendFixtureValue = 1;
EOF
: > "$fixture_root/package-lock.json"
cat > "$fixture_root/src-tauri/gen/schemas/windows-schema.json" <<'EOF'
{"schema": 1}
EOF
cat > "$fixture_root/.gitignore" <<'EOF'
/src-tauri/Cargo.lock
/src-tauri/target/
EOF
cat > "$fixture/Cargo.toml" <<'EOF'
[package]
name = "build-metadata-fixture"
version = "0.1.0"
edition = "2021"
build = "build.rs"

[dependencies]
sagascript-core = { path = "../sagascript-core" }
sagascript-engine-protocol = { path = "../sagascript-engine-protocol" }
sagascript-testsupport = { path = "../sagascript-testsupport" }
EOF
: > "$fixture/src/lib.rs"
cat > "$fixture/src/main.rs" <<'EOF'
fn main() {
    println!("{} {} {} {}",
        env!("SAGASCRIPT_CLI_GIT_HASH"), env!("SAGASCRIPT_CLI_BUILD_DATE"),
        env!("SAGASCRIPT_SOURCE_REVISION"), env!("SAGASCRIPT_SOURCE_DIRTY"));
}
EOF
cat > "$fixture/.gitignore" <<'EOF'
/target/
/Cargo.lock
EOF

(cd "$fixture" && cargo generate-lockfile --offline)

git -C "$fixture_root" init -q
git -C "$fixture_root" config user.email fixture@example.invalid
git -C "$fixture_root" config user.name fixture
git -C "$fixture_root" config core.hooksPath /dev/null
git -C "$fixture_root" add .
git -C "$fixture_root" commit -qm initial
rm -f "$fixture_root/.git/packed-refs"

build_count() {
    local log=$1
    shift
    local elapsed_ms
    (cd "$fixture" && env -u SAGASCRIPT_GIT_HASH -u SAGASCRIPT_BUILD_DATE \
        -u SAGASCRIPT_SOURCE_REVISION_INPUT -u SAGASCRIPT_SOURCE_DIRTY_INPUT \
        GIT_OPTIONAL_LOCKS=0 \
        GIT_CONFIG_NOSYSTEM=1 GIT_CONFIG_GLOBAL=/dev/null \
        CARGO_TARGET_DIR="$fixture_root/src-tauri/target" "$@" /usr/bin/time -p cargo build --offline -vv) > "$log" 2>&1
    local count
    count=$(rg -c 'Running `.*build-script-build' "$log" || true)
    elapsed_ms=$(awk '$1 == "real" { printf "%.0f", $2 * 1000 }' "$log")
    elapsed_ms=${elapsed_ms:-0}
    printf 'elapsed_ms=%s count=%s\n' "$elapsed_ms" "${count:-0}" > "${log%.log}.meta"
    (cd "$fixture" && "$fixture_root/src-tauri/target/debug/build-metadata-fixture") > "${log%.log}.identity"
    printf '%s\n' "${count:-0}"
}

assert_eq() {
    if [[ "$1" != "$2" ]]; then
        printf 'expected %s, got %s (%s)\n' "$2" "$1" "$3" >&2
        exit 1
    fi
}

clean_fixture() {
    (cd "$fixture" && CARGO_TARGET_DIR="$fixture_root/src-tauri/target" cargo clean >/dev/null)
}

refresh_git_status() {
    git -C "$fixture_root" status --porcelain=v1 --untracked-files=all >/dev/null
    git -C "$fixture_root" status --porcelain=v1 --untracked-files=all >/dev/null
}

assert_text() {
    if [[ "$1" != "$2" ]]; then
        printf 'expected %s, got %s (%s)\n' "$2" "$1" "$3" >&2
        git -C "$fixture_root" status --porcelain=v1 --untracked-files=all >&2 || true
        exit 1
    fi
}

# The patched build keeps env invalidation but avoids nonexistent Git paths
# when CI supplied both identity values.
clean_fixture
fixture_sha=$(git -C "$fixture_root" rev-parse HEAD)
ci_first=$(build_count "$evidence_root/ci-1.log" SAGASCRIPT_GIT_HASH=ci-hash SAGASCRIPT_BUILD_DATE=2026-09-08 SAGASCRIPT_SOURCE_REVISION_INPUT="$fixture_sha" SAGASCRIPT_SOURCE_DIRTY_INPUT=false)
ci_same=$(build_count "$evidence_root/ci-2.log" SAGASCRIPT_GIT_HASH=ci-hash SAGASCRIPT_BUILD_DATE=2026-09-08 SAGASCRIPT_SOURCE_REVISION_INPUT="$fixture_sha" SAGASCRIPT_SOURCE_DIRTY_INPUT=false)
ci_changed=$(build_count "$evidence_root/ci-3.log" SAGASCRIPT_GIT_HASH=ci-hash-2 SAGASCRIPT_BUILD_DATE=2026-09-08 SAGASCRIPT_SOURCE_REVISION_INPUT="$fixture_sha" SAGASCRIPT_SOURCE_DIRTY_INPUT=false)
ci_date_changed=$(build_count "$evidence_root/ci-date.log" SAGASCRIPT_GIT_HASH=ci-hash-2 SAGASCRIPT_BUILD_DATE=2026-09-09 SAGASCRIPT_SOURCE_REVISION_INPUT="$fixture_sha" SAGASCRIPT_SOURCE_DIRTY_INPUT=false)
assert_eq "$ci_first" 1 "first CI metadata build"
assert_eq "$ci_same" 0 "unchanged CI metadata build"
assert_eq "$ci_changed" 1 "changed CI metadata build"
assert_eq "$ci_date_changed" 1 "changed CI build date"
assert_text "$(cat "$evidence_root/ci-1.identity")" "ci-hash 2026-09-08 $fixture_sha false" "CI identity output"
assert_text "$(cat "$evidence_root/ci-3.identity")" "ci-hash-2 2026-09-08 $fixture_sha false" "changed CI identity output"
assert_text "$(cat "$evidence_root/ci-date.identity")" "ci-hash-2 2026-09-09 $fixture_sha false" "changed CI date identity"

# Explicit malformed source inputs are unknown, never replaced by Git fallback.
invalid_revision=$(build_count "$evidence_root/invalid-revision.log" SAGASCRIPT_GIT_HASH=ci-hash SAGASCRIPT_BUILD_DATE=2026-09-08 SAGASCRIPT_SOURCE_REVISION_INPUT=short SAGASCRIPT_SOURCE_DIRTY_INPUT=false)
assert_eq "$invalid_revision" 1 "invalid source revision metadata"
assert_text "$(cat "$evidence_root/invalid-revision.identity")" "ci-hash 2026-09-08 unknown false" "invalid revision is unknown"
invalid_dirty=$(build_count "$evidence_root/invalid-dirty.log" SAGASCRIPT_GIT_HASH=ci-hash SAGASCRIPT_BUILD_DATE=2026-09-08 SAGASCRIPT_SOURCE_REVISION_INPUT="$fixture_sha" SAGASCRIPT_SOURCE_DIRTY_INPUT=clean)
assert_eq "$invalid_dirty" 1 "invalid source dirty metadata"
assert_text "$(cat "$evidence_root/invalid-dirty.identity")" "ci-hash 2026-09-08 $fixture_sha unknown" "invalid dirty state is unknown"

# Hosted-style checkouts without packed-refs retain the existing conservative
# Git watch behavior. Cache invalidation cases below use all watched refs.
refresh_git_status
clean_fixture
first=$(build_count "$evidence_root/absent-1.log")
second=$(build_count "$evidence_root/absent-2.log")
assert_eq "$first" 1 "initial fallback build"
assert_eq "$second" 1 "fallback build with missing packed-refs"

# With all watched Git files present, an unchanged local build is clean.
git -C "$fixture_root" pack-refs --all --no-prune
refresh_git_status
clean_fixture
refs_first=$(build_count "$evidence_root/refs-1.log")
refs_same=$(build_count "$evidence_root/refs-2.log")
assert_eq "$refs_first" 1 "build after creating packed-refs"
if [[ "$refs_same" != 0 ]]; then
    refs_same=$(build_count "$evidence_root/refs-3.log")
fi
assert_eq "$refs_same" 0 "unchanged build with packed-refs"

# Partial metadata still uses local Git fallback and therefore keeps the
# watches. Recreate the hosted missing-ref shape to verify that fallback
# remains conservative even when one CI value is supplied.
rm -f "$fixture_root/.git/packed-refs"
refresh_git_status
clean_fixture
partial_first=$(build_count "$evidence_root/partial-missing-1.log" SAGASCRIPT_GIT_HASH=ci-hash)
partial_same=$(build_count "$evidence_root/partial-missing-2.log" SAGASCRIPT_GIT_HASH=ci-hash)
assert_eq "$partial_first" 1 "first partial metadata build"
assert_eq "$partial_same" 1 "partial metadata fallback build with missing packed-refs"
partial_date_missing_first=$(build_count "$evidence_root/partial-date-missing-1.log" SAGASCRIPT_BUILD_DATE=2026-09-08)
partial_date_missing_same=$(build_count "$evidence_root/partial-date-missing-2.log" SAGASCRIPT_BUILD_DATE=2026-09-08)
assert_eq "$partial_date_missing_first" 1 "first date-only metadata build"
assert_eq "$partial_date_missing_same" 1 "date-only metadata fallback build with missing packed-refs"
initial_hash=$(git -C "$fixture_root" rev-parse --short HEAD)
initial_date=$(git -C "$fixture_root" show -s --format=%cs HEAD)
assert_text "$(cat "$evidence_root/partial-missing-1.identity")" "ci-hash $initial_date $fixture_sha false" "hash-only fallback identity"
assert_text "$(cat "$evidence_root/partial-date-missing-1.identity")" "$initial_hash 2026-09-08 $fixture_sha false" "date-only fallback identity"

# With present Git metadata, partial CI values still use local fallback and
# an unchanged build is clean.
git -C "$fixture_root" pack-refs --all --no-prune
refresh_git_status
clean_fixture
partial_present_first=$(build_count "$evidence_root/partial-present-1.log" SAGASCRIPT_GIT_HASH=ci-hash)
partial_present_same=$(build_count "$evidence_root/partial-present-2.log" SAGASCRIPT_GIT_HASH=ci-hash)
assert_eq "$partial_present_first" 1 "first partial metadata build with refs"
assert_eq "$partial_present_same" 0 "unchanged partial metadata build with refs"
partial_date_present_first=$(build_count "$evidence_root/partial-date-present-1.log" SAGASCRIPT_BUILD_DATE=2026-09-08)
partial_date_present_same=$(build_count "$evidence_root/partial-date-present-2.log" SAGASCRIPT_BUILD_DATE=2026-09-08)
assert_eq "$partial_date_present_first" 1 "first date-only metadata build with refs"
assert_eq "$partial_date_present_same" 0 "unchanged date-only metadata build with refs"
assert_text "$(cat "$evidence_root/partial-present-1.identity")" "ci-hash $initial_date $fixture_sha false" "hash-only identity with refs"
assert_text "$(cat "$evidence_root/partial-date-present-1.identity")" "$initial_hash 2026-09-08 $fixture_sha false" "date-only identity with refs"

# Establish a clean local warm build before changing source, so the source
# case below measures the source watcher rather than a cold target or an absent
# Git ref. Keep the loose current branch ref and packed-refs present.
git -C "$fixture_root" pack-refs --all --no-prune
clean_fixture
local_warm_first=$(build_count "$evidence_root/local-warm-1.log")
local_warm_same=$(build_count "$evidence_root/local-warm-2.log")
assert_eq "$local_warm_first" 1 "local warm baseline build"
assert_eq "$local_warm_same" 0 "local unchanged warm build"

# Changes to the shared helper and generated-path policy must invalidate the
# cached build script. The new policy entry is committed in the fixture so the
# later untracked generated file is the only worktree change.
echo '// helper invalidation fixture' >> "$fixture_root/src-tauri/build_identity.rs"
helper_changed=$(build_count "$evidence_root/helper-changed.log")
assert_eq "$helper_changed" 1 "shared helper change"
git -C "$fixture_root" checkout -- src-tauri/build_identity.rs
helper_restored=$(build_count "$evidence_root/helper-restored.log")
assert_eq "$helper_restored" 1 "shared helper restore"
echo 'src-tauri/gen/schemas/custom-fixture-schema.json' >> "$fixture_root/scripts/build-identity-generated-paths.txt"
git -C "$fixture_root" add scripts/build-identity-generated-paths.txt
git -C "$fixture_root" commit -qm 'fixture: update generated path policy'
echo '{"schema": 2}' > "$fixture_root/src-tauri/gen/schemas/custom-fixture-schema.json"
policy_changed=$(build_count "$evidence_root/policy-changed.log")
assert_eq "$policy_changed" 1 "generated path policy change"
fixture_sha=$(git -C "$fixture_root" rev-parse HEAD)
policy_hash=$(git -C "$fixture_root" rev-parse --short HEAD)
policy_date=$(git -C "$fixture_root" show -s --format=%cs HEAD)
assert_text "$(cat "$evidence_root/policy-changed.identity")" "$policy_hash $policy_date $fixture_sha false" "new generated path policy is loaded"

# Existing generated outputs may change and be newly created without making
# the fallback identity dirty. The helper requires every listed path to be a
# regular file and still treats all other worktree changes as dirty.
echo '{"schema": 3}' >> "$fixture_root/src-tauri/gen/schemas/windows-schema.json"
echo '{"schema": 4}' > "$fixture_root/src-tauri/gen/schemas/acl-manifests.json"
clean_fixture
generated_only=$(build_count "$evidence_root/generated-only.log")
assert_eq "$generated_only" 1 "generated-only fallback identity"
assert_text "$(cat "$evidence_root/generated-only.identity")" "$policy_hash $policy_date $fixture_sha false" "generated-only changes stay clean"

echo 'unrelated untracked data' > "$fixture_root/unrelated untracked.txt"
clean_fixture
unrelated_dirty=$(build_count "$evidence_root/unrelated-untracked.log")
assert_eq "$unrelated_dirty" 1 "unrelated untracked file"
assert_text "$(cat "$evidence_root/unrelated-untracked.identity")" "$policy_hash-dirty $policy_date $fixture_sha true" "unrelated untracked file is dirty"
rm "$fixture_root/unrelated untracked.txt"

# A staged rename of an allowed generated path is never treated as generated-only.
git -C "$fixture_root" mv src-tauri/gen/schemas/windows-schema.json 'src-tauri/gen/schemas/renamed schema.json'
git -C "$fixture_root" add -- 'src-tauri/gen/schemas/renamed schema.json'
renamed_generated=$(build_count "$evidence_root/renamed-generated.log")
assert_eq "$renamed_generated" 1 "renamed generated path"
assert_text "$(cat "$evidence_root/renamed-generated.identity")" "$policy_hash-dirty $policy_date $fixture_sha true" "renames fail closed"

printf '// source change\n' >> "$fixture/src/lib.rs"
source_changed=$(build_count "$evidence_root/source.log")
assert_eq "$source_changed" 1 "source change"
expected_hash=$(git -C "$fixture_root" rev-parse --short HEAD)
expected_date=$(git -C "$fixture_root" show -s --format=%cs HEAD)
assert_text "$(cat "$evidence_root/source.identity")" "$expected_hash-dirty $expected_date $fixture_sha true" "dirty source identity"

# The CLI's local build identity follows the whole linked desktop workspace
# and frontend source, while keeping target/ outside its watched directories.
printf '// protocol change\n' >> "$fixture_root/src-tauri/crates/sagascript-engine-protocol/src/lib.rs"
protocol_changed=$(build_count "$evidence_root/protocol-source.log")
assert_eq "$protocol_changed" 1 "linked engine protocol source change"
printf '// new source file\n' > "$fixture_root/src-tauri/crates/sagascript-testsupport/src/new_fixture.rs"
new_crate_source=$(build_count "$evidence_root/new-crate-source.log")
assert_eq "$new_crate_source" 1 "new linked workspace source file"
printf '// desktop change\n' >> "$fixture_root/src-tauri/src/lib.rs"
desktop_changed=$(build_count "$evidence_root/desktop-source.log")
assert_eq "$desktop_changed" 1 "desktop source change"
printf '// frontend change\n' >> "$fixture_root/src/main.ts"
frontend_changed=$(build_count "$evidence_root/frontend-source.log")
assert_eq "$frontend_changed" 1 "frontend source change"

# Complete CI identity must still rebuild for a source change while retaining
# the explicit identity values.
ci_source_warm=$(build_count "$evidence_root/ci-source-warm.log" SAGASCRIPT_GIT_HASH=ci-hash SAGASCRIPT_BUILD_DATE=2026-09-08 SAGASCRIPT_SOURCE_REVISION_INPUT="$fixture_sha" SAGASCRIPT_SOURCE_DIRTY_INPUT=false)
ci_source_same=$(build_count "$evidence_root/ci-source-same.log" SAGASCRIPT_GIT_HASH=ci-hash SAGASCRIPT_BUILD_DATE=2026-09-08 SAGASCRIPT_SOURCE_REVISION_INPUT="$fixture_sha" SAGASCRIPT_SOURCE_DIRTY_INPUT=false)
assert_eq "$ci_source_same" 0 "complete CI source baseline is fresh"
printf '// complete CI source change\n' >> "$fixture/src/lib.rs"
ci_source_changed=$(build_count "$evidence_root/ci-source.log" SAGASCRIPT_GIT_HASH=ci-hash SAGASCRIPT_BUILD_DATE=2026-09-08 SAGASCRIPT_SOURCE_REVISION_INPUT="$fixture_sha" SAGASCRIPT_SOURCE_DIRTY_INPUT=false)
assert_eq "$ci_source_changed" 1 "source change with complete CI metadata"
assert_text "$(cat "$evidence_root/ci-source.identity")" "ci-hash 2026-09-08 $fixture_sha false" "complete CI source identity"

local_before_commit=$(build_count "$evidence_root/local-before-commit.log")
local_before_commit_same=$(build_count "$evidence_root/local-before-commit-same.log")
assert_eq "$local_before_commit_same" 0 "local dirty source baseline is fresh"
git -C "$fixture_root" add src-tauri/crates/sagascript-cli/src/lib.rs src-tauri/crates/sagascript-engine-protocol/src/lib.rs src-tauri/crates/sagascript-testsupport/src/new_fixture.rs src-tauri/src/lib.rs src/main.ts
git -C "$fixture_root" commit -qm source-change
commit_changed=$(build_count "$evidence_root/commit.log")
assert_eq "$commit_changed" 1 "commit change"
new_hash=$(git -C "$fixture_root" rev-parse --short HEAD)
new_full_hash=$(git -C "$fixture_root" rev-parse HEAD)
new_date=$(git -C "$fixture_root" show -s --format=%cs HEAD)
assert_text "$(cat "$evidence_root/commit.identity")" "$new_hash $new_date $new_full_hash false" "committed source identity"
git -C "$fixture_root" checkout -qb fixture-branch
branch_changed=$(build_count "$evidence_root/branch.log")
assert_eq "$branch_changed" 1 "branch change"
assert_text "$(cat "$evidence_root/branch.identity")" "$new_hash $new_date $new_full_hash false" "branch source identity"

# If Git cannot report a trustworthy status, the local fallback never claims
# a clean source tree.
no_git_bin="$evidence_root/no-git-bin"
mkdir -p "$no_git_bin"
cat > "$no_git_bin/git" <<'EOF'
#!/bin/sh
exit 127
EOF
chmod +x "$no_git_bin/git"
clean_fixture
git_unavailable=$(build_count "$evidence_root/git-unavailable.log" "PATH=$no_git_bin:$PATH")
assert_eq "$git_unavailable" 1 "Git command failure"
assert_text "$(cat "$evidence_root/git-unavailable.identity")" "unknown-dirty unknown unknown unknown" "Git failure is unknown and dirty"

# A source archive with no .git directory cannot claim a revision or clean state.
archive="$archive_root/src-tauri/crates/sagascript-cli"
mkdir -p "$archive" "$archive_root/src-tauri/crates/sagascript-core/src"
: > "$archive_root/src-tauri/Cargo.lock"
: > "$archive_root/src-tauri/crates/sagascript-core/Cargo.toml"
git -C "$fixture_root" archive HEAD | tar -x -C "$archive_root"
(cd "$archive" && env -u SAGASCRIPT_GIT_HASH -u SAGASCRIPT_BUILD_DATE \
    -u SAGASCRIPT_SOURCE_REVISION_INPUT -u SAGASCRIPT_SOURCE_DIRTY_INPUT \
    GIT_CONFIG_NOSYSTEM=1 GIT_CONFIG_GLOBAL=/dev/null \
    CARGO_TARGET_DIR="$archive_root/src-tauri/target" cargo build --offline -q)
archive_identity=$(cd "$archive" && "$archive_root/src-tauri/target/debug/build-metadata-fixture")
assert_text "$archive_identity" "unknown-dirty unknown unknown unknown" "source archive identity"

printf 'build metadata fixture passed: CI metadata, workspace source watches, generated-path policy, local dirtiness, Git failure, archive, branch, and commit cases\n'
for log in ci-1 ci-2 ci-3 ci-date absent-1 absent-2 refs-1 refs-2 partial-missing-1 partial-missing-2 partial-date-missing-1 partial-date-missing-2 partial-present-1 partial-present-2 partial-date-present-1 partial-date-present-2 local-warm-1 local-warm-2 helper-changed helper-restored policy-changed generated-only unrelated-untracked renamed-generated source protocol-source new-crate-source desktop-source frontend-source ci-source git-unavailable commit branch; do
    printf '%s: ' "$log"
    cat "$evidence_root/$log.meta"
    if [[ -f "$evidence_root/$log.identity" ]]; then
        printf '  identity: '
        cat "$evidence_root/$log.identity"
    fi
done
