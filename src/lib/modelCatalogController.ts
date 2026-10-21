export interface ModelCatalogRequest {
  done: Promise<boolean>;
  cancel: () => void;
}

/** Owns the latest-request lifecycle for a model catalogue. */
export function createModelCatalogController<Input, Result>(options: {
  load: (input: Input) => Promise<Result>;
  publish: (result: Result) => void;
  reject: (error: unknown) => void;
}) {
  let generation = 0;
  let disposed = false;

  function request(input: Input): ModelCatalogRequest {
    if (disposed) return { done: Promise.resolve(false), cancel: () => undefined };

    const requestGeneration = ++generation;
    const isCurrent = () => !disposed && generation === requestGeneration;
    let cancelled = false;

    const done = (async () => {
      // Let request() return its cancel handle before entering user code, and
      // skip a queued load if it was already superseded or disposed.
      await Promise.resolve();
      if (!isCurrent()) return false;

      let result: Result;
      try {
        result = await options.load(input);
      } catch (error) {
        if (isCurrent()) options.reject(error);
        return false;
      }

      if (!isCurrent()) return false;
      options.publish(result);
      return true;
    })();

    return {
      done,
      cancel() {
        if (cancelled) return;
        cancelled = true;
        // An effect cleanup may run after a manual refresh has superseded this
        // request; it must never invalidate that newer request.
        if (isCurrent()) generation += 1;
      },
    };
  }

  function dispose(): void {
    if (disposed) return;
    disposed = true;
    generation += 1;
  }

  return { request, dispose };
}
