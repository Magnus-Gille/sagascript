import { spawn } from "node:child_process";
import { fileURLToPath } from "node:url";
import { createServer } from "vite";

// Each run owns its server and browser context; no installed app or audio is used.
const server = await createServer({
  server: { host: "127.0.0.1", port: 0, strictPort: false },
});
try {
  await server.listen();
  const address = server.httpServer?.address();
  if (!address || typeof address === "string") throw new Error("Vite did not expose a TCP port.");
  const result = await new Promise((resolve, reject) => {
    const child = spawn(process.execPath, [fileURLToPath(new URL("./qa-transcription-tabs.mjs", import.meta.url))], {
      stdio: "inherit",
      env: { ...process.env, QA_URL: `http://127.0.0.1:${address.port}/?tab=transcribe` },
    });
    child.once("error", reject);
    child.once("exit", (code, signal) => resolve({ code, signal }));
  });
  if (result.signal) console.error(`Browser tests terminated by ${result.signal}`);
  process.exitCode = result.code ?? 1;
} finally {
  await server.close();
}
