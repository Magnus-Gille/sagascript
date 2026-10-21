import { describe, expect, test, vi } from "vitest";
import { deferred } from "../test/deferred";
import { createModelCatalogController } from "./modelCatalogController";

describe("model catalog controller", () => {
  test("older success and error results are ignored after a newer request", async () => {
    const older = deferred<string>();
    const newer = deferred<string>();
    const publish = vi.fn();
    const reject = vi.fn();
    const load = vi.fn((input: string) => (input === "older" ? older.promise : newer.promise));
    const subject = createModelCatalogController({ load, publish, reject });

    const oldRequest = subject.request("older");
    await vi.waitFor(() => expect(load).toHaveBeenCalledTimes(1));
    const newRequest = subject.request("newer");
    await vi.waitFor(() => expect(load).toHaveBeenCalledTimes(2));
    newer.resolve("new catalog");
    await expect(newRequest.done).resolves.toBe(true);
    older.resolve("stale catalog");
    await expect(oldRequest.done).resolves.toBe(false);

    expect(publish).toHaveBeenCalledOnce();
    expect(publish).toHaveBeenCalledWith("new catalog");
    expect(reject).not.toHaveBeenCalled();

    const failedOld = deferred<string>();
    const active = deferred<string>();
    const secondLoad = vi.fn((input: string) => (input === "failed" ? failedOld.promise : active.promise));
    const secondController = createModelCatalogController({
      load: secondLoad,
      publish,
      reject,
    });
    const failedRequest = secondController.request("failed");
    await vi.waitFor(() => expect(secondLoad).toHaveBeenCalledTimes(1));
    const activeRequest = secondController.request("active");
    await vi.waitFor(() => expect(secondLoad).toHaveBeenCalledTimes(2));
    failedOld.reject(new Error("stale failure"));
    active.resolve("active catalog");
    await expect(Promise.all([failedRequest.done, activeRequest.done])).resolves.toEqual([false, true]);
    expect(reject).not.toHaveBeenCalled();
    expect(publish).toHaveBeenLastCalledWith("active catalog");
  });

  test("canceling an older handle cannot invalidate a newer request", async () => {
    const older = deferred<string>();
    const newer = deferred<string>();
    const publish = vi.fn();
    const load = vi.fn((input: string) => (input === "older" ? older.promise : newer.promise));
    const subject = createModelCatalogController({
      load,
      publish,
      reject: vi.fn(),
    });

    const oldRequest = subject.request("older");
    await vi.waitFor(() => expect(load).toHaveBeenCalledTimes(1));
    const newRequest = subject.request("newer");
    await vi.waitFor(() => expect(load).toHaveBeenCalledTimes(2));
    oldRequest.cancel();
    newer.resolve("current");

    await expect(newRequest.done).resolves.toBe(true);
    older.resolve("stale");
    await expect(oldRequest.done).resolves.toBe(false);
    expect(publish).toHaveBeenCalledOnce();
    expect(publish).toHaveBeenCalledWith("current");
  });

  test("dispose ignores an outstanding error and prevents later loads", async () => {
    const outstanding = deferred<string>();
    const load = vi.fn(() => outstanding.promise);
    const publish = vi.fn();
    const reject = vi.fn();
    const subject = createModelCatalogController({ load, publish, reject });

    const request = subject.request(undefined);
    await vi.waitFor(() => expect(load).toHaveBeenCalledOnce());
    subject.dispose();
    outstanding.reject(new Error("late failure"));
    await expect(request.done).resolves.toBe(false);
    await expect(subject.request(undefined).done).resolves.toBe(false);

    expect(load).toHaveBeenCalledOnce();
    expect(publish).not.toHaveBeenCalled();
    expect(reject).not.toHaveBeenCalled();
  });

  test("dispose ignores an outstanding success", async () => {
    const outstanding = deferred<string>();
    const load = vi.fn(() => outstanding.promise);
    const publish = vi.fn();
    const reject = vi.fn();
    const subject = createModelCatalogController({ load, publish, reject });

    const request = subject.request(undefined);
    await vi.waitFor(() => expect(load).toHaveBeenCalledOnce());
    subject.dispose();
    outstanding.resolve("late catalog");

    await expect(request.done).resolves.toBe(false);
    expect(publish).not.toHaveBeenCalled();
    expect(reject).not.toHaveBeenCalled();
  });

  test("a current failure is reported and returns false", async () => {
    const error = new Error("catalog failed");
    const reject = vi.fn();
    const subject = createModelCatalogController({
      load: async () => {
        throw error;
      },
      publish: vi.fn(),
      reject,
    });

    await expect(subject.request(undefined).done).resolves.toBe(false);
    expect(reject).toHaveBeenCalledWith(error);
  });
});
