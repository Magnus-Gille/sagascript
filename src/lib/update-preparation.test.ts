import { expect, test } from "vitest";
import { UpdatePreparationTimeoutError, drainUpdateWork } from "./update-preparation";

const deferred = () => {
  let resolve!: () => void;
  const promise = new Promise<void>((r) => (resolve = r));
  return { promise, resolve };
};

test("waits for terminal result delivery and rendering before snapshot", async () => {
  let busy = true,
    rendered = false,
    done = false;
  const work = drainUpdateWork({
    busy: () => busy,
    settle: async () => {
      if (!busy) rendered = true;
    },
    timeoutMs: 200,
    pollMs: 1,
  }).then(() => (done = true));
  await new Promise((r) => setTimeout(r, 5));
  expect(done).toBe(false);
  busy = false;
  await work;
  expect(rendered).toBe(true);
});

test("waits for an in-flight correction even when transcription is idle", async () => {
  const edit = deferred();
  let snapshotted = false;
  const work = drainUpdateWork({ busy: () => false, settle: () => edit.promise, timeoutMs: 200 }).then(() => (snapshotted = true));
  await Promise.resolve();
  expect(snapshotted).toBe(false);
  edit.resolve();
  await work;
  expect(snapshotted).toBe(true);
});

test("blocked handoff and failed initialization fail closed", async () => {
  await expect(drainUpdateWork({ busy: () => true, settle: async () => {}, timeoutMs: 10, pollMs: 1 })).rejects.toThrow(/finish/);
  await expect(drainUpdateWork({ busy: () => false, settle: () => new Promise(() => {}), timeoutMs: 10 })).rejects.toThrow(/finish/);
  await expect(
    drainUpdateWork({
      busy: () => false,
      settle: async () => {
        throw new Error("review failed");
      },
    }),
  ).rejects.toThrow(/review failed/);
});

test("a drain deadline is distinguishable from cancellation or rendering failure", async () => {
  await expect(drainUpdateWork({ busy: () => true, settle: async () => {}, timeoutMs: 10, pollMs: 1 })).rejects.toSatisfy(
    (error) => error instanceof UpdatePreparationTimeoutError && /finish preparing/.test(error.message),
  );
  const renderError = new Error("rendering failed");
  await expect(
    drainUpdateWork({
      busy: () => false,
      settle: async () => {
        throw renderError;
      },
    }),
  ).rejects.toSatisfy((error) => error === renderError);
  const cancellation = "Update preparation expired.";
  await expect(drainUpdateWork({ busy: () => true, settle: async () => {}, failure: () => cancellation })).rejects.toSatisfy(
    (error) => !(error instanceof UpdatePreparationTimeoutError) && error instanceof Error && error.message === cancellation,
  );
});
