// Every Node test file belongs to exactly one group (#350). The files stay
// where they are (#361 moves the UI tests to Vitest); this manifest gives
// them names. scripts/run-frontend-tests.mjs fails when a `test-*.mjs` or
// `test_*.mjs` file under scripts/ is missing here, or an entry points to a
// file that does not exist. (This file is not named test-*.mjs so that the
// discovery does not take it for a test.)
//
//   npm run test:frontend      all groups (what CI runs)
//   npm run test:ui            ui
//   npm run test:ci            ci
//   npm run test:release       release
//   npm run test:code-health   codeHealth, plus tests/scripts/ (code-health.yml)

export const GROUP_DESCRIPTIONS = {
  ui: "frontend logic and component contracts (src/lib), language/settings surface",
  ci: "CI workflows, toolchain pins, local verification and evaluation tooling",
  release: "release, updater, signing, packaging and Windows distribution",
  codeHealth: "code-health producer and workflow (also in npm run test:code-health)",
};

// npm packages a group's tests import. The runner checks them before
// starting, so a checkout without `npm ci` gets one clear error instead of a
// failure per file. Groups not listed need nothing beyond Node itself.
export const GROUP_NPM_PACKAGES = {
  ui: ["typescript"],
};

export const TEST_GROUPS = {
  ui: [
    "scripts/test-dictation-ui-state.mjs",
    "scripts/test-dictionary-explicit-save.mjs",
    "scripts/test-file-transcription-integration.mjs",
    "scripts/test-file-transcription-recovery.mjs",
    "scripts/test-finnish-language.mjs",
    "scripts/test-hotkey-shortcuts.mjs",
    "scripts/test-language-glossary.mjs",
    "scripts/test-language-switch-recovery.mjs",
    "scripts/test-meeting-integration.mjs",
    "scripts/test-meeting-playback.mjs",
    "scripts/test-meeting-progress.mjs",
    "scripts/test-meeting-reprocessing-integration.mjs",
    "scripts/test-meeting-reprocessing.mjs",
    "scripts/test-onboarding-download-state.mjs",
    "scripts/test-overlay-state.mjs",
    "scripts/test-profile-shortcuts.mjs",
    "scripts/test-recent-transcriptions.mjs",
    "scripts/test-settings-layout.mjs",
    "scripts/test-settings-recovery.mjs",
    "scripts/test-transcribe-stages.mjs",
    "scripts/test-transcribe-ui-state.mjs",
    "scripts/test-transcription-queue.mjs",
    "scripts/test-update-preparation.mjs",
    "scripts/test-update-recovery.mjs",
  ],
  ci: [
    "scripts/diarization_sv/test_reference_review.mjs",
    "scripts/test-ci-build-identity.mjs",
    "scripts/test-ci-cli-feature-coverage.mjs",
    "scripts/test-ci-cohort-timings.mjs",
    "scripts/test-ci-docs-scope.mjs",
    "scripts/test-ci-docs-workflow.mjs",
    "scripts/test-ci-parallel-gates.mjs",
    "scripts/test-ci-run-timings.mjs",
    "scripts/test-ci-unrun-suites.mjs",
    "scripts/test-clippy-policy.mjs",
    "scripts/test-dependency-policy.mjs",
    "scripts/test-frontend-test-groups.mjs",
    "scripts/test-pipeline-cache.mjs",
    "scripts/test-rust-toolchain-pin.mjs",
    "scripts/test-verify-ci-parity.mjs",
    "scripts/test-verify-runner.mjs",
    "scripts/test-workflow-hardening.mjs",
  ],
  release: [
    "scripts/test-frontend-reuse.mjs",
    "scripts/test-generate-updater-manifest.mjs",
    "scripts/test-generate-updater-smoke-config.mjs",
    "scripts/test-pianissimo-requirements.mjs",
    "scripts/test-record-release-downloads.mjs",
    "scripts/test-release-promotion.mjs",
    "scripts/test-release-surface.mjs",
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
  codeHealth: [
    "scripts/test-code-health-producer.mjs",
    "scripts/test-code-health-workflow.mjs",
  ],
};
