<script lang="ts">
  import type { OnboardingLanguage } from "./types";

  let {
    selectedLanguage,
    selectLanguage,
    languageError,
    languageSaving,
    platform,
    selectLanguageAndContinue,
  }: {
    selectedLanguage: OnboardingLanguage;
    selectLanguage: (language: OnboardingLanguage) => void;
    languageError: string | null;
    languageSaving: boolean;
    platform: string | null;
    selectLanguageAndContinue: () => void;
  } = $props();
</script>

<div class="step">
  <div class="icon">
    <svg width="48" height="48" viewBox="0 0 48 48" fill="none">
      <rect width="48" height="48" rx="12" fill="var(--accent-dim)" />
      <path
        d="M14 18h10M19 14v4M16 22c1.5 3 4 5.5 7 7M22 22c-1.5 3-4 5.5-7 7"
        stroke="var(--accent)"
        stroke-width="2"
        stroke-linecap="round"
        stroke-linejoin="round"
      />
      <path d="M26 34l3-8 3 8M27 32h4" stroke="var(--accent)" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" />
    </svg>
  </div>
  <h1>Set up Sagascript</h1>
  <p class="description">
    Choose your first profile's language. Its recommended local model is selected automatically; you can add more profiles later.
  </p>
  <div class="language-options">
    <button
      class="language-option"
      class:selected={selectedLanguage === "en"}
      aria-pressed={selectedLanguage === "en"}
      onclick={() => selectLanguage("en")}
    >
      <span class="lang-flag">EN</span>
      <span class="lang-name">English</span>
    </button>
    <button
      class="language-option"
      class:selected={selectedLanguage === "sv"}
      aria-pressed={selectedLanguage === "sv"}
      onclick={() => selectLanguage("sv")}
    >
      <span class="lang-flag">SV</span>
      <span class="lang-name">Svenska</span>
    </button>
    <button
      class="language-option"
      class:selected={selectedLanguage === "no"}
      aria-pressed={selectedLanguage === "no"}
      onclick={() => selectLanguage("no")}
    >
      <span class="lang-flag">NO</span>
      <span class="lang-name">Norsk</span>
    </button>
    <button
      class="language-option"
      class:selected={selectedLanguage === "fi"}
      aria-pressed={selectedLanguage === "fi"}
      onclick={() => selectLanguage("fi")}
    >
      <span class="lang-flag">FI</span>
      <span class="lang-name">Suomi</span>
    </button>
    <button
      class="language-option"
      class:selected={selectedLanguage === "da"}
      aria-pressed={selectedLanguage === "da"}
      onclick={() => selectLanguage("da")}
    >
      <span class="lang-flag">DA</span>
      <span class="lang-name">Dansk</span>
    </button>
  </div>
  {#if languageError}
    <div class="status-indicator error">
      <span class="status-dot"></span>
      <span>{languageError}</span>
    </div>
  {/if}
  <div class="actions">
    <button class="primary" onclick={selectLanguageAndContinue} disabled={languageSaving || platform === null}>
      {platform === null ? "Preparing…" : languageSaving ? "Saving…" : "Continue"}
    </button>
  </div>
</div>

<style>
  .step {
    display: flex;
    flex-direction: column;
    align-items: center;
    text-align: center;
    max-width: 380px;
    animation: fadeIn 0.3s ease;
  }

  @keyframes fadeIn {
    from {
      opacity: 0;
      transform: translateY(8px);
    }
    to {
      opacity: 1;
      transform: translateY(0);
    }
  }

  .icon {
    margin-bottom: 20px;
  }

  h1 {
    font-size: 20px;
    font-weight: 600;
    color: var(--text);
    margin: 0 0 12px;
  }

  .description {
    font-size: 13px;
    line-height: 1.6;
    color: var(--text-muted);
    margin: 0 0 8px;
  }

  /* Language selector */

  .language-options {
    display: flex;
    flex-wrap: wrap;
    justify-content: center;
    gap: 10px;
    margin: 20px 0 24px;
  }

  .language-option {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 6px;
    padding: 14px 20px;
    background: var(--bg-secondary);
    border: 2px solid var(--border);
    border-radius: 10px;
    cursor: pointer;
    transition:
      border-color 0.2s,
      background 0.2s;
    min-width: 90px;
  }

  .language-option:hover {
    border-color: var(--text-muted);
  }

  .language-option.selected {
    border-color: var(--accent);
    background: color-mix(in srgb, var(--accent) 8%, var(--bg-secondary));
  }

  .lang-flag {
    font-size: 14px;
    font-weight: 700;
    color: var(--accent);
    letter-spacing: 1px;
  }

  .lang-name {
    font-size: 12px;
    color: var(--text-muted);
  }

  .language-option.selected .lang-name {
    color: var(--text);
  }

  /* Status indicators */

  .status-indicator {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 8px 16px;
    border-radius: var(--radius);
    background: color-mix(in srgb, var(--danger) 8%, var(--bg-secondary));
    font-size: 12px;
    color: var(--text-muted);
    margin: 16px 0 24px;
    transition: background 0.3s;
  }

  .status-indicator.error {
    background: color-mix(in srgb, var(--danger) 12%, var(--bg-secondary));
    color: var(--danger);
  }

  .status-dot {
    width: 8px;
    height: 8px;
    border-radius: 50%;
    background: var(--danger);
    flex-shrink: 0;
    transition: background 0.3s;
  }

  .status-indicator.error .status-dot {
    background: var(--danger);
  }

  /* Actions */

  .actions {
    display: flex;
    flex-direction: column;
    gap: 10px;
    width: 100%;
    max-width: 300px;
  }

  .actions button {
    width: 100%;
    padding: 10px 20px;
    font-size: 13px;
  }
</style>
