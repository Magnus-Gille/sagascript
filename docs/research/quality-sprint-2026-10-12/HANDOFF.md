# Överlämning: kvalitetssprint Sagascript (2026-10-12)

Du tar över genomförandet av en kvalitetssprint i `Magnus-Gille/sagascript` (Tauri v2 + Rust +
Svelte 5 + Swift Core ML-host, macOS-dikteringsapp). Två oberoende utvärderingar är gjorda på
samma revision och allt arbete är nedbrutet i GitHub-issues. Din uppgift är att genomföra
ticketsen i angiven ordning, en ticket per branch/PR, utan att släppa något.

## 1. Utgångsläge

- **Revision:** `origin/main` = `a69d5d1e58cc6f169b8984132ba8e170d3edd64d` (Sagascript 1.4.4-dev).
  Starta alltid från `origin/main`, aldrig från lokal `main` (den låg 54 commits efter vid
  utvärderingen).
- **Underlag:**
  - Codex sprintplan **#351** med tickets **K01–K11 = #340–#350** (svenska, självbärande).
  - Claude-utvärdering med 38 tickets, varav 33 filade som **#352–#384** (label `from:claude-code`),
    och 5 inlagda som kommentarer på #343, #344, #347, #350. Fulltext + baseline:
    `docs/research/quality-sprint-2026-10-12/README.md` på branchen `claude/quality-sprint-audit`
    (lokalt worktree `/Users/magnus/repos/sagascript-quality-sprint-claude`; be ägaren pusha
    branchen om du kör någon annanstans — issue-texterna är i sig kompletta).
  - Synteskommentar på #351 med sammanslagen vågordning. Kommentarer på #340–#350 innehåller
    konkreta tillägg till respektive ticket — läs dem innan du börjar på en Codex-ticket.
- **Baseline (verifierad):** `cargo clippy --workspace --all-targets -- -D warnings` rent;
  `cargo test --workspace` ≈1 490 gröna, 2 ignored; `svelte-check` 0 fel; Node-tester 404
  (394 pass, 10 skip); Python 127; `npm audit` 0. **Men:** `cargo fmt --check` = 906 hunks i
  72/128 filer (aldrig formaterat); 83 `lock().unwrap()` på `Mutex<AppController>` inkl. fyra
  `Drop`-impl; 13 frontend-testfiler regex:ar `.svelte`-källa; `check-linux` är inte required;
  engine-protocol-craten och ORT-hosten testas aldrig i PR-CI.

## 2. Regler som gäller (från AGENTS.md och ägaren)

- Arbeta **endast i ett eget worktree på en task-branch**; rör aldrig primärcheckouten
  `/Users/magnus/repos/sagascript`. En ticket = en branch = en PR.
- Läs/skriv/committa aldrig `.env`, `.env.*`, `secrets/**`. Inga hårdkodade hemligheter.
- Invarianter: CLI-first (varje funktion har CLI-motsvarighet), privacy-first (lokalt är
  default), `enigo`/TIS/HIToolbox bara på macOS-huvudtråden (`run_on_main_thread`), Svelte 5
  runes (inga legacy stores), generated build-metadata för version/SHA.
- **Push till `main` är förbjudet.** Leverans = PR mot `main`. Ägaren mergar. Ingen release,
  installation, signering, credential- eller produktionsändring ingår i någon ticket.
- Substantiella PR:er behöver oberoende cross-model review innan de markeras klara; redovisa
  vilken reviewer. Försvaga aldrig tester, grindar eller acceptanskriterier för att bli klar.
- Varje konstaterad bugg får ett regressionstest som är rött före fixen och grönt efter.
  Refaktorer är beteendebevarande: samma CLI/JSON/progress-kontrakt, cancellation, cleanup.
- Fixtures får aldrig skriva till riktiga användarinställningar eller inspelningar; använd
  `SAGASCRIPT_SETTINGS_PATH` och tempkataloger.

## 3. Praktiska fallgropar (kända, dokumenteras/fixas i #350/#352/#353)

```bash
git -C /Users/magnus/repos/sagascript fetch origin
git -C /Users/magnus/repos/sagascript worktree add ../sagascript-<ticket> -b <prefix>/<ticket>-<slug> origin/main
cd ../sagascript-<ticket> && npm ci
npm run build            # KRÄVS före cargo test/clippy: tauri::generate_context! panikar utan ../dist
cd src-tauri
cargo check --workspace && cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings
cargo clippy -p sagascript-cli --all-targets --no-default-features -- -D warnings
cd .. && npx svelte-check --tsconfig ./tsconfig.json && npm run test:frontend && npm run test:code-health
python3 -m unittest discover -s scripts/dictation_eval -p 'test_*.py'
./scripts/build-engine-host.sh && (cd src-tauri/engine-host/coreml && swift test)   # swift test kräver genererad BuildInfo.swift
```

- Tips: `CARGO_TARGET_DIR=/Users/magnus/repos/sagascript/src-tauri/target` i worktreet sparar
  ~15 min whisper.cpp-bygge. Kör ett cargo-kommando i taget (byggl ås).
- Cargo-bygget regenererar `src-tauri/gen/schemas/*` — committa inte de diffarna.
- `cargo test` utan `--workspace` täcker bara app-craten.

## 4. Arbetsordning (sammanslagen, följ den)

**Våg 0 — grindar, låg risk, gör först (alla S/M, oberoende):**
1. **#352** rustfmt-commit + `cargo fmt --check` i CI — **måste landa före allt annat Rust-arbete**
   (annars rebase-helvete). Koordinera med öppna PR:er #259, #271, #300, #306.
