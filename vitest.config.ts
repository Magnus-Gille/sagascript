// Vitest (#361): unit tests for src/, run with `npm run test:unit`.
// Reuses vite.config.ts (and through it the Svelte plugin) so tests compile
// exactly what the app bundles. Where tests live and how to write them:
// docs/testing/frontend.md.
import { defineConfig, mergeConfig } from "vitest/config";
import viteConfig from "./vite.config";

export default mergeConfig(
  viteConfig,
  defineConfig({
    test: {
      include: ["src/**/*.test.ts"],
      // No DOM yet: component tests (and a DOM environment) arrive with the
      // second half of #361. src/test/setup.ts provides what the Tauri IPC
      // mock needs in plain Node.
      environment: "node",
      setupFiles: ["src/test/setup.ts"],
      restoreMocks: true,
      coverage: {
        provider: "v8",
        // A report, not a gate (yet): every frontend module, tested or not.
        include: ["src/lib/**/*.ts"],
        exclude: ["src/**/*.test.ts", "src/**/*.d.ts"],
        reporter: ["text", "html"],
        reportsDirectory: "coverage",
      },
    },
  }),
);
