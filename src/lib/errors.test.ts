import { describe, expect, test } from "vitest";
import { backendErrorRecovery, isBackendError, toErrorMessage, type BackendError } from "./errors";

const busy: BackendError = { code: "busy", message: "Sagascript is busy.", retryable: true };

describe("isBackendError", () => {
  test("accepts the wire shape and nothing else", () => {
    expect(isBackendError(busy)).toBe(true);
    expect(isBackendError({ code: "busy", message: "x", retryable: false })).toBe(true);
    for (const value of [
      null,
      undefined,
      "busy",
      42,
      [],
      {},
      { code: "busy" },
      { code: "busy", message: "x" },
      { code: 7, message: "x", retryable: false },
      { code: "busy", message: 7, retryable: false },
      { code: "busy", message: "x", retryable: "yes" },
      // An Error from the runtime, or a Node system error, is not a backend error.
      new Error("boom"),
      Object.assign(new Error("no such file"), { code: "ENOENT" }),
    ]) {
      expect(isBackendError(value), String(value)).toBe(false);
    }
  });

  test("matches one code when given", () => {
    expect(isBackendError(busy, "busy")).toBe(true);
    expect(isBackendError(busy, "cancelled")).toBe(false);
    expect(isBackendError("busy", "busy")).toBe(false);
  });

  test("never matches on the message text", () => {
    const lookalike = { code: "transcription_failed", message: "Dictionary changed elsewhere: x", retryable: false };
    expect(isBackendError(lookalike, "dictionary_changed_elsewhere")).toBe(false);
  });

  test("narrows the type", () => {
    const failure: unknown = busy;
    if (isBackendError(failure)) {
      const code: string = failure.code;
      const retryable: boolean = failure.retryable;
      expect([code, retryable]).toEqual(["busy", true]);
    } else {
      throw new Error("not narrowed");
    }
  });
});

describe("toErrorMessage", () => {
  test("reads every shape a rejected invoke can have", () => {
    expect(toErrorMessage("Failed to save setting.")).toBe("Failed to save setting.");
    expect(toErrorMessage(busy)).toBe("Sagascript is busy.");
    expect(toErrorMessage(new Error("network down"))).toBe("network down");
    expect(toErrorMessage({ message: "plain object" })).toBe("plain object");
  });

  test("falls back for empty or unknown values", () => {
    expect(toErrorMessage(undefined)).toBe("Unknown error");
    expect(toErrorMessage(null, "Could not save.")).toBe("Could not save.");
    expect(toErrorMessage("", "Could not save.")).toBe("Could not save.");
    expect(toErrorMessage("   ", "Could not save.")).toBe("Could not save.");
    expect(toErrorMessage(new Error(""), "Could not save.")).toBe("Could not save.");
    expect(toErrorMessage({ message: 5 }, "Could not save.")).toBe("Could not save.");
    expect(toErrorMessage(42, "Could not save.")).toBe("Could not save.");
    expect(toErrorMessage({}, "Could not save.")).toBe("Could not save.");
  });
});

describe("backendErrorRecovery", () => {
  test("a missing or damaged model is a download, not a retry", () => {
    for (const code of ["engine_model_missing", "engine_model_load_failed"]) {
      expect(backendErrorRecovery({ code, message: "x", retryable: false })).toBe("download_model");
      // Even if a host marked it retryable: the same attempt would fail again.
      expect(backendErrorRecovery({ code, message: "x", retryable: true })).toBe("download_model");
    }
  });

  test("a retryable error offers another attempt", () => {
    expect(backendErrorRecovery(busy)).toBe("retry");
    expect(backendErrorRecovery({ code: "engine_busy", message: "x", retryable: true })).toBe("retry");
  });

  test("anything else offers nothing", () => {
    expect(backendErrorRecovery({ code: "file_not_found", message: "x", retryable: false })).toBeNull();
    expect(backendErrorRecovery({ code: "cancelled", message: "x", retryable: false })).toBeNull();
    expect(backendErrorRecovery("Transcription failed")).toBeNull();
    expect(backendErrorRecovery(new Error("x"))).toBeNull();
    expect(backendErrorRecovery(undefined)).toBeNull();
  });
});