2. **#353** `rust-toolchain.toml`, `rust-version = "1.88"`, Node 22, rätta "Rust 1.75+".
3. **#354** branch-skydd/workflow-härdning (ägaren gör GitHub-inställningarna; du gör YAML-delen).
4. **#357** kör engine-protocol, ORT-host, release-profile-guard och Windows-frontendtester i PR-CI.
5. **#350** (K11) lokal verifiering: fixa dist-/BuildInfo-blockerarna, inför `npm run verify` som CI
   anropar, test-taxonomi `tests/{ui,ci,release}`. Se min kommentar på #350.
6. **#356** clippy-policy med baseline · **#355** cargo-deny/npm audit, reqwest 0.13 ·
   **#382** `ARCHITECTURE.md` + recept + ADR:er (poison-policy, feltyper, teststrategi) ·
   (**#358** CI-dedupe, P2).

**Våg 1 — buggar, lås, kontrakt, testinfra:**
7. **#340** (K01) minimal fix: backup + vägra överskriva oläst settings-fil. Sedan **#365**
   (`schema_version` + migreringsmodul + fixture-korpus).
8. **#359** poison-tolerant `lock_controller()`, säkra alla fyra `Drop`-impl, `catch_unwind` i
   settings-watchern. Besluta policy (rekommendation: återhämta + `error!`-logg) och skriv ADR.
9. **#361** Vitest + testing-library + typad `mockIPC`, ta bort `transpileModule`-laddare och
   källtext-tester, `.js`→`.ts`, ESLint/Prettier. **Före #341/#345/#379.**
10. **#341** (K02) readback-bugg — låt setter-kommandon returnera persisterat `Settings`.
11. **#347** (K08) kontraktsfixtures + mina tillägg: kontraktstest `generate_handler!` ↔
    `invoke`-namn ↔ Playwright-mock, `UiState`-enum + `ui_state::set`, ta bort 20 döda wrappers,
    3 råa eventnamn in i `events.rs`.
12. **#360** typade fel (`CommandError`/`BackendError {code,message}`, `Cancelled`/`Engine`-varianter,
    path-skrubbning, ingen strängmatchning i frontend).
13. **#363** teststödmodul + flytta inline-tester ur gud-filerna till syskonfiler. **Före våg 2.**
14. **#364** CLI `Out`-lager, `ExitCode`, EPIPE-fix (`transcribe --jsonl | head -1` panikar i dag),
    JSON-fel vid preflight.
15. **#366** engine-host: tidsbaserat `Failed` + `reset()`, wedge-watchdog, stdin-timeout,
    PCM-städning, `CREATE_NO_WINDOW`.
16. **#367** Rust↔Swift-protokoll: gyllene JSONL-vektorer, `WireErrorCode`-enum, ta bort
    `serde(default)` på kritiska fält, konformans i CI.
17. **#362** `useTauriEvent` (lyssnarläckor), `models`/`permissions`-stores · **#342** (K03)
    gemensam `publish_new` · (**#368** async-hygien, P2).

**Våg 2 — mekaniska uppdelningar (ren flytt, `git diff --color-moved` ska visa bara flytt):**
18. **#369** `main.rs` → `tray/`, `updater/`, `launch/`, `dictation/` · **#370** `commands.rs` per domän +
    `persist_and_sync` · **#346** (K07) resolver i core (4 call-sites) + **#373** CLI/app-paritet ·
    **#343** (K04) `transcribe/`-katalog + `Source`-enum (se kommentar) · **#371** `whisper/` ·
    **#372** core-layout, `models/`, bryt settings↔transcription · **#374** app→cli-inversion.

**Våg 3 — beteendebärande (karakteriseringstester FÖRE flytt):**
19. **#344** (K05) dikteringspipeline bakom portar + **#375** `hotkey::transition` · **#345** (K06) ·
    **#379** Settings-paneler · **#376** updater-traits · **#377** AppController · **#378**
    `SpeechEngine` · **#348/#349** (K09/K10) Swift-tester · **#380** UI-primitiver/a11y · **#381**
    flake-detektor (+ #320).

**Repo:** **#383** docs-städning · **#384** git-hygien (destruktiva delar bara med ägarens
explicita bekräftelse).

Hårda ordningsberoenden: #352 → allt Rust; #363 → #369/#370/#343/#371; #361 → #341/#345/#379;
#359 → #344; #346 → #343; #342 → #374. Allt annat kan gå parallellt inom sin våg.

## 5. Definition of done per ticket

- Acceptanskriterierna i issuen är uppfyllda och verifierade med kommandon (klistra in utdata
  i PR:n). Grindarna i avsnitt 3 gröna, båda feature-lägena för CLI (`default` och
  `--no-default-features`).
- Regressionstest för buggar; karakteriseringstester före refaktorer; inga källtext-tester.
- PR-beskrivning: vad, varför, hur verifierat, vilken reviewer, avvikelser från ticketen.
  Länka issuen (`Closes #NNN`). Avsluta commit-meddelanden/PR-texter enligt repots
  attributionsregler.
- Rapportera tillbaka per ticket: PR-länk, verifieringsresultat, blockerare, minsta nästa steg.
  Om en ticket visar sig fel eller för stor: skriv det i issuen och föreslå omskärning —
  gissa inte.

## 6. Vad som INTE ingår

Ingen release, tag, signering, notarisering, installation, Dependabot-/GitHub-inställningar
(ägaren), historierensning av git, radering av branches/worktrees, eller beteendeförändringar
i transkriptionskvalitet/modellval. Blanda inte in nya features.
