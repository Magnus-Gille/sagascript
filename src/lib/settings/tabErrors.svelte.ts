// The error of each tab of the Settings window (SAGA-379): a settings change
// that fails is reported on the tab where it was made. The shell shows the
// message of the tab that is showing and marks the button of a tab that has
// one while another tab shows; a change on one tab neither shows nor clears
// the error of another.

/** One tab's error message; empty while it has none. */
export class TabError {
  message: string = $state("");

  /** Take `message` away if it is the one shown: what it said is no longer true. */
  dismiss(message: string): void {
    if (message && this.message === message) this.message = "";
  }
}

export class TabErrors {
  // A plain Map on purpose: only each error's `message` is state. Which tabs have
  // an object is not, and `of` may add one while the shell renders, which a
  // reactive map would refuse.
  // eslint-disable-next-line svelte/prefer-svelte-reactivity
  readonly #byTab = new Map<string, TabError>();

  constructor(tabIds: readonly string[]) {
    for (const id of tabIds) this.#byTab.set(id, new TabError());
  }

  /** The error of tab `tabId`, the same object every time. A tab that was not listed gets one on first use. */
  of(tabId: string): TabError {
    let error = this.#byTab.get(tabId);
    if (!error) {
      error = new TabError();
      this.#byTab.set(tabId, error);
    }
    return error;
  }

  /** Take `message` away from every tab that shows it. */
  dismiss(message: string): void {
    for (const error of this.#byTab.values()) error.dismiss(message);
  }
}
