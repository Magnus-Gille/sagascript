// The tabs of the Settings window (SAGA-379): which one shows, and the one way
// to change it. A panel can hold the tabs (`held`) or ask before they change
// (`guard`, the dictionary editor's dirty guard).

export type SettingsTab = "dictate" | "transcribe" | "settings";

/** What a panel with unsaved work asks before the tab changes. */
export interface LeaveGuard {
  requestLeave(leave: () => void): void;
  isAsking(): boolean;
}

export class TabRouter {
  active = $state<SettingsTab>("dictate");
  /** A panel holds the tabs while it is set: a language change is in flight. */
  held = $state(false);
  guard = $state.raw<LeaveGuard | null>(null);

  get asking(): boolean {
    return this.guard?.isAsking() ?? false;
  }

  request(nextTab: SettingsTab, afterNavigate?: () => void): void {
    if (this.held) return;
    if (nextTab === this.active) {
      afterNavigate?.();
      return;
    }
    const leave = (): void => {
      this.active = nextTab;
      afterNavigate?.();
    };
    // The dictionary editor asks first while it holds unsaved changes.
    if (this.guard) this.guard.requestLeave(leave);
    else leave();
  }
}
