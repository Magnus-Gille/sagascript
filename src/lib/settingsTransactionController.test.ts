import { describe, expect, test, vi } from "vitest";
import { createSettingsTransactionController } from "./settingsTransactionController";

const blockedError = "Reload settings first.";

function controller(overrides: Partial<Parameters<typeof createSettingsTransactionController>[0]> = {}) {
  const isUnconfirmed = vi.fn(() => false);
  const readBack = vi.fn(async () => undefined);
  const markUnconfirmed = vi.fn(() => undefined);
  const restamp = vi.fn(() => false);
  return {
    subject: createSettingsTransactionController({
      isUnconfirmed,
      unconfirmedError: blockedError,
      readBack,
      markUnconfirmed,
      restamp,
      ...overrides,
    }),
    isUnconfirmed,
    readBack,
    markUnconfirmed,
    restamp,
  };
}

describe("settings transaction controller", () => {
  test("a rejected mutation is not read back", async () => {
    const { subject, readBack, markUnconfirmed, restamp } = controller();
    const error = new Error("write failed");
    const mutate = vi.fn(async () => {
      throw error;
    });

    await expect(subject.save(mutate)).resolves.toEqual({ saved: false, error });
    expect(mutate).toHaveBeenCalledOnce();
    expect(readBack).not.toHaveBeenCalled();
    expect(markUnconfirmed).not.toHaveBeenCalled();
    expect(restamp).not.toHaveBeenCalled();
  });

  test("afterSave runs before read-back", async () => {
    const order: string[] = [];
    const readBack = vi.fn(async () => {
      order.push("read-back");
    });
    const { subject } = controller({ readBack });

    const outcome = await subject.save(
      async () => {
        order.push("mutation");
        return "saved";
      },
      (result) => {
        order.push(`after-save:${result}`);
      },
    );

    expect(order).toEqual(["mutation", "after-save:saved", "read-back"]);
    expect(outcome).toEqual({ saved: true, result: "saved", confirmed: true });
  });

  test("a read-back failure keeps the save successful and marks it unconfirmed without retrying the mutation", async () => {
    const error = new Error("read failed");
    let unconfirmed = false;
    const mark = vi.fn((error: unknown, afterSave: boolean) => {
      unconfirmed = error instanceof Error && afterSave;
    });
    const { subject } = controller({
      isUnconfirmed: () => unconfirmed,
      readBack: async () => {
        throw error;
      },
      markUnconfirmed: mark,
    });
    const mutate = vi.fn(async () => "saved");

    const first = await subject.save(mutate);
    const second = await subject.save(mutate);

    expect(first).toEqual({ saved: true, result: "saved", confirmed: false });
    expect(mark).toHaveBeenCalledWith(error, true);
    expect(second).toEqual({ saved: false, error: blockedError });
    expect(mutate).toHaveBeenCalledOnce();
  });

  test("pre-existing unconfirmed settings block the mutation and read-back", async () => {
    const { subject, readBack } = controller({ isUnconfirmed: () => true });
    const mutate = vi.fn(async () => "saved");

    await expect(subject.save(mutate)).resolves.toEqual({ saved: false, error: blockedError });
    expect(mutate).not.toHaveBeenCalled();
    expect(readBack).not.toHaveBeenCalled();
  });

  test("a concurrent unconfirmed state is restamped and the save stays unconfirmed", async () => {
    const restamp = vi.fn(() => true);
    const { subject } = controller({ restamp });

    await expect(subject.save(async () => "saved")).resolves.toEqual({
      saved: true,
      result: "saved",
      confirmed: false,
    });
    expect(restamp).toHaveBeenCalledWith(true);
  });

  test("afterSave errors remain outside the mutation failure path", async () => {
    const { subject, readBack } = controller();
    const error = new Error("caller callback failed");

    await expect(
      subject.save(
        async () => "saved",
        () => {
          throw error;
        },
      ),
    ).rejects.toBe(error);
    expect(readBack).not.toHaveBeenCalled();
  });
});
