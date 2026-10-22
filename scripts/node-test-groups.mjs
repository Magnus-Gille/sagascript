// Every Node test file belongs to exactly one group (#350). Frontend module
// tests run under Vitest (`npm run test:unit`, src/**/*.test.ts, #361); the
// ui group keeps the source-text and component-harness tests until #361's
// phase B replaces them. This manifest gives the files names.
// scripts/run-frontend-tests.mjs fails when a `test-*.mjs` or `test_*.mjs`
// file anywhere under scripts/ (except fixtures/ and node_modules/) is
// missing here, or an entry points to a file that does not exist. (This file
// is not named test-*.mjs so that the
// discovery does not take it for a test.)
//
//   npm run test:frontend      all groups (what CI runs)
//   npm run test:ui            ui
//   npm run test:ci            ci
//   npm run test:release       release
//   npm run test:code-health   codeHealth, plus tests/scripts/ (code-health.yml)

export const GROUP_DESCRIPTIONS = {
  ui: "component source-text and harness tests awaiting #361 phase B (module tests: npm run test:unit)",
  ci: "CI workflows, toolchain pins, local verification and evaluation tooling",
  release: "release, updater, signing, packaging and Windows distribution",
  codeHealth: "code-health producer and workflow (also in npm run test:code-health)",
};

// npm packages a group's tests import. The runner checks them before
// starting, so a checkout without `npm ci` gets one clear error instead of a
// failure per file. Groups not listed need nothing beyond Node itself.
export const GROUP_NPM_PACKAGES = {
  ui: [],
  // scripts/lib/ci-workflow.mjs parses the workflows with `yaml` (#358).
  ci: ["vite", "yaml"],
  release: ["yaml"],
};

export const TEST_GROUPS = {
  ui: ["scripts/test-file-transcription-recovery.mjs", "scripts/test-finnish-language.mjs"],
  ci: [
    "scripts/diarization_sv/test_reference_review.mjs",
    "scripts/test-browser-mock-commands.mjs",
    "scripts/test-ci-build-identity.mjs",
    "scripts/test-ci-cargo-gates.mjs",
    "scripts/test-ci-cli-feature-coverage.mjs",
    "scripts/test-ci-cohort-timings.mjs",
    "scripts/test-ci-docs-scope.mjs",
    "scripts/test-ci-docs-workflow.mjs",
    "scripts/test-ci-parallel-gates.mjs",
    "scripts/test-ci-run-timings.mjs",
    "scripts/test-ci-smoke-cli.mjs",
    "scripts/test-ci-unrun-suites.mjs",
    "scripts/test-ci-windows-shell-guard.mjs",
    "scripts/test-clippy-policy.mjs",
    "scripts/test-dependency-policy.mjs",
    "scripts/test-engine-host-identity.mjs",
    "scripts/test-engine-host-conformance-reader.mjs",
    "scripts/test-frontend-test-groups.mjs",
    "scripts/test-npm-license-policy.mjs",
    "scripts/test-pipeline-cache.mjs",
    "scripts/test-rust-toolchain-pin.mjs",
    "scripts/test-verify-ci-parity.mjs",
    "scripts/test-verify-runner.mjs",
    "scripts/test-workflow-hardening.mjs",
  ],
  release: [
    "scripts/test-draft-reverification.mjs",
    "scripts/test-frontend-reuse.mjs",
    "scripts/test-generate-updater-manifest.mjs",
    "scripts/test-generate-updater-smoke-config.mjs",
    "scripts/test-pianissimo-requirements.mjs",
    "scripts/test-record-release-downloads.mjs",
    "scripts/test-release-promotion.mjs",
    "scripts/test-release-surface.mjs",
    "scripts/test-release-version-consistency.mjs",
    "scripts/test-release-updater-startup.mjs",
    "scripts/test-report-windows-arm64-native.mjs",
    "scripts/test-smoke-signed-app-start.mjs",
    "scripts/test-test-build-input-safety.mjs",
    "scripts/test-updater-smoke-environment.mjs",
    "scripts/test-windows-acceptance.mjs",
    "scripts/test-windows-arm-pianissimo.mjs",
    "scripts/test-windows-arm64-native.mjs",
    "scripts/test-windows-package-workflow.mjs",
    "scripts/test-windows-release-identity.mjs",
    "scripts/test-windows-x64-cpu-policy.mjs",
    "scripts/test-windows-x64-portable.mjs",
  ],
  codeHealth: ["scripts/test-code-health-producer.mjs", "scripts/test-code-health-workflow.mjs"],
};
