// Vitest (#361): unit and component tests for src/, run with `npm run test:unit`.
// Reuses vite.config.ts (and through it the Svelte plugin) so tests compile
// exactly what the app bundles. Where tests live and how to write them:
// docs/testing/frontend.md.
import { svelteTesting } from "@testing-library/svelte/vite";
import { defineConfig, mergeConfig } from "vitest/config";
import viteConfig from "./vite.config";

export default mergeConfig(
  viteConfig,
  defineConfig({
    // Resolves Svelte's browser runtime (mount, effects) rather than its
    // server build. src/test/setup.ts unmounts components itself, ordered
    // with the IPC mock, so the plugin's own cleanup is off.
    plugins: [svelteTesting({ autoCleanup: false })],
    test: {
      include: ["src/**/*.test.ts"],
      // Every file gets a DOM (happy-dom): components render into it, and
      // @tauri-apps/api keeps its IPC hooks on `window`. Logic modules stay
      // DOM-free by design (docs/testing/frontend.md), not by environment.
      environment: "happy-dom",
      setupFiles: ["src/test/setup.ts"],
      restoreMocks: true,
      coverage: {
        provider: "v8",
        // A report, not a gate (yet): every frontend module and component,
        // tested or not.
        include: ["src/lib/**/*.ts", "src/lib/**/*.svelte"],
        exclude: ["src/**/*.test.ts", "src/**/*.d.ts"],
        reporter: ["text", "html"],
        reportsDirectory: "coverage",
      },
    },
  }),
);
