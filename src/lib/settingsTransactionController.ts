export type SaveOutcome<T> = { saved: false; error: unknown } | { saved: true; result: T; confirmed: boolean };

export interface SettingsTransactionControllerOptions {
  isUnconfirmed: () => boolean;
  unconfirmedError: unknown;
  readBack: () => Promise<unknown>;
  markUnconfirmed: (error: unknown, afterSave: boolean) => void;
  restamp: (afterSave: boolean) => boolean;
}

/** Coordinates one settings write with the read-back that confirms it. */
export function createSettingsTransactionController({
  isUnconfirmed,
  unconfirmedError,
  readBack,
  markUnconfirmed,
  restamp,
}: SettingsTransactionControllerOptions) {
  async function save<T>(mutate: () => Promise<T>, afterSave?: (result: T) => void): Promise<SaveOutcome<T>> {
    if (isUnconfirmed()) return { saved: false, error: unconfirmedError };

    let result: T;
    try {
      result = await mutate();
    } catch (error) {
      return { saved: false, error };
    }

    // This deliberately stays outside the mutation catch: a caller callback
    // failure must not turn a completed backend write into a failed save.
    afterSave?.(result);

    try {
      await readBack();
    } catch (error) {
      markUnconfirmed(error, true);
      return { saved: true, result, confirmed: false };
    }

    if (restamp(true)) return { saved: true, result, confirmed: false };
    return { saved: true, result, confirmed: true };
  }

  return { save };
}
