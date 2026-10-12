# Kvalitetssprint 2026-10-12 — utvärdering och tickets

Revision: `origin/main` @ `a69d5d1` (Sagascript 1.4.4-dev). Utvärderingen gjordes i ett
separat worktree (`claude/quality-sprint-audit`); inget i applikationskoden ändrades.
Syfte: kodkvalitet, stabilitet och ändringsbarhet (lättare att lägga till funktioner,
hitta och tvätta buggar, och lättare för agenter att arbeta i kodbasen).

Dokumentet har fyra delar: (1) sammanfattning, (2) uppmätt baseline med kommandon,
(3) prioriterad ticketlista i vågor, (4) tickets i fulltext redo att filas som
GitHub-issues. Befintliga issues (#178 CI-tid, #258 branch-drift, #319, #320, #332) är
inte duplicerade; tickets refererar till dem där det är relevant.

---

## 1. Sammanfattning

**Läget är bättre än det ser ut, men ändringskostnaden är hög.** Alla deterministiska
kontroller är gröna på `origin/main`: clippy `-D warnings` utan anmärkning, 1 490 Rust-tester
gröna på 54 s, 404 Node-tester, 127 Python-tester, svelte-check 0 fel, `npm audit` 0.
Problemen är strukturella, inte akuta:

1. **Fem gud-filer bär nästan all förändring.** `transcribe.rs` 4 678 rader (en funktion på
   731 rader, cyklomatisk komplexitet 94), `whisper_backend.rs` 3 892, `main.rs` 3 727
   (`main()` ≈ 685 rader), `settings/manager.rs` 3 652, `commands.rs` 3 353 (67 kommandon),
   `Settings.svelte` 3 512 (90 funktioner, ~70 `$state`, 71 commits). Samma filer toppar
   churn-listan och fix-historiken (78 av 227 commits sedan augusti är fixar).
2. **Kontraktet frontend↔backend är stränghanterat i tre lager** (kommandonamn, eventnamn,
   payload-strängar som `"loading_model"`, fel som `Result<_, String>` — 191 i app-craten).
   Inget verktyg fångar drift. 20 av 81 API-wrappers i `api.ts` är döda.
3. **Frontend-testerna låser refaktorering.** 13 testfiler läser `.svelte`-källkod som text
   och asserterar på regex (869 `assert.match`); funktioner klipps ur komponenten med
   AST/brace-matchning och körs i `vm`. Att dela upp `Settings.svelte` gör de flesta röda
   utan beteendeändring.
4. **Ett globalt `Mutex<AppController>` låses på 83 ställen med `unwrap()`**, inklusive i
   fyra `Drop`-impl. Ett panic under lås förgiftar mutexen och appen är död till omstart;
   panic i `Drop` under unwinding ger abort. Logg-flush och audio-start sker under låset.
5. **Verktygsdisciplin saknas där den är billigast.** Ingen `cargo fmt --check` (72 av 128
   Rust-filer är oformaterade, 906 hunks), ingen toolchain-pin (README säger Rust 1.75+,
   beroendeträdet kräver 1.88), Node 20 (EOL april 2026), ingen cargo-deny/audit, inga
   Dependabot-alerts, `check-linux` är inte required, ORT-engine-hosten (23 tester) byggs men
   testas aldrig, `sagascript-engine-protocol` testas inte i PR-CI.
6. **Fresh clone → grönt kräver odokumenterad magi.** `cargo test` panikar tills
   `npm run build` körts (`frontendDist ../dist` saknas); `swift test` misslyckas tills
   `scripts/build-engine-host.sh` genererat `BuildInfo.swift`. CONTRIBUTING, AGENTS.md och
   PR-mallen anger tre olika verifieringskommandon, inget av dem är det CI kör.

**Rekommenderad ordning:** våg 0 (billiga grindar och formatering, en vecka) → våg 1
(testmigrering frontend, lås-/felhantering, kontrakt) → våg 2 (mekaniska uppdelningar av
gud-filer) → våg 3 (beteendebärande refaktoreringar med karakteriseringstester först).

---

## 2. Baseline — mätningar

Alla kommandon kördes 2026-10-12 på `a69d5d1` (macOS, Rust 1.93.1, Node 26). Siffrorna är
avsedda som före-mätning för sprinten; kör samma kommandon efteråt.

### 2.1 Storlek och struktur

| Yta | Filer | Rader | Kommentar |
|---|---:|---:|---|
| Rust (app + 3 crates + engine-host/ort) | 138 | 83 114 | varav ~40 % inline-testkod |
| Svelte | 9 | 7 467 | `Settings.svelte` 3 512 |
| TypeScript / JS i `src/` | 19 + 4 | 2 398 + 839 | 4 `.js`-filer finns för att testladdaren inte klarar relativa TS-importer |
| Node-skript (`scripts/*.mjs`) | 84 | 15 338 | 53 är tester |
| Python (`scripts/dictation_eval`, `diarization_sv`, …) | 74 | 11 150 | |
| Swift (`engine-host/coreml`) | 13 | 2 934 | |
| Workflows (`.github/workflows`) | 10 | 2 811 | `ci.yml` 709 |
| Dokumentation (`docs/**/*.md` + toppnivå) | 52 | — | 12 föräldralösa, 6 historiska |

Största källfiler (rader): `cli/transcribe.rs` 4 678 · `core/transcription/whisper_backend.rs`
3 892 · `src/main.rs` 3 727 · `core/settings/manager.rs` 3 652 · `src/commands.rs` 3 353 ·
`core/download.rs` 2 192 · `core/settings/store.rs` 2 175 · `cli/diarization.rs` 2 084 ·
`cli/config.rs` 1 691 · `src/app_controller.rs` 1 573 · `cli/lib.rs` 1 520 ·
`core/transcription/pianissimo_model.rs` 1 510 · `core/audio/decoder.rs` 1 492 ·
`cli/diarization_cache.rs` 1 465 · `core/transcription/engine_host/client.rs` 1 275.

Churn (commits som rört filen sedan 2026-06): `Settings.svelte` 60+ · `main.rs` 49 ·
`commands.rs` 43 · `cli/lib.rs` 31 · `cli/transcribe.rs` 27 · `whisper_backend.rs` 21 ·
`api.ts` 20. Fix-andel: 78 av 227 commits sedan 2026-08-01 (34 %). Median-PR: 328 rader;
sju PR:er över 3 900 rader (exkl. fixtures/lockfiles).

### 2.2 Deterministiska kontroller (origin/main)

| Kontroll | Resultat | Tid |
|---|---|---|
| `cargo clippy --workspace --all-targets -- -D warnings` | 0 varningar | 8 s (varm cache) |
| `cargo clippy -p sagascript-cli --all-targets --no-default-features -- -D warnings` | 0 varningar | 40 s |
| `cargo test --workspace` | 1 490 godkända, 0 fel, 2 ignorerade (kräver modeller) | 54 s |
| `cargo fmt --all -- --check` | **906 hunks i 72 av 128 filer** | — |
| `npx svelte-check` | 0 fel, 0 varningar, 150 filer | 6 s |
| `npm run test:frontend` | 404 tester: 394 ok, 10 skippade | 13 s |
| `npm run test:code-health` | 35 ok | — |
| `python -m unittest discover -s scripts/dictation_eval` | 127 ok | 2 s |
| `swift test` (engine-host/coreml) | **byggfel: `BuildInfo` saknas** tills `scripts/build-engine-host.sh` körts | — |
| `cargo test`/`clippy` i färskt worktree | **panic i `tauri::generate_context!`** tills `npm run build` körts | — |
| `npm audit` | 0 sårbarheter | — |
| cargo-audit / cargo-deny | inte installerade, inte i CI | — |

Testfördelning: app-crate 282 · cli 322 + 38 integration · core 778 + 57 integration ·
engine-protocol 15. Två `#[ignore]` kräver lokala modeller; fyra fler i `windows_dictation.rs`
och `windows_clipboard_tests.rs`.

### 2.3 Clippy-census (produktionskod, tester exkluderade via `clippy.toml`)

Kört med `-A clippy::all` plus utvalda lints, `allow-unwrap-in-tests = true`:

| Lint | Träffar | Hotspots |
|---|---:|---|
| `unwrap_used` | 255 | `commands.rs` 51, `main.rs` 36, `engine_host/client.rs` 35, `whisper_backend.rs` 27, `engine_host/process.rs` 18, `plain_file_jobs.rs` 14, `audio/capture.rs` 11 |
| `expect_used` | 61 | |
| `panic` | 4 | |
| `too_many_lines` (>150) | 14 funktioner | `cli/transcribe.rs:1512` **731 rader** · `main.rs:967` 583 (`main()`) · `main.rs:2124` 393 · `settings/hotkey.rs:75` 329 · `diarization_qualification.rs:128` 278 · `commands.rs:2624` 268 · `benchmark_dictation.rs:149` 217 · `transcribe.rs:798` 208 · `transcribe.rs:1100` 205 · `meeting_jobs.rs:133` 183 |
| `cognitive_complexity` (>30) | 2 | `cli/transcribe.rs:1512` 40 · `diarization_qualification.rs:128` 33 |
| `await_holding_lock` | 0 | bra |
| `needless_pass_by_value` | 55 | |
| `#[allow(dead_code)]` | 28 st | `hotkey/service.rs`, `platform/{linux,windows}.rs`, `app_controller.rs` (`paste`, `auto_paste`), `logging/log_events.rs` |
| `#[allow(clippy::too_many_arguments)]` | 16 st | |

Lås: `lock().unwrap()`/`lock().expect()` 133 i app-craten (commands.rs 47, main.rs 36,
plain_file_jobs 15, update_activity 11), 117 i core (varav engine_host/client.rs 33,
process.rs 17, audio/capture.rs 11). Endast `meeting_jobs.rs` och `meeting_media.rs` hanterar
poison. `Result<_, String>`: 191 i app, 56 i core, 31 i cli.

Tidigare mätning (docs/research/code-health-2026-10-09, `rust-code-analysis`,
`cargo llvm-cov`): `transcribe_file` cyklomatisk 94 / kognitiv 85; core-täckning 89,6 %
rader (inkl. tester), `whisper_backend.rs` 66 %; CLI `transcribe.rs` 55 %; Swift
`CoreMLEngine.swift` 1,3 %, `TdtDecoder.swift` 18 %.

### 2.4 Kontrakt frontend ↔ backend

- 88 `#[tauri::command]` registrerade = 88 anropade från `src/` (ingen drift i namnen).
- 20 av 81 wrappers i `api.ts` anropas aldrig (`setWhisperModel`, `isModelReady`,
  `cancelRecording`, `getLoadedModel`, …).
- Eventnamn: `events.rs` har konstanter, men `"update-preparing"`, `"update-aborted"` och
  `"navigate_tab"` (snake_case) emitteras som råsträngar (`main.rs:479,621,641,2051`), och
  `"transcription-progress"` emitteras både som konstant och råsträng (`commands.rs:1244`).
  `TRANSCRIPTION_PHASE` emitteras (`commands.rs:2731,2898`) men har ingen lyssnare i `src/`.
- Frontend lyssnar med 16 strängliteraler i 6 filer; `state-changed`-payload valideras ad hoc
  mot `["idle","recording","loading_model","transcribing"]` (`Settings.svelte:1162`), och
  backend skickar även `"settings_reloaded"` på samma event (`main.rs:2781`).
- `invoke` anropas aldrig med typparameter; returtyper är oprövade påståenden. `: any` 29 st.
- Kontroll av felmeddelanden används som kontrollflöde: `startsWith("Dictionary changed
  elsewhere:")` 6 ställen, `includes("failed to open file:")`.

### 2.5 CI och repo-hygien

- Required checks på `main` (ruleset "Protect main"): `check-macos`, `check-windows`.
  **`check-linux` saknas** → Linux-GUI-kompilering, no-ALSA-grinden och diarization-Python-
  kontrakten är rådgivande. 0 required reviews. `delete_branch_on_merge: false`.
  Dependabot vulnerability alerts: **avstängda**.
- `ci.yml` 709 rader: setup-preamble kopierad i 5 jobb, 8 identiska cargo-steg macOS/Windows,
  4 identiska smoke-block (bash/PowerShell), 26 `cargo`-steg. Regex-tester
  (`test-ci-cli-feature-coverage.mjs`, `test-ci-parallel-gates.mjs`) låser kopiorna.
- Ingen PR-lane kör `cargo test --workspace` → `sagascript-engine-protocol` (15 tester)
  testas bara i release-gate. `engine-host/ort` (eget workspace, 23 tester) byggs i
  `windows-package.yml` men testas/lintas aldrig. `scripts/engine-host-conformance.py` är
  dokumenterat som obligatoriskt men körs inte.
- `sbom.yml`: `permissions: contents: write` på varje push till main, `cargo install
  cargo-sbom` och `npm install -g @cyclonedx/cdxgen` opinnade.
- Toolchain: ingen `rust-toolchain.toml`, ingen `rust-version`; beroenden kräver ≥ 1.88 (`ort`,
  `time`), docs säger 1.75+. Node 20 i 11 workflows (EOL 2026-04); `site/` kräver Node ≥22.13.
- Beroenden: 713 paket i lockfilen, 59 crates i flera versioner; `reqwest 0.12` (vår) +
  `0.13` (tauri) = två HTTP-stackar. `[patch.crates-io]` git-pin av `global-hotkey`.
- Git: 111 remote-branches (36 redan mergade i main, 18 utan commits på 60 dagar), 259 lokala
  branches, 89 worktrees varav 52 prunable; lokal `main` låg 54 commits efter `origin/main`.
  Packfil 279 MB, varav 3 × 114 MB WAV i historiken (`flowdictate-tauri/benchmark/audio/`).
- CI-tid: `ci.yml` på main 8 min normalt, 16–20 min vid kall cache; release 22–31 min.
  Första-försök-fel på PR senaste 30 körningar: 2 fel, 2 avbrutna, 26 gröna.

### 2.6 Dokumentation

- README listar 7 av 47 docs. Föräldralösa: `meeting-reprocessing.md`, `meeting-review-contract.md`,
  `meeting-review-usage.md`, `meeting-workflow.md`, `personal-glossary.md`,
  `testing/multifile-transcription.md`, tre benchmark-docs, tre research-docs.
- Historiska/pensionerade utan läsare: `presenter-mode-design.md` (278 rader, funktionen togs
  bort i #217), `windows-port-plan.md` (827 rader, motsäger `windows-release.md`),
  `ci-performance.md` (ersatt av `ci-performance/`), `transcribe-review.md` och
  `testing/integrated-pr-review.md` (PR-loggar), `glossary-scoping-decision.md` (ersatt),
  `INSTALLING.md` (tredje installationsguiden).
- Verifieringskommandon skiljer sig mellan AGENTS.md, CONTRIBUTING.md och PR-mallen
  (mallen säger bara `cargo test`, som CONTRIBUTING uttryckligen varnar för). Ingen av dem nämner
  `test:frontend`, `test:browser`, `test:code-health`, Python- eller Swift-testerna.
  AGENTS.md/CONTRIBUTING saknar `engine-host/`, `sagascript-engine-protocol`, `tests/`, `tooling/`.
- Det finns ingen arkitekturbeskrivning av dataflödena (hotkey → record → transcribe → paste,
  settings-lager, updater, engine-host) och inga "så här lägger du till ett kommando / en
  inställning / ett CLI-subkommando"-recept.

---

## 3. Prioriterad ticketlista

Prio: P0 = gör först, låser upp annat · P1 = sprintens kärna · P2 = bör göras i sprinten om
tid finns · P3 = backlog. Effort: S < 1 dag · M 1–3 dagar · L > 3 dagar (för en agent med
review). "Beror på" anger ordning, inte hårda blockeringar om inget annat sägs.

### Våg 0 — grindar och hygien (vecka 1, nästan ingen risk)

| ID | Ticket | Prio | Effort | Beror på |
|---|---|---|---|---|
| T01 | Formatera hela Rust-koden med rustfmt och inför `cargo fmt --check` i CI | P0 | S | — |
| T02 | Pinna toolchains: `rust-toolchain.toml`, `rust-version`, Node 22, rätta "Rust 1.75+" | P0 | S | — |
| T03 | Branch-skydd och workflow-härdning (check-linux required, permissions, timeouts, SBOM-token, Dependabot) | P0 | S | — |
| T04 | Beroendepolicy: cargo-deny + npm audit i CI, en HTTP-stack (reqwest 0.13), granska git-patch | P1 | M | T02 |
| T05 | Fresh clone → grönt med ett kommando (`npm run verify`), och ett enda sanningens verifieringsrecept | P0 | M | — |
| T06 | Clippy-lintpolicy för workspace med baseline (unwrap/expect/panic i produktionskod, långa funktioner) | P1 | S | T01 |
| T07 | Kör testsviterna som aldrig körs i PR-CI (engine-protocol, ORT-host, release-profile-guard, Windows frontend) | P0 | S | — |
| T08 | Test-taxonomi: dela `scripts/test-*.mjs` i ui/ci/release, sluta dubbelköra, fixa glob-undantag | P2 | M | — |
| T09 | Avduplicera `ci.yml` (composite action + gate-skript) och ersätt regex-testerna på YAML | P2 | M | T07, T08 |

### Våg 1 — kontrakt, felhantering, lås och testinfrastruktur (gör refaktorering säker)

| ID | Ticket | Prio | Effort | Beror på |
|---|---|---|---|---|
| T10 | Poison-tolerant låshjälpare, säkra `Drop`-impl och watcher-tråd i app-craten | P0 | S | — |
| T11 | Typat IPC-kontrakt frontend↔backend: eventkonstanter, `UiState`, `CommandMap`/`EventMap`, kontraktstest | P1 | M | — |
| T12 | Typade fel över gränserna: `CommandError`/`BackendError {code,message}`, `Cancelled`/`Engine`-varianter, path-skrubbning | P1 | M | T11 |
| T13 | Frontend-testmigrering till Vitest + testing-library + typad `mockIPC`; ESLint + Prettier | P0 | M/L | — |
| T14 | Frontend-delade stores/hjälpare: `useTauriEvent` (lyssnarläckor), `models`, `permissions`, `toErrorMessage` | P1 | M | T13 |
| T15 | Delad Rust-teststödmodul (TempDir, fixtures, sha256) och flytta inline-tester i gud-filer till syskonfiler | P1 | M | T01 |
| T16 | CLI utdata- och exitlager: `Out`-writer, `ExitCode`, `CliError`, EPIPE, strukturerade JSON-fel, en `BatchSink` | P1 | M | — |
| T17 | Settings-robusthet: backup vid typad parse-miss, `schema_version` + ordnade migreringar i en modul | P1 | M | — |
| T18 | Engine-host-övervakning: tidsbaserat `Failed`, wedge-watchdog, stdin-timeout, PCM-städning, `CREATE_NO_WINDOW` | P1 | M | — |
| T19 | Engine-host-protokoll maskinkontrollerat: gyllene JSONL-vektorer, Swift `WireErrorCode`, konformans i CI | P1 | M | T07 |
| T20 | Async-hygien och nedladdningshärdning i core (`spawn_blocking`, `sync_all`, ev. Range-resume) | P2 | S/M | — |

### Våg 2 — mekaniska uppdelningar (ren flytt, inga beteendeändringar)

| ID | Ticket | Prio | Effort | Beror på |
|---|---|---|---|---|
| T21 | Dela `src/main.rs` i `tray/`, `updater/`, `launch/` och ett tunt `main.rs` | P1 | M | T01, T10 |
| T22 | Dela `src/commands.rs` per domän + `persist_and_sync`-hjälpare + flytta `macos_mic` till `platform/` | P1 | M | T01, T10 |
| T23 | Dela `cli/transcribe.rs` till `transcribe/` (args, inputs, vocab, language_regions, resolve, output, meeting) | P1 | M/L | T15, T16 |
| T24 | Dela `whisper_backend.rs` till `whisper/`, slå ihop `context`/`loaded_runtime`, `TranscribeRequest`, rätt feature-flagga | P1 | M/L | T15 |
| T25 | Koherent modullayout i core: `meeting/`, `diarization/{eval}`, `models/` ur `settings/manager.rs`, bryt settings↔transcription-cykeln | P1 | M/L | T17 |
| T26 | CLI/app-paritet: `RunContext`, `Settings`-operationer, tabellstyrt config-nyckelregister, `WhisperModel::from_id`, `--json` överallt | P1 | M | T16 |
| T27 | Bryt beroendeinversionen app → cli: flytta mötes-/diariseringspipeline, cache, proposal-I/O till core | P2 | L | T23, T25, T26 |

### Våg 3 — beteendebärande refaktoreringar (karakteriseringstester först)

| ID | Ticket | Prio | Effort | Beror på |
|---|---|---|---|---|
| T28 | Dikteringspipeline bakom portar (`Transcriber`, `PasteTarget`, `UiSink`, `Clock`); avduplicera timeout/abort, paste, perf-logg | P1 | L | T21, T22, T11 |
| T29 | `hotkey::transition` — en tillståndsmaskin för registrering/rollback i stället för två kopior | P1 | M | T22 |
| T30 | Updater-orkestrering bakom traits (`UpdateProvider`, `MenuSink`, `Recovery`), `CheckingGuard`, veckovis signerad smoke | P2 | M | T21, PR #259 |
| T31 | `AppController`-nedbrytning: telemetri ut ur låset, döda fält bort, settings-snapshot | P2 | M/L | T28 |
| T32 | `SpeechEngine`-trait för Whisper + Pianissimo, en avbrytningsmodell, `EngineAvailability` i stället för 22 anrop | P2 | L | T24, T12 |
| T33 | Dela `Settings.svelte` i flikpaneler, `GlossaryEditor`, `UpdateRecoveryController`, flikregister | P1 | L | T13, T14, T11 |
| T34 | UI-primitiver och CSS-konsolidering (Tabs, Modal på `<dialog>`, Notice, Spinner, tokens, reduced-motion) | P2 | M | T33 |
| T35 | Testtillförlitlighet: väggklocksgränser bort, env-mutation i parallella tester, nattlig flake-detektor, #320 | P2 | M | T15 |

### Dokumentation, repo och agent-ergonomi

| ID | Ticket | Prio | Effort | Beror på |
|---|---|---|---|---|
| T36 | `ARCHITECTURE.md` + recept ("lägg till kommando/inställning/subkommando/event") + uppdaterad `AGENTS.md` + ADR-katalog | P0 | M | — |
| T37 | Dokumentstädning: arkivera pensionerade docs, länka föräldralösa, en installationsguide, döda skript, bredare docs-only-allowlist | P2 | S | — |
| T38 | Git-hygien (engångs, ägaren kör): prune 52 worktrees, radera 36 mergade branches, ff lokal main, beslut om 340 MB WAV i historiken | P2 | S | — |

---

## 4. Tickets i fulltext

Varje ticket är skriven för att kunna klistras in som GitHub-issue. Radreferenser gäller
`a69d5d1`. Acceptanskriterierna är deterministiska där det går.

### T01 · Formatera Rust-koden med rustfmt och gör `cargo fmt --check` till en grind

**Labels:** `area:quality`, `package:ci`, `rust` · **Prio:** P0 · **Effort:** S

**Problem.** `cargo fmt --all -- --check` ger 906 hunks i 72 av 128 Rust-filer; ingen commit i
historiken har någonsin formaterat koden, och CI saknar `fmt --check`. CONTRIBUTING.md:52
säger ändå "Follow standard rustfmt formatting". Varje PR som råkar formatera en fil skapar
brus-diffar och mergekonflikter; agenter som kör `cargo fmt` av vana rör hundratals rader.

**Omfattning.**
1. Lägg till `src-tauri/rustfmt.toml` (minst `edition = "2021"`; inga exotiska val) och
   `.editorconfig`.
2. En ren commit `style: rustfmt the workspace` som bara formaterar: `cargo fmt --all` i
   `src-tauri/` och `cargo fmt` för `src-tauri/engine-host/ort/` (eget workspace).
3. CI-steg `cargo fmt --all -- --check` (båda workspaces) i `test-linux` (billigast) och i
   `release-quality-gate.yml`.
4. Notera i PR-beskrivningen att öppna PR:er (#259, #271, #300, #306) behöver rebase med
   `git rebase -X theirs`-liknande strategi eller `cargo fmt` på sin sida; koordinera så att
   formateringen landar i ett lugnt fönster.

**Acceptans.**
- `cargo fmt --all -- --check` returnerar 0 i båda workspaces på `main`.
- CI blockerar en PR som introducerar oformaterad kod (verifiera med en test-PR).
- Diffen i formateringscommiten innehåller inga semantiska ändringar (`cargo test --workspace`
  och clippy gröna, `git diff --stat` visar bara whitespace/radbrytningar).

**Utanför scope.** Lint-ändringar (T06), Prettier för frontend (T13).

### T02 · Pinna toolchains och rätta MSRV-påståendet

**Labels:** `package:ci`, `documentation` · **Prio:** P0 · **Effort:** S

**Problem.** Ingen `rust-toolchain.toml`, ingen `rust-version` i `Cargo.toml`; PR-CI använder
flytande `dtolnay/rust-toolchain@stable` medan `code-health.yml` pinnar 1.93.1. README:214 och
CONTRIBUTING.md:11 lovar "Rust 1.75+", men beroendeträdet kräver ≥ 1.88 (`ort`, `time`).
Node 20 används i 11 workflow-ställen trots EOL 2026-04; `site/` kräver Node ≥ 22.13.

**Omfattning.** `rust-toolchain.toml` (channel pinnad till den version CI kör, komponenter
`clippy, rustfmt`), `[workspace.package] rust-version = "1.88"` (verifiera med
`cargo +1.88 check --workspace` eller sätt till lägsta som bygger), `.nvmrc` + `engines` i
`package.json` med Node 22, byt `node-version: 20` → 22 i alla workflows, uppdatera README och
CONTRIBUTING. Lägg ett CI-jobb som bygger med MSRV (`cargo check --workspace` på
`rust-version`) så att påståendet förblir sant.

**Acceptans.** `rg 'node-version: 20' .github` tomt; `cargo metadata` visar `rust_version`;
MSRV-jobbet grönt; README/CONTRIBUTING anger samma versioner som filerna.

### T03 · Branch-skydd och workflow-härdning

**Labels:** `package:ci`, `security` · **Prio:** P0 · **Effort:** S (delar är GitHub-inställningar som ägaren gör)

**Problem (verifierat via GitHub API 2026-10-12).** Ruleset "Protect main" kräver bara
`check-macos` och `check-windows`; `check-linux` är rådgivande, så Linux-GUI-kompilering,
no-ALSA-grinden och diarization-Python-kontrakten kan vara röda utan att blockera.
`delete_branch_on_merge` är av (36 mergade remote-branches ligger kvar). Dependabot
vulnerability alerts är avstängda. `ci.yml` saknar `permissions:` (default skrivtoken) och
`timeout-minutes` (default 360). `sbom.yml` kör med `contents: write` på varje push till main
och installerar `cargo-sbom` och `@cyclonedx/cdxgen` opinnade med den tokenen.

**Omfattning.**
- Ruleset: lägg till `check-linux` som required (behåll aggregator-mönstret; path-filtrerade
  workflows ska inte vara required).
- Repo-inställningar: `delete_branch_on_merge: true`; aktivera Dependabot alerts + security
  updates; lägg `.github/dependabot.yml` för `cargo`, `npm`, `github-actions` med månatlig,
  grupperad schemaläggning så att PR-floden är hanterbar.
- `ci.yml`: toppnivå `permissions: { contents: read }`, `timeout-minutes` per jobb (60 för
  native, 20 för övriga). Samma i `code-health.yml` och `windows-package.yml`.
- `sbom.yml`: `contents: read` på jobbet, `write` bara i release-attach-steget; pinna
  verktygsversioner (`cargo install --locked cargo-sbom --version X`, `npm i -g
  @cyclonedx/cdxgen@X`).
- Pinna actions konsekvent (SHA + versionkommentar) i alla workflows, som `code-health.yml`
  redan gör.

**Acceptans.** `gh api repos/.../rulesets/13091396` listar tre required checks; `gh api
repos/.../vulnerability-alerts` svarar 204; varje workflow har `permissions` och
`timeout-minutes`; `rg 'uses: .*@v[0-9]+$' .github` tomt.

### T04 · Beroendepolicy i CI och färre dubbla stackar

**Labels:** `package:ci`, `rust`, `security` · **Prio:** P1 · **Effort:** M · **Beror på:** T02

**Problem.** 713 paket i `Cargo.lock`, 59 crates i flera versioner. Första parts `reqwest 0.12`
(root + core) duplicerar `reqwest 0.13` från `tauri 2.10`/`tauri-plugin-updater` — två
HTTP/TLS-stackar i binären. `[patch.crates-io]` pinnar `global-hotkey` till en git-rev med en
kommentar om att släppa patchen när en release finns. Ingen cargo-audit/cargo-deny, ingen
`npm audit` i CI; SBOM genereras men kontrolleras inte.

**Omfattning.** `src-tauri/deny.toml` (advisories, bans med `multiple-versions = "warn"` initialt,
licenser enligt `THIRD_PARTY_NOTICES`, sources = crates.io + den enda tillåtna git-källan) och
`cargo deny check` i `test-linux`; `npm audit --audit-level=high` i macOS-lanen; bump
`reqwest` till 0.13 i root och core och verifiera nedladdningstesterna; kontrollera om
`global-hotkey` ≥ 0.8.1 innehåller #216 och ta bort patchen om så. Lämna transitiva dubbletter.

**Acceptans.** `cargo deny check` grönt i CI; `cargo tree -d | rg '^reqwest'` visar en version;
`THIRD_PARTY_NOTICES.md` regenererat (`npm run licenses:check` grönt).

### T05 · Fresh clone → grönt med ett kommando, och ett enda verifieringsrecept

**Labels:** `area:quality`, `documentation`, `package:ci` · **Prio:** P0 · **Effort:** M

**Problem.** I ett färskt worktree panikar `cargo test`/`cargo clippy` i
`tauri::generate_context!()` ("frontendDist ../dist doesn't exist") tills `npm run build`
körts; `swift test` misslyckas med "cannot find BuildInfo" tills `scripts/build-engine-host.sh`
genererat den gitignorerade `BuildInfo.swift`. Inget av detta står i CONTRIBUTING. AGENTS.md,
CONTRIBUTING.md och `.github/PULL_REQUEST_TEMPLATE.md` anger tre olika verifieringslistor;
mallen säger bara `cargo test` (app-craten), vilket CONTRIBUTING:20 uttryckligen varnar för.
Ingen av dem nämner `test:frontend`, `test:browser`, `test:code-health`, Python- eller
Swift-testerna, `release:check` eller `licenses:check`. Agenter gissar därför vilka kontroller
som räcker.

**Omfattning.**
1. Gör Rust-workspacet byggbart utan frontend: antingen låt `build.rs` skapa en tom
   `dist/index.html`-placeholder när katalogen saknas (och dokumentera att riktiga builds
   kör `npm run build` först), eller peka `frontendDist` på en incheckad minimal placeholder
   för `cargo test`. Verifiera att `cargo tauri build` fortfarande använder riktig dist.
2. Gör `swift test` självförsörjande: generera `BuildInfo.swift` via SwiftPM build-plugin
   eller lägg en incheckad default `BuildInfo.swift` som skriptet skriver över i release
   (`check-engine-host-identity.sh` ska fortfarande fånga saknad SHA i release).
3. Ett kommando som kör exakt det CI kör: `npm run verify` (eller `make verify`) =
   fmt-check, clippy-matrisen, `cargo test --workspace`, ORT-host-tester, svelte-check,
   `test:frontend`, `test:code-health`, Python-tester, `swift test`; plus `npm run verify:fast`
   för inre loopen. Låt CI anropa samma skript så listorna inte kan glida isär.
4. Skriv om CONTRIBUTING "Running tests", AGENTS.md "Verification" och PR-mallen till att
   peka på det kommandot. Lägg till en "Fresh clone"-sektion med de två fallgroparna ovan.

**Acceptans.** `git clone && npm ci && npm run verify` grönt på en ren macOS-maskin utan
manuella steg; `cargo test --workspace` grönt utan `dist/`; `swift test` grönt utan
förgenererad fil; de tre dokumenten citerar samma kommando.

### T06 · Clippy-lintpolicy för workspace med mätt baseline

**Labels:** `area:quality`, `rust` · **Prio:** P1 · **Effort:** S · **Beror på:** T01

**Problem.** `[workspace.lints]` innehåller bara `unexpected_cfgs`. Produktionskoden har
(klippy-census med tester exkluderade) 255 `unwrap_used`, 61 `expect_used`, 4 `panic`,
14 funktioner > 150 rader, 55 `needless_pass_by_value`, 28 `#[allow(dead_code)]`,
16 `#[allow(clippy::too_many_arguments)]`. Inget hindrar att siffrorna växer.

**Omfattning.** `src-tauri/clippy.toml` med `allow-unwrap-in-tests = true`,
`allow-expect-in-tests = true`, `too-many-lines-threshold = 150`,
`cognitive-complexity-threshold = 30`. `[workspace.lints.clippy]`: `unwrap_used`,
`expect_used`, `panic`, `await_holding_lock`, `too_many_lines`, `cognitive_complexity`,
`dbg_macro`, `print_stdout`/`print_stderr` (med `#[allow]` på CLI:ns utdata-lager, T16) som
`warn`. Dokumentera baseline-siffrorna i ticketen; sätt `deny` per crate när en crate når noll
(engine-protocol kan gå till deny direkt). Ta bort `#[allow(dead_code)]` som sitter på `pub`
-funktioner i `pub mod` (de gör inget) och radera funktionerna som faktiskt är döda
(`AudioCaptureService::last_captured_audio`, `HotkeyMode::display_name`,
`AppController::auto_paste`/`paste`-fältet, `hotkey/service.rs`).

**Acceptans.** Clippy-körningen i CI rapporterar antalet warnings per lint; antalet får inte öka
(enkelt skript som jämför mot baseline-fil i repot); `rg '#\[allow\(dead_code\)\]' src-tauri`
≤ 10 med motivering i kommentar vid varje.

### T07 · Kör testsviterna som aldrig körs i PR-CI

**Labels:** `package:ci`, `bug` · **Prio:** P0 · **Effort:** S

**Problem.** Ingen PR-lane kör `cargo test --workspace`, så `sagascript-engine-protocol`
(15 tester, Rust↔Swift-trådformatet) testas bara i release-gate. `src-tauri/engine-host/ort`
är ett eget Cargo-workspace med ~33 tester (inkl. paritetstestet mot
`engine-host/test-vectors/context-biasing.json`) som byggs i `windows-package.yml:344` men
aldrig testas eller lintas. Release-profilens säkerhetstest `dev_overrides.rs:73-79`
(`release_build_ignores_the_real_environment`) kompileras bort i `cargo test` och körs av
inget workflow. Windows-lanen kör bara 4 handplockade frontend-tester (`ci.yml:543`) och
modell-smoke-stegen har `continue-on-error: true` (`ci.yml:668,677`).

**Omfattning.** I `test-linux`: `cargo test -p sagascript-engine-protocol`,
`cargo test --manifest-path src-tauri/engine-host/ort/Cargo.toml` + clippy + fmt (bygger utan
ORT-binär tack vare `load-dynamic`; verifiera), `cargo test --release -p sagascript-core --lib
dev_overrides`. I `test-windows`: hela `npm run test:frontend`. Ta bort `continue-on-error`
på Windows-smoke eller flytta den till ett nattligt jobb som öppnar en issue vid fel (beslut
i ticketen). Uppdatera `scripts/test-ci-cli-feature-coverage.mjs` som annars låser stegen.

**Acceptans.** Workflow-loggar visar att de fyra sviterna körs på PR; en avsiktlig
testbrytning i engine-protocol blockerar PR:n.

### T08 · Test-taxonomi för Node-testerna

**Labels:** `area:quality`, `package:ci` · **Prio:** P2 · **Effort:** M

**Problem.** `scripts/run-frontend-tests.mjs` globbar alla 53 `scripts/test-*.mjs` till en
`node --test`-körning; ungefär hälften är UI-state-tester, hälften workflow-/release-/Windows-
kontraktstester som läser YAML som text. `test_reference_review.mjs` (understreck) missas av
globben och körs för hand i `ci.yml:121`. `test-code-health-*.mjs` körs två gånger (i globben
och i `test:code-health`); `code-health.yml` spenderar en macOS-runner per PR på Node-tester.
`tests/scripts/` är en tredje plats.

**Omfattning.** Flytta till `tests/ui/`, `tests/ci/`, `tests/release/` (+ befintliga
`tests/scripts` in i samma schema); npm-skript per kategori (`test:ui`, `test:ci`,
`test:release`) och `test:node` som kör alla; `run-frontend-tests.mjs` ersätts av `node --test
tests/**/*.test.mjs`; code-health-PR-jobbet körs på `ubuntu-24.04`. Byt namn på de YAML-
läsande testerna så att det syns vad de är.

**Acceptans.** Varje testfil körs exakt en gång i en CI-körning; `npm run test:ui` kör bara
tester som importerar `src/`; inga testfiler kvar i `scripts/`.

### T09 · Avduplicera `ci.yml` och ersätt regex-testerna på YAML

**Labels:** `package:ci` · **Prio:** P2 · **Effort:** M · **Beror på:** T07, T08 · **Relaterat:** #178

**Problem.** `ci.yml` (709 rader) upprepar setup-preamblen (checkout, python, rust, node,
rust-cache, npm ci) i 5 jobb, 8 identiska cargo-steg i macOS/Windows, 4 identiska smoke-block
(bash/PowerShell), och `cargo check` körs redundant före clippy. Kopiorna glider redan isär
(Windows saknar diarization-matris och JSON-stream-verifiering). `test-ci-cli-feature-
coverage.mjs:35-47` och `test-ci-parallel-gates.mjs` asserterar på exakta `run:`-strängar och
låser kopiorna. `windows-package.yml` kör `cargo test --workspace` och bygger Windows-binären
en gång till på varje PR som rör `src/**`.

**Omfattning.** Composite action `.github/actions/setup` för preamblen; `scripts/ci/cargo-
gates.sh` (eller en matris) som är den enda listan över cargo-kommandon och anropas av alla
tre OS; en smoke-skriptfil per OS i stället för inline; bestäm om `test-windows` eller
`windows-package` äger Windows-testerna. Skriv om YAML-regex-testerna till strukturella
kontroller (parse YAML, kontrollera att gate-skriptet anropas och att required-aggregatorerna
finns). Mät runner-minuter före/efter.

**Acceptans.** `ci.yml` < 350 rader; samma cargo-kommandon körs på alla OS enligt loggarna;
inga `assert.match(ciYaml, …)` kvar i testerna; PR-runner-minuter minskade (siffra i PR).

### T10 · Poison-tolerant låshjälpare, säkra `Drop`-impl och watcher-tråd

**Labels:** `bug`, `rust`, `area:dictation`, `priority:P0` · **Prio:** P0 · **Effort:** S

**Problem.** `pub type SharedController = Mutex<AppController>` (`commands.rs:404`) låses med
`.lock().unwrap()` på 83 ställen (commands.rs 47, main.rs 36), inklusive i global-shortcut-
callbacken (`main.rs:845,853,923`). Under låset görs audio-start/stop
(`app_controller.rs:431,554`) och synkron loggskrivning + flush (`logging/service.rs:210-217`).
`Cargo.toml:107-109` väljer medvetet bort `panic = "abort"` så att panics unwindar — men en
panic med låset taget förgiftar mutexen och därefter panikar varje kommando och varje
hotkey-tryck tills appen startas om (samma bugg-klass som tray-UTF-8-fallet i juli).
Fyra `Drop`-impl låser med `unwrap()` (`commands.rs:716-718`, `update_activity.rs:143-153`,
`plain_file_jobs.rs:31-33`); panic i `Drop` under unwinding = dubbel panic = abort.
Settings-watchern (`main.rs:2616`) körs i `std::thread::spawn` utan `catch_unwind`; en panic
där (t.ex. `main.rs:2684`) dödar hot-reload tyst. I core: `engine_host/client.rs` 33,
`process.rs` 17, `audio/capture.rs` 11 `lock().unwrap()`; `whisper_backend.rs:1477`
(`lock_state_bounded`) panikar vid poison mitt i inferens. Policyn är inkonsekvent:
`meeting_jobs.rs`, `meeting_media.rs`, `dev_overrides.rs` och delar av `pianissimo_backend.rs`
hanterar poison, resten inte. Hardening-auditen i juli sköt medvetet upp "blanket poison
recovery" som ett separat beslut — detta är det beslutet.

**Omfattning.**
1. Besluta och dokumentera policyn (ADR i T36): rekommendation = återhämta
   (`unwrap_or_else(PoisonError::into_inner)`) med `tracing::error!` som bär panic-
   meddelandet, eftersom tillståndet i `AppController` är idempotent-återställbart och
   alternativet (abort) är sämre för en menyradsapp.
2. `fn lock_controller(&SharedController) -> MutexGuard<AppController>` i app-craten och en
   generisk `lock_or_recover` i core (eller byt till `parking_lot`, som inte förgiftar);
   mekanisk ersättning av alla sites. `Drop`-impl använder alltid den tåliga varianten.
3. Wrappa watcher-loopens kropp i `catch_unwind`, logga och fortsätt; lägg en hälsoflagga som
   Settings-UI kan visa.
4. Regressionstester: (a) förgifta låset i ett test och verifiera att nästa kommando lyckas,
   (b) panic i `Drop`-vägen ger inte abort, (c) watcher överlever en injicerad panic.

**Acceptans.** `rg 'lock\(\)\.unwrap\(\)' src-tauri/src` = 0 utanför tester;
`rg 'lock\(\)\.unwrap\(\)' src-tauri/crates/sagascript-core/src` ≤ 5 med kommentar;
de tre testerna finns och är gröna; ADR länkad från AGENTS.md.

**Utanför scope.** Att ta bort I/O ur låset (T31), lock-ordning i `WhisperBackend` (T24).

### T11 · Typat IPC-kontrakt frontend ↔ backend

**Labels:** `area:quality`, `frontend`, `rust` · **Prio:** P1 · **Effort:** M

**Problem.** Tre stränglager utan verktygsstöd: (1) kommandonamn — 88 `invoke("…")` utan
typparameter, returtyper är oprövade påståenden, `Settings` m.fl. i `api.ts:32-52` är
handkopior av Rust-structs, argumentnycklar måste matcha Rust-parameternamn i camelCase;
(2) eventnamn — `events.rs` har konstanter men `"update-preparing"`, `"update-aborted"`,
`"navigate_tab"` emitteras råa (`main.rs:479,621,641,2051`), `"transcription-progress"` både
rått (`commands.rs:1244`) och via konstant, `TRANSCRIPTION_PHASE` har ingen lyssnare;
frontend lyssnar med 16 literaler; (3) payload — `state-changed` bär fria strängar
(`"idle"|"recording"|"transcribing"|"loading_model"` på 26+ ställen plus `"settings_reloaded"`
som inte är ett state, `main.rs:2781`), `AppState` serialiseras separat
(`app_controller.rs:41-47`), `format!("{:?}", whisper_model)` används som wire-värde
(`commands.rs:1462`). Tray, `AppState` och emit uppdateras för hand i par (14+14+12 ställen);
filtranskribering emitterar `"transcribing"` utan att ändra `AppState` och uppdaterar aldrig
tray. 20 av 81 wrappers i `api.ts` är döda. Playwright-mocken (`scripts/fixtures/transcription-
browser-mock.js:192`) är en fri `switch(cmd)` som kan glida.

**Omfattning.**
1. Backend: komplettera `events.rs` (alla fem saknade/råa namn; kebab-case-testet täcker
   dem), `#[derive(Serialize)] enum UiState { Idle, Recording, Transcribing, LoadingModel }`
   och en enda `ui_state::set(app, UiState)` som emitterar, uppdaterar tray och overlay
   (ersätter de handparade blocken). Ta bort `settings_reloaded` från `state-changed`
   (eget event).
2. Frontend: `src/lib/contract.ts` med `CommandMap` (namn → `{args, ret}`) och `EventMap`
   (namn → payload), typade `call<K>()`/`on<K>()`; byt ut alla `invoke(`/`listen(`-literaler;
   ta bort de 20 döda wrappers. Alternativ med högre utväxling: `tauri-specta` + `specta`
   genererar `contract.ts` från Rust — utvärdera i ticketen och välj (rekommendation:
   börja handskrivet, byt till generering i T12 om underhållet blir tungt).
3. Kontraktstest i CI: parsa `generate_handler![]` i `main.rs`, nycklarna i `CommandMap`,
   `invoke`-namn i `src/` och `case`-etiketterna i Playwright-mocken; misslyckas vid skillnad.
4. Mocken typas mot samma `CommandMap`.

**Acceptans.** `rg "invoke\(\s*['\"]" src` = 0 utanför `contract.ts`/`api.ts`;
`rg "listen\(\s*['\"]" src` = 0; `rg 'emit[a-z_]*\(\s*"' src-tauri/src` = 0; kontraktstestet
körs i `test:ui`; `rg '"loading_model"|"transcribing"' src-tauri/src` = 0 utanför `UiState`.

### T12 · Typade fel över gränserna

**Labels:** `area:quality`, `rust`, `frontend` · **Prio:** P1 · **Effort:** M · **Beror på:** T11

**Problem.** `Result<_, String>` används 191 gånger i app-craten, 56 i core, 31 i CLI; 106
`.map_err(|e| e.to_string())`. `meeting_reprocessing_commands.rs` kastar källfel helt
(`.map_err(|_| "…")` på 10 rader). `DictationError` (14 varianter, nästan alla `String`-
payload, `Clone + Serialize` så ingen `#[source]`) plattas till sträng vid kommandogränsen;
`main.rs:2324,2351` packar om strängar till `TranscriptionFailed(format!(…))` och tappar
sorten (timeout vs ModelBusy vs join-fel). Avbrytning är en sträng
(`"Pianissimo transcription cancelled"`, `pianissimo_backend.rs:624`); rikt `EngineHostError`
(code, retryable, crash-info) plattas av `map_engine_error` (`:617`). Swift-felmeddelanden
innehåller absoluta sökvägar (`CoreMLEngine.swift:98,392,396`, `CompiledModelCache.swift:76`)
som loggas och visas trots regeln att loggen inte får bära hemkatalogen. Frontend gör
kontrollflöde på meddelandetext: `startsWith("Dictionary changed elsewhere:")` ×6,
`includes("failed to open file:")`, och normaliserar fel med 30 kopior av
`typeof e === "string" ? e : e?.message || …` plus tre separata `errorText`-funktioner.
`PasteError`, `ApplicationLaunchError`, `AccessibilityPermissionDenied` konstrueras aldrig.

**Omfattning.**
1. Core: lägg varianterna `Cancelled` och `Engine { code, retryable, message }` i
   `DictationError`; typad `SettingsError` (ersätter 49 `Result<_, String>` i settings);
   `From`-impl från de nio `thiserror`-enumerna (`Meeting*`, `Reference`, `Proposal`); ta
   bort oanvända varianter. Path-skrubbning i en funktion vid Rust-gränsen för engine-fel
   (`EngineFailure { kind, user_message, detail }`).
2. App: `#[derive(Serialize, thiserror::Error)] enum CommandError { Busy, NotRecording,
   Cancelled, Settings(SettingsError), Transcription{kind,message}, Io{…} }` med
   `From`-impl; `#[serde(tag="code", content="message")]`. Konvertera kommandon
   domänvis (börja med transcribe/hotkey/dictionary som frontend matchar på).
3. Frontend: `BackendError {code, message}` i `contract.ts`, `toErrorMessage(e: unknown)`,
   `isBackendError(e, code)`; ersätt alla `startsWith/includes`-matchningar och de 30
   normaliseringskopiorna; ta bort de 29 `: any`.
4. CLI: JSON-felobjekt bär `code` (ligger i T16:s `CliError`).

**Acceptans.** `rg 'Result<[^>]*, String>' src-tauri/src` ≤ 20 (resterande motiverade);
`rg 'startsWith\("Dictionary|includes\("failed to open' src` = 0; `rg ': any' src` = 0;
ett test visar att en Swift-felsträng med `/Users/x/…` når UI utan sökvägen; UI kan visa
"Försök igen" vs "Ladda ner modellen igen" baserat på `code`.

### T13 · Frontend-testmigrering till Vitest, och ESLint/Prettier

**Labels:** `frontend`, `area:quality`, `priority:P0` · **Prio:** P0 · **Effort:** M/L

**Problem.** 13–14 testfiler läser `.svelte`-källa som text och asserterar med regex (869
`assert.match/doesNotMatch` totalt; `test-settings-recovery.mjs` 97, `test-meeting-
integration.mjs` 91, `test-language-glossary.mjs` 68); `test-language-switch-recovery.mjs` och
`test-dictionary-explicit-save.mjs` klipper ut funktioner ur `Settings.svelte` med TypeScript-
AST/brace-matchning och kör dem i `vm.runInContext` mot en handdeklarerad kopia av
komponentens state. De passerar när komponenten är fel och faller på rena omdöpningar. 15
filer laddar TS via `ts.transpileModule` + `data:`-URL, som inte kan följa relativa importer —
det är därför fyra `.js`-filer finns i `src/lib`. Ingen ESLint, ingen Prettier; `scripts/*.mjs`
typkontrolleras inte. Detta är det enskilt största hindret för att dela `Settings.svelte` (T33).

**Omfattning.**
1. Vitest (återanvänder `vite.config.ts` och Svelte-transformen) + `@testing-library/svelte`
   + `happy-dom`; typad `mockIPC` från `@tauri-apps/api/mocks` kopplad till `CommandMap` (T11).
2. Konvertera de 15 transpile-laddarna till vanliga `import`; byt `.js` → `.ts`.
3. Ersätt varje källtext-test med antingen (a) komponenttest som asserterar på roller/synlig
   text/registrerade IPC-anrop, eller (b) rena enhetstester av logik som extraheras till
   `.svelte.ts`/`.ts` (glossary-dirty-tracking, språkbytesåterställning, update-prep-ordning).
   Radera källtext-testerna i takt med att täckningen flyttas; `test-settings-layout.mjs`
   och `test-settings-recovery.mjs` först.
4. Behåll Playwright-sviterna som end-to-end (typad mock).
5. `eslint` (`eslint-plugin-svelte`, `typescript-eslint`, minst `no-explicit-any`,
   `no-unused-vars`) och `prettier` (inkl. `scripts/`, `tests/`); `npm run lint` i CI och i
   `npm run verify` (T05).

**Acceptans.** `rg 'readFileSync\([^)]*\.svelte' tests scripts` = 0;
`rg 'transpileModule' tests scripts` = 0; `ls src/lib/*.js` tomt; `npx vitest run` och
`npm run lint` gröna i CI; täckningsrapport från Vitest för `src/lib/*.ts` ≥ dagens testantal
(394) i antal assertioner på beteende.

### T14 · Frontend-delade stores och hjälpare

**Labels:** `frontend`, `bug` · **Prio:** P1 · **Effort:** M · **Beror på:** T13

**Problem.** Lyssnarläckor: `Settings.svelte:1132-1186` kastar bort `unlisten`-promisarna från
fem `listen(...)` och `onDragDropEvent`; cleanup vid 1243 hanterar bara två; `state-changed`
prenumereras tre gånger (Settings ×2, Overlay). Accessibility-flödet (begär, polla, retry
hotkey) finns tre gånger (`Onboarding.svelte:221-232`, `Settings.svelte:1359-1366,1912-1917`);
modellnedladdning med Pianissimo-gren (`if (id === "pianissimo-sv") …`) fyra gånger; literalen
`"pianissimo-sv"` nio gånger i komponenter; `getPlatform()`/`getSettings()` anropas i flera
komponenter var. `applySetting` (`Settings.svelte:1256`) hämtar om allt och kör tre
sekventiella kommandon per profil efter varje mutation (1 + 1 + 3N IPC-anrop för en
VAD-toggle, 16 call-sites). Effekter läser och skriver samma state (`:1028`, `:960`) och
generationsräknare ersätter request-ägarskap.

**Omfattning.** `src/lib/tauri-events.svelte.ts` (`useTauriEvent(name, handler)` med disposed-
flagga och korrekt cleanup — mönstret finns redan i `onboarding-download-state.js`);
`models.svelte.ts` (nedladdningsstate, progress, `downloadModel(id)` som döljer Pianissimo-
grenen, `PIANISSIMO_ID`-konstant från kontraktet); `permissions.svelte.ts`; `settings.svelte.ts`
och `platform` som delad källa; begränsa `refreshProfileModels` till mutationer som rör
språk/profiler/modeller. Enhetstester för varje modul (DOM-fria).

**Acceptans.** `rg '"pianissimo-sv"' src` ≤ 1; `rg 'listen\(' src/lib/*.svelte` = 0 (allt via
hjälparen); ett test visar att unmount tar bort alla lyssnare; IPC-anrop vid VAD-toggle ≤ 2
(mätt via mock).

### T15 · Delad Rust-teststödmodul och testkod ut ur gud-filerna

**Labels:** `area:quality`, `rust`, `tests` · **Prio:** P1 · **Effort:** M · **Beror på:** T01

**Problem.** Inline-testmoduler utgör 54 % av `settings/manager.rs`, 57 % av `store.rs`, 55 %
av `download.rs`, 33 % av `cli/transcribe.rs`, 31 % av `whisper_backend.rs` och ligger
interfolierade mitt i produktionskoden (`commands.rs` 13 `#[cfg(test)]`-markörer,
`whisper_backend.rs` 8, varav tre `cfg(all(test, feature="diarization"))` som inte körs i
default `cargo test`). `git blame`, review och kompileringstid lider. Fixtures dupliceras: 47
`std::env::temp_dir().join(format!(…))`-sites i 22 filer, 6 handrullade RAII-guards, 6 varianter
av `MeetingTranscript`-byggare, 4 kopior av `sha256_hex`, 3 kopior av env-scrub-loopen i CLI-
integrationstester; root-craten saknar `[dev-dependencies]` (ingen `tempfile`).
`docs/testing/integrated-pr-review.md` dokumenterar en verklig katalogkollision mellan
parallella tester.

**Omfattning.** Dev-only crate `src-tauri/crates/sagascript-testsupport` (eller
`tests/support` per crate) med `TempDir`-wrapper (tempfile), `meeting_transcript()`-byggare,
`sha256_hex`, `isolated_cli_env()`, `fake_settings()`; ersätt alla kopior. Flytta inline-
testmodulerna i de sju hotspot-filerna till syskonfiler `foo_tests.rs` via `#[cfg(test)]
#[path = …] mod tests;` (mönstret finns i `meeting_jobs_tests.rs`, `windows_clipboard_tests.rs`,
`meeting_reprocessing_tests.rs`). Gör detta före uppdelningarna i våg 2 så att de diffarna
inte svämmar över av testkod.

**Acceptans.** `rg 'std::env::temp_dir' src-tauri --type rust` ≤ 3;
`rg -c '#\[cfg\(test\)\]' commands.rs main.rs whisper_backend.rs transcribe.rs manager.rs
store.rs download.rs` ≤ 1 per fil; `cargo test --workspace` kör samma antal tester som före
(1 490 ± nya).

### T16 · CLI utdata- och exitlager

**Labels:** `area:cli`, `bug`, `rust` · **Prio:** P1 · **Effort:** M

**Problem.** 157 `println!/eprintln!`-sites (transcribe 56, config 33, models 17, engine 15)
och fyra `process::exit`; ingen SIGPIPE/`BrokenPipe`-hantering, så `sagascript transcribe …
--jsonl | head -1` och `config list | head` panikar med exit 101 (`emit_jsonl_item`,
`transcribe.rs:1415-1423`), och paniken unwindar förbi `shutdown_shared_client()`
(`lib.rs:~620`). Med `--json/--jsonl` ger preflight-fel (okänd profil, modell saknas, inga
filer) tom stdout + exit 1; enfils-`--json` skriver objekt vid lyckat och array vid fel
(`transcribe.rs:996-1008` och duplicerat `1292-1306`). stderr blandar `--progress-json`-NDJSON
med prosa och tracing. Bara ~5 subkommandon har `--json` trots CLI-first-invarianten
(`list-models`, `config list`, `glossary list`, `formats`, `profiles list` saknar).
`run()` returnerar `()` och slutar i `process::exit` (`lib.rs:633,638`) → otestbar; varje `Err`
blir exit 1 oavsett sort; `latency-report` kodar "budget missed" som `SettingsError`
(`latency.rs:808-815`); `cancel.rs:117` anropar `exit` från signalhanteraren;
`cli.command.unwrap()` (`lib.rs:587`). `scripts/verify-json-cli-streams.py` körs bara för
`transcribe --json` på en fil.

**Omfattning.** `struct Out { stdout: Box<dyn Write>, stderr: … }` som skickas in i `run_*`;
`emit_json(&impl Serialize)`; `BrokenPipe` → tyst exit 0/141; `pub fn run(cli) -> ExitCode`
och `CliError { Usage=2, Runtime=1, Cancelled=130/143, CheckFailed=3 }`; strukturerat
toppnivå-felobjekt på stdout för `--json/--jsonl`; en `BatchSink { Plain | Json | Jsonl }` som
ersätter de två emissionskopiorna; `--json` på de fem saknade subkommandona; `clippy::
print_stdout/print_stderr` warn utanför `Out`. Utöka `verify-json-cli-streams.py` + CI till
jsonl-, fel- och EPIPE-fall samt `engine/glossary/record --json`. Behåll befintliga
meddelanden byte-identiska (CI-skript och `latency_budget_cli`-tester asserterar på dem).

**Acceptans.** `sagascript transcribe --jsonl dir/ | head -1` exit 0; `sagascript transcribe
--json saknad.wav` ger `{"status":"error","code":…}` på stdout; `rg 'println!' crates/sagascript-
cli/src` = 0 utanför utdata-lagret; `run()` har enhetstester för exitkoder utan att spawna.

### T17 · Settings-robusthet: ingen tyst dataförlust, versionerade migreringar

**Labels:** `bug`, `rust`, `area:quality` · **Prio:** P1 · **Effort:** M

**Problem.** `store.rs::load_from` (326-381) returnerar `Settings::default()` vid typad
parse-miss (t.ex. en okänd enum-variant efter nedgradering eller när en modell tas bort ur
`WhisperModel`), och nästa `update()`/`save_to` (498-562) skriver över filen med defaults —
`.bak`-skyddet (515-530) täcker bara JSON-syntaxfel. Migreringslogik finns i fem mekanismer:
`canonicalize_legacy_keys` (`store.rs:28`), identifier-katalogkopiering (148-216), råfil-
sniffning (`has_embedded_glossary_fields`, `needs_profile_model_migration`, 564-596), ad hoc-
markörer (`model_lineup_version`, `engine_defaults_version`, `profile_glossary_migrated`) samt
`materialize_profile_models`/`migrate_global_glossary_to_profiles` som körs vid varje load.
`load()` (217-240) gör filsystemsprober mot transcription-modulen medan migreringar väntar
(cirkulärt beroende, se T25). Hardening-auditen sköt upp "preserve parseable fields" — avgör
nu.

**Omfattning.** (1) Backup till `.bak` + `warn!` även vid typad parse-miss, och förbjud
överskrivning av en fil som inte kunde läsas förrän användaren bekräftat (eller spara till
sidofil). (2) `schema_version` i filen + `settings/migrations.rs` med en ordnad lista
`fn(Value) -> Result<Value>` som är enhetstestbara var för sig, ersätter de fem mekanismerna.
(3) Besluta per fält om lenient deserialisering (`#[serde(default)]`/`deserialize_with`
fallback) — rekommendation: ja för enum-fält som pekar på modeller/språk. (4) Gyllene korpus av
riktiga gamla settings-filer per version i `tests/fixtures/settings/` som migreras i test.

**Acceptans.** Test: fil med okänd `whisper_model` → load ger defaults för det fältet,
behåller övriga, skriver `.bak`, och en efterföljande `update()` tappar inget annat fält;
`rg 'needs_profile_model_migration|has_embedded_glossary_fields' store.rs` = 0; alla
fixtures migrerar idempotent (två körningar = samma resultat).

### T18 · Engine-host-övervakning som återhämtar sig

**Labels:** `bug`, `rust`, `area:dictation`, `priority:P1` · **Prio:** P1 · **Effort:** M

**Problem (verifierat).** `EngineHostClient::reset()` (`client.rs:377`) är det enda som nollar
`failed`, och dess enda anropare är ett test (`tests/engine_host.rs:485`). Efter 4
krascher/spawn-/handshake-fel inom 300 s (`client.rs:1005-1020`, `max_restarts: 3`) returnerar
varje anrop "marked failed … reset required" tills appen startas om; fyra dikteringsförsök mot
en saknad host räcker. `after_timeout` (`client.rs:1117-1134`) dödar hosten bara om `ping`
misslyckas, men `ping` besvaras inline även när inferensen hänger (CoreML `prediction()` kan
inte avbrytas) — en fastnad window-request behåller sin slot för alltid och efterföljande
fönster köar bakom. `HostProcess::send` gör `write_all` under `stdin`-mutexen utan timeout
(`process.rs:241`) och `terminate` tar samma mutex (`:312`). Backoff-`sleep` (upp till 5 s)
hålls under lifecycle-låset och är inte cancel-medveten. Dikteringsljud skrivs till
`sagascript-pcm-*`-tempkataloger (`pipeline.rs:84`) som bara städas vid `Drop` — ingen
startsvepning efter SIGKILL. På Windows spawnas konsol-subsystem-hosten utan
`CREATE_NO_WINDOW` (ej verifierat på enhet). Dokumenterade `load_progress`-events och `busy`-
koden emitteras aldrig av någon riktig host; den fasta 180 s load-deadline förlängs inte.

**Omfattning.** Tidsbaserat `Failed` (half-open efter cooldown) + `reset()` vid "engine doctor",
inställningsändring och manuell retry; wedge-watchdog (host-sidan: `exit(70)` om ett jobb
ignorerar cancel i N s, eller klient-sidan: `status.in_flight` efter timeout + kill); stdin-
skrivning via writer-tråd med timeout, `terminate` oberoende av stdin-låset; cancel-medveten
backoff; startsvepning av gamla `sagascript-pcm-*`; `#[cfg(windows)] creation_flags(
CREATE_NO_WINDOW)`; implementera eller ta bort `load_progress`/`busy`. Fake-host-lägen i
`fake_engine_host.rs` för "hänger men svarar ping" och "slutar läsa stdin".

**Acceptans.** Tester: (a) 4 fel → cooldown → frisk host fungerar igen utan restart;
(b) host som ignorerar cancel men svarar ping → hosten startas om inom timeout + N s och nästa
fönster lyckas; (c) stdin-blockering → request timeout, inte evig hängning; (d) stale PCM-
katalog tas bort vid start. `rg 'lock\(\)\.unwrap' client.rs process.rs` = 0 (T10).

### T19 · Engine-host-protokollet maskinkontrollerat mellan Rust och Swift

**Labels:** `area:quality`, `rust`, `swift` · **Prio:** P1 · **Effort:** M · **Beror på:** T07

**Problem.** Protokollet är definierat tre gånger: prosa i `docs/engine-host-protocol.md`, Rust-
typer i `sagascript-engine-protocol` (används av klient, ORT-host och fake-host) och en
handskriven Swift-dubblett med `[String: Any]`-byggare (`EngineTypes.swift:36-140`) och
felkoder som fria strängar (18 literal-sites). Rust parsar med `#[serde(default)]` på nästan
alla svarsfält (`LoadResult` helt default, `lib.rs:341-356`) så ett stavfel i Swift ger tyst 0/
tom sträng i stället för fel. Swift-sidan har andra semantik (`priority` saknad → `"batch"`,
`hello` kontrollerar bara att `client` är en dictionary). Konstanter dupliceras (16 MiB,
boost-gränser, capability-defaults). Enda korskontrollen är `engine-host/test-vectors/
context-biasing.json` (Swift + ORT) och `scripts/engine-host-conformance.py` som inget
workflow kör. Rust-integrationstesterna (55 st) kör mot en Rust-fake och kan inte upptäcka
Swift-drift. "Dirty tree"-detektion finns tre gånger (`build-engine-host.sh`, `ort/build.rs`,
`ci-build-identity.mjs`); codesign-raderna dupliceras i två workflows och "testas" med regex.

**Omfattning.** Gyllene JSONL-transkript i `engine-host/test-vectors/protocol/` som Rust
avkodar via protokoll-craten och Swift `ProtocolServerTests` asserterar byte-för-byte/
semantiskt via `FakeEngine`; Swift `enum WireErrorCode: String` + en `WireProtocol.swift` som
äger parsning/byggning; ta bort `serde(default)` på fält klienten beror på (`window_s`,
`blank_id`, `vocab_size`); kör den modellfria delmängden av `engine-host-conformance.py`
(`hello`, `ping`, `shutdown`, EOF) i PR-CI mot den stagade Swift-hosten, full körning i
release-gate; ett delat `scripts/engine-host-identity.sh` för version/SHA/dirty som alla tre
konsumenter anropar, `check-engine-host-identity.sh` faller på `dirty=true` i release;
Swift-tester för `TdtDecoder` och `CoreMLEngine.readSamples` med små inspelade tensorfixturer
(täckning idag 18 % resp. 1,3 %).

**Acceptans.** Ett avsiktligt omdöpt svarsfält i Swift får `swift test` eller Rust-
vektortestet att falla; `rg '"[a-z_]+"' EngineTypes.swift` visar inga felkods-literaler utanför
enumen; konformans-steget syns i PR-loggen; Swift-täckning för `TdtDecoder.swift` ≥ 60 %.

### T20 · Async-hygien och nedladdningshärdning i core

**Labels:** `rust`, `area:quality` · **Prio:** P2 · **Effort:** S/M

**Problem.** `download_to_path_with_limits` (`download.rs:139`) kör synkront `create_dir_all` och
en `read_dir`-svepning inne i `async fn` (170-173); `transcription/model.rs:186-195` anropar
`prepare_existing_artifact` som gör synkron SHA-256 av multi-GB-filer när ingen stämpel finns
(`download.rs:556-600,673-693`); samma mönster i `pianissimo_model.rs:~800,826` och en synkron
`std::fs::write` i `ensure_coreml_encoder`. Bara två `spawn_blocking` i icke-testkod. Ingen
`sync_all` på tempfilen före `rename` (bara stämpeln syncas); ingen HTTP Range-resume (avbruten
3 GB-nedladdning börjar om och `TmpFileGuard` raderar delen); `reqwest::Client` byggs per
nedladdning. 65 av 66 async-kommandon i `commands.rs` gör synkron fil-I/O utan `.await`.

**Omfattning.** `spawn_blocking` kring hashning/fs i de fyra ställena; `sync_all` före
`rename`; delad `Client`; (separat beslut) Range-resume med stämpel. Gör settings-setters i
`commands.rs` till `fn` eller kör skrivningen i `spawn_blocking` (hänger ihop med T22:s
`persist_and_sync`).

**Acceptans.** `rg 'fn .*async' -A 30 download.rs model.rs pianissimo_model.rs | rg
'std::fs::'` tomt utanför `spawn_blocking`; nedladdningstesterna (49) gröna; ett test för
`sync_all`-ordningen (injicerad fs-fasad eller strace-fri kontroll av anropsordning).

### T21 · Dela `src/main.rs` i `tray/`, `updater/`, `launch/`

**Labels:** `rust`, `area:quality`, `refactor` · **Prio:** P1 · **Effort:** M · **Beror på:** T01, T10

**Problem.** `main.rs` är 3 727 rader (≈ 2 850 produktion) med 58 fria funktioner och 54
`cfg(target_os)`-grenar: CLI-dispatch, `main()` på 685 rader (967-1652) vars `.setup`-closure
inlinar hotkey-registrering med hälsobokföring (1143-1203), tray/meny (1204-1340), legacy-
migrering (1346), watcher (1360), overlay-precreate (1383), modellvärmning (1411-1450);
uppdateringsflödet (233-700); hela dikteringspipelinen `stop_recording_and_transcribe`
(2124-2578); settings-watchern (2605-2810); launch-modes; fönsterhantering. Det är filen med
näst högst churn och 34 fix-touches sedan augusti; bara de rena predikaten är testade.

**Omfattning.** Ren flytt utan beteendeändring, en commit per modul: `tray/` (menyformat,
profilmeny, `tray_label`, `update_tray_status`, position/identitet, rader 93-230 och
1652-1830), `updater/` (233-700 + `updates.rs`, `update_activity.rs`, `update_recovery.rs`),
`launch/` (`GuiLaunchMode`, `InitialWindowRequest`, `gui_launch_mode`, `second_instance_*`,
`reveal_existing_main_window`, `open_settings_window`, `attach_parent_console`,
`migrate_legacy_settings`, 1751-2080), `settings_watcher.rs` (2605-2810), `dictation/`
(2124-2578 — flyttas här, omarbetas i T28). `main()` blir wiring. Flytta testerna med
(T15 först). Behåll `pub(crate)`-synlighet där `commands.rs` använder funktioner.
Bevara `#[cfg(any(target_os = "macos", test))]`-gatingen.

**Acceptans.** `main.rs` < 400 rader; inga nya `pub` utanför crate; `cargo test --workspace`
samma testantal; `git diff --stat` per commit visar bara flyttade block (verifiera med
`git diff --color-moved=zebra`).

### T22 · Dela `src/commands.rs` per domän, `persist_and_sync`, `macos_mic` till `platform/`

**Labels:** `rust`, `area:quality`, `refactor` · **Prio:** P1 · **Effort:** M · **Beror på:** T01, T10

**Problem.** `commands.rs` är 3 353 rader med 67 kommandon och 13 insprängda testmoduler.
Settings-setters följer mönstret `store::update(|s| …)?` + `ctrl.lock().settings_mut().x =
persisted.x` (15 `update`-anrop, 11 `settings_mut()`-tilldelningar, t.ex. 2226-2234, 616-625)
— lätt att glömma andra halvan, och watchern (`main.rs:2794`) synkar hela objektet med en
annan fältmängd, `HotkeyService.current_shortcut` är en tredje kopia. `mod macos_mic`
(3101-3300) med 6 av cratens `unsafe` och `Class::get(...).expect(...)` (3146,3148,3169) ligger
i kommandofilen; `get_platform` (3327-3345) är fyra `cfg`-block för `std::env::consts::OS`.
`retry_hotkey_registration` blockerar en tokio-worker med otimad `recv()` (`:948`);
`apply_hotkey_change` (200 rader, sync OS-registrering + disk-I/O) anropas från `async fn`.
`unreachable!()` på 480, 829, 858.

**Omfattning.** `commands/{settings, hotkey, transcribe, models, dictionary,
update_recovery, permissions, files, mod}.rs` med re-exports i `mod.rs` så `generate_handler!`
är stabil; `fn persist_and_sync(ctrl, |&mut Settings|) -> Result<Settings, CommandError>` som
enda vägen att skriva settings (setters blir enradare, ev. makro); flytta `macos_mic` till
`platform/macos.rs` bakom en `platform::mic_permission()`-fasad; `get_platform` →
`std::env::consts::OS`; tidsbegränsa `recv()`; byt `unreachable!` mot `Result`.

**Acceptans.** Ingen fil i `commands/` > 600 rader; `rg 'settings_mut\(\)\.' src-tauri/src/
commands` = 0; `rg 'unsafe' src-tauri/src/commands` = 0; `rg 'unreachable!' src-tauri/src` = 0;
test: "setter skriver till disk och in-memory är lika" för alla setters via en tabell.

### T23 · Dela `cli/transcribe.rs` till en `transcribe/`-katalog

**Labels:** `area:cli`, `rust`, `refactor` · **Prio:** P1 · **Effort:** M/L · **Beror på:** T15, T16

**Problem.** 4 678 rader (≈ 2 877 produktion), repots största fil, 27+ commits sedan juni, 20
fix-touches. Minst nio ansvar: `TranscribeArgs` (30+ flaggor, 49-207); ett publikt
mötesbibliotek som appen importerar (208-768); `run`/`run_inner` (769-1019); Pianissimo-
batchvägen (1021-1313); progress-events med strängfaser (1315-1425); batchkörning;
**`transcribe_file` 1512-2302 ≈ 790 rader, cyklomatisk 94, kognitiv 85**, 31 `cfg(feature=
"diarization")`-grindar, 24 `eprintln!`, 15 `expect`/`unreachable!` som bär invarianter
(`.expect("cache_hit requires cached data")`, `unreachable!("cache hits require the
diarization feature")`); diarization-expansion (2303-2437); fuzzy-vokabulärkorrigering
(2438-2596); språkfönster (2617-2804); option-resolution; profil/språk/modell-parsning
(2895-3100). `models.rs` och `benchmark_config.rs` importerar `parse_model`/`model_id_string`
härifrån. Ett test (`4352-4372`) `include_str!`:ar källfilen och letar efter substrängar i en
funktionskropp.

**Omfattning.** Mekaniska flyttar först (en commit var): `transcribe/{args, inputs,
vocab_correction, language_regions, resolve, output, meeting, progress}.rs` med re-export-shim
`sagascript_cli::transcribe::*`. Därefter `enum Source { Cached(Cached), Decoded(Audio) }` i
stället för `cache_hit` + `Option` + `Option`, och `transcribe_file` delat i steg (identitet,
cache-uppslag, decode, språkdetektering, diariserad gren, plain gren, JSON-sammansättning) som
var och en tar `&mut Out` (T16) och är testbara med fejkade segment. Ersätt `include_str!`-
testet med en typnivå-seam (`EncoderPolicy::NoDownload`). Kompilera i båda feature-lägena.

**Acceptans.** Ingen funktion > 150 rader i `transcribe/` (clippy `too_many_lines`);
`rg 'unreachable!|expect\("cache' crates/sagascript-cli/src/transcribe` = 0; `cargo clippy -p
sagascript-cli --no-default-features` och default gröna; `rg 'include_str!\("transcribe'` = 0;
kognitiv komplexitet < 30 för alla funktioner.

### T24 · Dela `whisper_backend.rs`, en låst runtime-struct, `TranscribeRequest`, rätt feature-flagga

**Labels:** `rust`, `area:dictation`, `refactor` · **Prio:** P1 · **Effort:** M/L · **Beror på:** T15

**Problem.** 3 892 rader, 8 insprängda testmoduler (tre `cfg(all(test, feature="diarization"))`
som inte körs i default `cargo test`), `impl WhisperBackend` 828-2327 med ~45 metoder:
modell-livscykel/cache, lås- och abortprotokoll (1386-1494), 14 `transcribe_*`-ingångar i en
kedja `transcribe_sync → _with_progress → _with_progress_and_prompt → _with_options →
_with_options_segments` (1495-1660), parallell chunkning med 8 positionsargument
(`#[allow(too_many_arguments)]` 1740, 1852), DTW-ordgruppering (2599-2860), stitching
(438-640), progress, Windows-x64-CPU-check. Fem mutexar (`context`, `state`, `loaded_runtime`,
`cached_runtime`, `load_lock`) med lås-ordning dokumenterad i kommentar; invarianten "context
och loaded_runtime båda Some/None" upprätthålls av `panic!` (1110). `unsafe impl Send/Sync`
(819-820, även `capture.rs:56-57`) utan `// SAFETY:` — whisper-rs 0.15.1 deklarerar redan
`Send+Sync`, så impl:erna döljer troligen bara framtida fel. `cfg(feature = "diarization")`
21 gånger i filen gatar DTW/ordtidsstämplar som inte har med ort att göra;
`ContextProfile::TokenAlignment` ger runtime-fel när flaggan är av.

**Omfattning.** `transcription/whisper/{mod, runtime, entry, parallel, words, stitch,
progress}.rs` som ren flytt; slå ihop `context` + `loaded_runtime` till `Mutex<Option<Loaded
{key, ctx}>>` och ta bort panic-invarianten; `TranscribeRequest { audio, language, options,
hooks }` som ersätter kedjan (3–4 metoder kvar); prova att ta bort `unsafe impl` och bygga, annars
`// SAFETY:`; byt feature-grind för DTW till `word-timestamps` (eller gör den ovillkorlig) och
se till att CI kör matrisen (T07). Karakteriseringstester för abort/timeout med en fejk-
kontext före omstruktureringen.

**Acceptans.** Ingen fil i `whisper/` > 900 rader; `rg 'panic!\("whisper model/context'` = 0;
`rg 'unsafe impl' sagascript-core/src` = 0 eller med SAFETY-kommentar; `rg -c 'cfg\(feature =
"diarization"\)' whisper/` ≤ 3; `cargo test -p sagascript-core --features diarization` kör de
tre tidigare dolda modulerna.

### T25 · Koherent modullayout i core och bruten settings↔transcription-cykel

**Labels:** `rust`, `area:quality`, `refactor` · **Prio:** P1 · **Effort:** M/L · **Beror på:** T17

**Problem.** `lib.rs` har 18 platta `pub mod`; 13 toppnivåfiler ≈ 9 150 rader: `meeting*` är sju
filer (4 463 rader) som är ett subsystem; `diarization_{evaluation,qualification,reference,
strata,report}` + `speaker_hint` ≈ 4 700 rader ligger utanför `diarization/` för att hela
katalogen är ort-gatad trots att `report.rs`, `merge.rs`, `clustering.rs` inte behöver ort;
två olika "report"-moduler. Utvärderingskoden (≈ 3 200 produktionsrader) används bara av CLI:n.
`settings/manager.rs` (3 652 rader, 84 pub-items) är en felmärkt modellkatalog (URL:er/
SHA-256 på rad 55-540) + `FileModel` + `Settings` + profiler + hotkey-validering + migreringar;
den och `store.rs::load()` anropar `crate::transcription::{model::is_model_downloaded,
pianissimo_backend::runtime_supported_on_this_os, pianissimo_model::is_downloaded}`
(`store.rs:224-241`, `manager.rs:595`) medan åtta transcription-filer importerar
`crate::settings` — en cykel. `WhisperModel::dtw_preset` returnerar en `whisper_rs`-typ.
Testseams är publika (`TestHook` re-exporterad, `EngineHostConfig.test_hook` pub-fält,
`run_idle_check_now` `#[doc(hidden)]`). Sju `#[allow(dead_code)]` på pub-funktioner.

**Omfattning.** `models/` (`WhisperModel`, `FileModel`, integritetstabell, `id()/from_id()/
FromStr`) utan beroende på settings; `settings/{mod, profiles, migrations}` (T17); injicera
tillgänglighetsprober (`LineupEnv` finns redan för en väg) i stället för att settings anropar
transcription; `meeting/{mod, media, review, reprocess/{mod, matching, plan, proposal}}`;
`diarization/` alltid kompilerad med ort-stegen i `diarization/engine/` och utvärderingen i
`diarization/eval/` — eller flytta eval till CLI/egen crate (beslut i ticketen; rekommendation:
`diarization/eval/` i core, gatad på en `eval`-feature); `pub(crate)` där externa användare
saknas; testseams bakom `#[doc(hidden)] pub mod testing`. Re-export-shims en release.

**Acceptans.** `rg 'crate::transcription' settings/` = 0; `ls crates/sagascript-core/src/*.rs`
≤ 4 filer (lib, error, + ev. två); `cargo doc --no-deps` visar < 40 pub-items i `settings`;
`cargo build -p sagascript-core --no-default-features` kompilerar `diarization::report`.

### T26 · CLI/app-paritet: en implementation per operation

**Labels:** `area:cli`, `rust`, `bug` · **Prio:** P1 · **Effort:** M · **Beror på:** T16

**Problem.** AGENTS.md säger att CLI:n är sanningskällan, men samma användarintention ger
olika persisterat tillstånd: `config set language sv` (`config.rs:595-606`) anropar bara
`set_default_profile_language`, medan appens `set_language` (`commands.rs:520-530`) även sätter
`whisper_model = recommended(language)` och `auto_select_model = true`; `auto_paste` klampas i
appen utan Accessibility (`commands.rs:1479`) men CLI:n persisterar `true` och varnar;
`vad_enabled` laddar VAD-modellen i appen (`:2254`) men inte i CLI:n. "Resolve run context"
(profil → språk → modell → glossary) är kopierad fyra gånger (`transcribe.rs:798-835`,
`record.rs:71-110`, `meeting_reprocessing_cli.rs:227-257`, `commands.rs:94-130`) och
`Settings::default_profile()` återimplementeras fyra gånger trots att den finns
(`manager.rs:993`). Beam-default dupliceras (`transcribe.rs:2150` vs `commands.rs:69-79`).
Modell-ID-mappning finns i tre mekanismer (handtabeller `parse_model`/`model_id_string`,
serde-roundtrip på 9 ställen med `.expect`, `FileModelPreference::parse_id`); `list-models`
beräknar lineup olika i CLI (`models.rs:127-170`, `installed_size_bytes`) och app
(`commands.rs:1307-1365`, `download_size_bytes`). Config-nyckelregistret är stränghanterat:
en ny nyckel rör 9+ ställen i `config.rs` med `_ => unreachable!()` på tre ställen;
`parse_enum_value` (`config.rs:917`) accepterar `config set language english` fel/rätt olika
mot `transcribe --language english`. Atomisk "publish via hard_link" är kopierad tre gånger,
`sha256`-hjälpare fyra, glossary-profilvalidering i två uppsättningar.

**Omfattning.** I core: `RunContext::resolve(&Settings, RunOverrides)`,
`settings::ops::{apply_language_selection, set_auto_paste, set_vad}` som båda ytorna anropar
(sidoeffekter — hotkey-registrering, nedladdning, permission-check — stannar i respektive
yta), `WhisperModel::{id, from_id, FromStr}` + `PIANISSIMO_ID`, `models::catalog(language) ->
Vec<ModelInfo>`, `fsutil::publish_new`, `sha256_hex`, `glossary_ops`. I CLI: tabellstyrt
`struct Key { name, help, get, set, reset }` som genererar `VALID_KEYS`, `list` och hjälptext;
`ValueEnum`/`value_parser` för språk/modell. Paritetstest-tabell som kör varje operation genom
CLI-funktion och app-funktion och jämför persisterad `Settings`. Besluta medvetet för varje
driftpunkt (språkbyte: ska CLI:n också nollställa legacy-fälten? rekommendation ja).

**Acceptans.** Paritetstestet grönt för `language`, `auto_paste`, `vad_enabled`,
`whisper_model`, profiler; `rg 'unreachable!' crates/sagascript-cli/src/config.rs` = 0;
`rg 'find\(\|c\| c\.id == "default"\)' src-tauri` = 0; `sagascript list-models --json` och
appens modellväljare bygger på samma `catalog()`.

### T27 · Bryt beroendeinversionen app → cli

**Labels:** `rust`, `area:quality`, `refactor` · **Prio:** P2 · **Effort:** L · **Beror på:** T23, T25, T26

**Problem.** Appen importerar domänlogik från CLI-craten: `sagascript_cli::transcribe::
{transcribe_meeting_file_with_control, MeetingControl}` (`meeting_jobs.rs:196`),
`sagascript_cli::meeting_reprocessing::{plan_reprocessing, execute_reprocessing,
ReprocessingInput}` (`meeting_jobs.rs:220`, `meeting_reprocessing_commands.rs:123,131`),
`sagascript_cli::meeting_proposal::read_proposal` (`:243`). Beroendet core ← cli ← app gör att
GUI:t drar in clap, arboard, indicatif, ctrlc; feature-flaggor måste speglas i två
`Cargo.toml`; hela CLI-craten är de facto publik API; core kan inte återanvända
pipelinerna. Kandidater för core: mötespipelinen (`transcribe.rs:208-768`),
`meeting_reprocessing.rs` (med `WorkBackend`-seamen), `diarization_cache.rs` (543
produktionsrader), proposal-I/O, RTTM/UEM-parsers (`diarization.rs:1038-1172`).

**Omfattning.** Ren relokering efter T23/T25/T26, modul för modul, med tunna clap-adaptrar
kvar i CLI:n; appens `sagascript-cli`-beroende reduceras till `open`-konstanter och `run`
(eller tas bort genom att flytta de två sakerna till core/app). `meeting_reprocessing_tests.rs`
flyttar intakt. Lyft `WorkBackend` till core och lägg en fejk för plain mötestranskribering så
`begin_meeting_file`/`begin_meeting_reprocessing` (`meeting_jobs.rs:97,110`: spawn_blocking/
timeout/cancel/join, 0 inline-tester) kan testas.

**Acceptans.** `rg 'use sagascript_cli::' src-tauri/src` ≤ 2 (open/run);
`cargo tree -p sagascript -e normal | rg 'clap|indicatif|ctrlc'` tomt;
timeout/cancel/join-tester för mötesjobb finns.

### T28 · Dikteringspipelinen bakom portar

**Labels:** `rust`, `area:dictation`, `priority:P1`, `refactor` · **Prio:** P1 · **Effort:** L · **Beror på:** T21, T22, T11

**Problem.** Det hetaste flödet (hotkey → record → transcribe → paste) är otestbar orkestrering:
`handle_hotkey_event` (`main.rs:831-966`), `handle_hotkey_release` (2082),
`stop_recording_and_transcribe` (2124-2578, ≈ 455 rader), `restore_target_before_paste` (2812)
tar `AppHandle`/`State`. Timeout/abort/grace-transaktionen är kopierad tre gånger
(`main.rs:2297-2352`, `commands.rs:1112-1165`, `:1238-1256`) och konstanterna
`TRANSCRIPTION_TIMEOUT_SECS`/`ABORT_GRACE_SECS` är definierade i båda filerna; de två
live-vägarna har redan olika felform. Profil-resolution upprepas (`main.rs:2222-2243` vs
`commands.rs:1061-1085`). Auto-paste och main-thread-dispatch finns på 5+ sätt: den härdade
vägen (`main.rs:2404-2480`), fyra fire-and-forget-block i filtranskribering
(`commands.rs:2669,2690,2865,2958`) som på Windows kan blockera UI-tråden, `dispatch_to_main`
(privat), `on_main` (500 ms timeout) och otimad `recv()`; `AppController::auto_paste` är
`dead_code`. Perf-logg-JSON byggs i tre nästan identiska block (`main.rs:2371,2430,2490`).
`handle_hotkey_event` tar controller-låset två gånger i rad (845, 853) → TOCTOU.
Ingen Whisper-fejk: `LiveDictationBackend` håller konkret `Arc<WhisperBackend>`
(`live_dictation.rs:25-27`); live-diktering med Whisper täcks bara av CLI-smoke i CI.

**Omfattning.** `dictation/` med `pipeline.rs`, `paste.rs`, `perf_log.rs`; portar `trait
Transcriber`, `trait PasteTarget`, `trait UiSink` (tray, overlay, events via `UiState`, T11),
`trait Clock`; en `run_bounded(transcriber, audio, …) -> (Result<String, DictationError>,
Timings)` som båda live-vägarna använder; en `paste::dispatch(app, text, opts) -> PasteOutcome`
som äger main-thread/blocking-valet med `on_main_thread<T>(app, f, timeout)`; radera döda
fält. Skriv karakteriseringstester (alla `StopRecordingOutcome`-grenar, tom audio, no-speech,
fel, cancel, paste-sedan-restore-ordning, timeout → abort → grace) mot fejkar FÖRE flytten och
kör dem oförändrade efter.

**Acceptans.** `rg 'TRANSCRIPTION_TIMEOUT_SECS' src-tauri/src` = 1 definition; `rg
'PasteService::new\(\)\.paste' src-tauri/src` = 0 utanför `dictation/paste.rs`; `rg
'run_on_main_thread' src-tauri/src` ≤ 2; ≥ 15 nya pipeline-tester utan Tauri-runtime; inga
beteendeändringar i den manuella acceptansen (RELEASING.md:169-190).

### T29 · `hotkey::transition` — en tillståndsmaskin i stället för två kopior

**Labels:** `rust`, `area:dictation`, `bug` · **Prio:** P1 · **Effort:** M · **Beror på:** T22

**Problem.** Avregistrera gammal → registrera ny → vid fel avregistrera delmängd →
återregistrera gammal → `health.record(...)` finns i både settings-watchern
(`main.rs:2690-2775`) och `apply_hotkey_change` (`commands.rs:731-930`); "återställ gamla
inställningar"-blocket (fem fält) upprepas tre gånger bara i watchern (2717-2721, 2732-2736,
2744-2748). Felmatrisen (`OperationalHotkey::{Registered, Inactive, Unactive}`) är den mest
riskfyllda delen av hotkey-hanteringen (fail-closed, säkerhetsrelevant) och lever i två
ad hoc-kopior. Hardening-auditen flaggade `HotkeyService`-refaktorn som optional med
deadlock-risk; `hotkey/service.rs` är nu nästan helt `dead_code`.

**Omfattning.** `hotkey::transition::apply(old, new, &mut dyn Registrar, &HotkeyHealth) ->
TransitionOutcome` med injicerade `register/unregister`-closures; tabelldrivna tester för hela
matrisen (3 grenar i watchern + 4 i `apply_hotkey_change` som karakteriseringstester före);
båda anropare använder den; radera `hotkey/service.rs`-resterna och `HotkeyService.
current_shortcut`-kopian.

**Acceptans.** En implementation (`rg 'unregister_all|register_shortcut' src-tauri/src` bara i
`hotkey/`); ≥ 10 tabelltester; `rg 'allow\(dead_code\)' src-tauri/src/hotkey` = 0.

### T30 · Updater-orkestrering bakom traits

**Labels:** `rust`, `area:updates` · **Prio:** P2 · **Effort:** M · **Beror på:** T21 · **Relaterat:** #255, PR #259

**Problem.** `UpdateMenuState { checking, installing, … }` i en `Mutex` (`main.rs:238-246`);
`check_for_updates_with_intent` sätter `checking = true` (366) och litar på att varje utväg
nollar den — en panic i den spawnade tasken eller tidig return låser "Check for Updates" till
omstart. `install_signed_update` (548-694) är en pollingloop (250 ms upp till 600 s) direkt
på `tauri_plugin_updater::Update` + `AppHandle` med strängliteraler för tray-status;
sekvensen (download → vänta på idle → förbered drafts → install → restart) är otestbar och
spridd över fyra filer. Den signerade self-upgrade-smoken (`test-build.yml:203`) är manuell.

**Omfattning.** `updater::Orchestrator` över `trait UpdateProvider`, `trait MenuSink`,
`trait Recovery`; `enum UpdatePhase`; `CheckingGuard` (Drop nollar flaggan); fejk-drivna
tester för download-fel, väntetimeout, recovery-konflikt, restart; schemalägg
`test-build.yml`:s updater-smoke veckovis. Koordinera med PR #259 (in-app updates) så att den
inte byggs vidare på den gamla strukturen.

**Acceptans.** ≥ 8 orkestreringstester utan Tauri; `rg '"Checking for updates|"Downloading'
src-tauri/src` = 0 utanför `MenuSink`-impl; ett test där tasken panikar → menyn återställs.

### T31 · `AppController`-nedbrytning

**Labels:** `rust`, `area:dictation`, `refactor` · **Prio:** P2 · **Effort:** M/L · **Beror på:** T28

**Problem.** `AppController` (1 573 rader) äger samtidigt audio, paste, hotkey, logging
(fil-I/O), settings och sessionstelemetri (`app_controller.rs:66-94`). Logg-flush och
audio-start sker under det globala låset; hotkey-down-latens kopplas till diskskrivning.
Fälten `paste` och `hotkey` och `auto_paste` är döda.

**Omfattning.** Steg 1: flytta `session_data`, `record_phase`, `log_dictation_performance` till
en kanalbackad `TelemetrySink` utanför låset (synkron sink i tester för ordningsgaranti).
Steg 2: radera döda fält. Steg 3 (valfritt, L): `RecordingStateMachine` + `ArcSwap<Settings>`;
granska lås-ordning mot `HotkeyHealth`, `UpdateActivity`, mötesjobb-slot (i dag jobs-slot →
controller i `meeting_jobs.rs:151,158`).

**Acceptans.** Inget fil-I/O-anrop nåbart från en funktion som håller `SharedController`
(verifiera med ett test som injicerar en blockerande logg-sink och mäter att hotkey-down
returnerar < 5 ms); dödkodsfälten borta; befintliga 55 controller-tester gröna.

### T32 · `SpeechEngine`-trait och en avbrytningsmodell

**Labels:** `rust`, `area:dictation`, `refactor` · **Prio:** P2 · **Effort:** L · **Beror på:** T24, T12

**Problem.** Inget trait för transkriptionsbackend i hela `transcription/`; Whisper och
Pianissimo kopplas för hand: `LiveDictationBackend { pianissimo: bool }` (`live_dictation.rs:
26-36`, kastar `model, options`), `plain_file_jobs.rs:102`, CLI `transcribe.rs:1109-1138`,
`glossary.rs:328`; `FileModel::PianissimoOriginal` 51 referenser i `settings/manager.rs`, ~80
i workspace; `runtime_supported_on_this_os()` på 22 ställen. Tre överlappande
avbrytningsmekanismer (`AtomicBool` + sticky flag, `CancelToken`, pollande
`with_cancel_bridge`-tråd), `cancelled_error()` två gånger. Ett tredje engine innebär dussintals
ställen.

**Omfattning.** Steg 1 (M): `EngineAvailability`-gate som ersätter de 22 anropen och de
spridda meddelandena. Steg 2 (L): `trait SpeechEngine { warm, transcribe_dictation,
transcribe_file, request_abort, capabilities }` bakom `LiveDictationBackend` och
`plain_file_jobs`, med en `CancelToken` som enda modell och fejk-implementation för tester
(ger också Whisper-vägen sin första fejk, se T28). Rör inte settings-enumen i första steget.

**Acceptans.** `rg 'runtime_supported_on_this_os\(' src-tauri` ≤ 2;
`rg 'pianissimo: bool'` = 0; en fejk-engine kör hela `transcribe_file`-stegen i test;
`rg 'fn cancelled_error'` = 1.

### T33 · Dela `Settings.svelte` i flikpaneler och controllers

**Labels:** `frontend`, `area:quality`, `priority:P1`, `refactor` · **Prio:** P1 · **Effort:** L · **Beror på:** T13, T14, T11

**Problem.** 3 512 rader: 2 040 script-rader, 90 funktioner, ~70 `$state`, 7 `$effect`, 826
rader CSS, 71 commits, 39 fix-touches sedan augusti. Nio separata ansvar i en komponent:
shell/flikar/build-info; update-recovery-orkestrering (186-680, egen mutationskö, läser
`fileComponents` via `bind:this` och förlitar sig på `tick()`-ordning); dictate-profiler och
hotkey-capture; test-recorder; transcribe-flik (kö, drag-and-drop, recent files); dictionary/
glossary (~12 `$state`, dirty-guard som gatar routern `requestTabChange` 1665); språkbyte med
recovery; overlay/Advanced engine; modellnedladdningar. Att lägga till en ny panel kräver
ändringar i `SettingsTab`-unionen (107), två literaltripplar (1175, 1234), Rust-sidans
`index.html?tab=` (`main.rs:2044`), flikknapparna (2054-2063), ett nytt `hidden=`/`{#if}`-
block, egen fel-/saving-state, och `test-settings-layout.mjs` som asserterar exakt tre flikar i
exakt ordning.

**Omfattning.** I ordning, med Playwright-sviterna gröna i varje steg: (1)
`UpdateRecoveryController` som `.svelte.ts`/klass med injicerad `api` + egna enhetstester;
(2) `ModelDownloads`-store (T14); (3) `GlossaryEditor.svelte` som exponerar sin dirty-guard till
routern; (4) `TranscribeTab.svelte` (mest fristående; jobb måste överleva flikbyte — behåll
`hidden`-semantiken), `DictateTab.svelte`, `SettingsTab.svelte`, `AdvancedPanel.svelte`;
(5) tunt `Settings.svelte` med `SETTINGS_TABS`-konstant som URL-parser, tray-event och nav
delar, `<TabPanel>`-register. Varje panel får egen fel- och saving-state.

**Acceptans.** `Settings.svelte` < 400 rader; ingen `.svelte` > 800 rader; en ny panel = en ny
fil + en rad i `SETTINGS_TABS` (visa med en test-PR); `rg '"dictate" \|\| ' src` = 0;
Vitest-komponenttester per panel; Playwright gröna.

### T34 · UI-primitiver, CSS-konsolidering och a11y

**Labels:** `frontend`, `a11y` · **Prio:** P2 · **Effort:** M · **Beror på:** T33

**Problem.** Huvudflikraden (`Settings.svelte:2054-2063`) saknar `role="tablist"/"tab"`,
`aria-selected`, `aria-controls` (de inre resultatflikarna 2409-2426 har dem); två modal-
strategier (handbyggd `role="dialog"` med egen fokusfälla 2646-2680 vs `<dialog>.showModal()`
758); `prefers-reduced-motion` saknas för Settings-spinners/pulse (3241, 2971, 3473);
`outline: none` på inputs (`app.css:84`, `Settings.svelte:3105/3289/3498`). CSS: `button`
omdefinieras i tre komponenter, `.error` i två, `.transcribe-error` i två, fyra `@keyframes
spin/pulse`, två `.spinner`; hårdkodade färger (`#ffb0b0`, 11 hex i MeetingProgress, 4 i
TranscriptionStages); bara `--danger` som statustoken; dark-only utan `prefers-color-scheme`.

**Omfattning.** `Tabs.svelte`, `Modal.svelte` (på `<dialog>`), `Notice.svelte`,
`Spinner.svelte`, `Switch.svelte`, `Field.svelte`; delade regler och `--warning`/`--success`-
tokens i `app.css`; reduced-motion globalt; synlig fokusring; ta bort lokala omdefinitioner.
Axe-körning i Playwright som grind på de tre vyerna.

**Acceptans.** `rg '@keyframes' src` ≤ 2 filer; `rg 'outline: none' src` = 0; axe utan
violations av nivå serious/critical; flikraden navigerbar med piltangenter.

### T35 · Testtillförlitlighet: tidskänsliga tester och flake-detektor

**Labels:** `tests`, `package:ci` · **Prio:** P2 · **Effort:** M · **Beror på:** T15 · **Relaterat:** #320, #307

**Problem.** Väggklocksgränser i assertioner: `engine_host.rs:281 (<300 ms), 407 (<500 ms),
484 (<100 ms)`, `pianissimo_adapter.rs:155 (<500 ms), 296 (<250 ms efter 500 ms sleep)`,
`download.rs:1678,1768,1786 (<2 500 ms)`; supervisor-tester med 50 ms ticks som var och en
spawnar en barnprocess på 3-vCPU-runners; `cli/engine.rs:700-703` muterar process-global env
(`ENGINE_TEST_FORCE_FAILS`) i parallella tester (blir `unsafe` i edition 2024);
`logging/service.rs:140-146` har `cfg(test)`-grenar i produktionskod; flera tester löser
riktig `dirs::data_dir()` (`store.rs:85`, `model.rs:16`, `diarization/mod.rs:240`). #320
(Swift `outOfOrderResponsesAndInteractivePriority`) beror troligen på att testet observerar
trådankomst i `started`-arrayen medan `RequestScheduler.start` dispatchar på en concurrent
queue (`ProtocolServer.swift:42-50,127-141`). Ingen flake-detektor; retries skulle dölja
racen.

**Omfattning.** Byt väggklocksgränser mot event-/condition-väntan eller injicerad `Clock`;
höj ticks; skicka test-flaggan genom `probe()` i stället för env; ta bort `cfg(test)` ur
`logging/service.rs` via injicerad loggsökväg; fixa #320 genom att öppna gates en i taget
eller registrera pick-ordning vid pump; nattligt jobb `cargo test -p sagascript-core --test
engine_host` ×20 under CPU-stress som öppnar issue vid fel. Inga automatiska retries.

**Acceptans.** `rg 'elapsed\(\) <|Duration::from_millis\([0-9]+\)\)' tests/` utan tids-
assertioner på väggklocka; `rg 'set_var' crates/*/src` = 0 utanför `#[cfg(test)]`-fri kod;
20 nattkörningar i rad gröna innan ticketen stängs.

### T36 · Arkitekturbeskrivning, recept och ADR:er för agenter och människor

**Labels:** `documentation`, `priority:P0` · **Prio:** P0 · **Effort:** M

**Problem.** AGENTS.md (2,3 kB) och CONTRIBUTING nämner inte `engine-host/{coreml,ort,
test-vectors}`, `crates/sagascript-engine-protocol`, `scripts/`, `tests/`, `tooling/`,
`site/`, eller mötesmodulerna. Det finns ingen beskrivning av dataflödena (hotkey → record →
transcribe → paste med main-thread-regeln; settings-lagren disk/`AppController`/watcher/
frontend; updater-faserna; engine-host-livscykeln) och inga recept för de vanligaste
ändringarna. Invarianter (CLI-first, privacy-first, main-thread, "poison policy", "dist måste
byggas") finns bara som enstaka rader eller i commit-meddelanden. Resultatet är att varje agent
upptäcker samma fallgropar på nytt (se T05) och att refaktoreringar i våg 2–3 saknar en karta
att hålla sig till.

**Omfattning.** `docs/ARCHITECTURE.md` (≤ 400 rader: crate-karta med ansvar, de fyra
dataflödena som sekvensdiagram i Mermaid, trådmodell/lås-ordning, feature-flaggor, var
tester bor); `docs/recipes/` med fem steg-för-steg-recept: lägg till ett Tauri-kommando (Rust +
`contract.ts` + mock + test), en inställning (Settings-struct + migrering + CLI-nyckel +
UI-panel + paritetstest), ett CLI-subkommando (`Out`, `--json`, exitkoder, smoke), ett event,
ett nytt engine; `docs/adr/` med ADR-0001…: CLI-first, privacy-first, main-thread-regeln,
poison-policy (T10), feltyper över gränser (T12), teststrategi (vilka lager, inga källtext-
tester); uppdatera AGENTS.md "Code layout" + "Verification" till att peka på `npm run verify`
(T05) och ARCHITECTURE; PR-mall i linje. Håll dokumenten korta och länkade — inte en kopia av
koden.

**Acceptans.** En ny agent-session får i uppgift "lägg till inställningen X" och behöver inte
läsa mer än receptet + de filer receptet pekar på (testa med en faktisk liten feature-PR);
`scripts/audit-agent-instructions.sh` (ägarens) grönt; alla tre verifieringsdokument citerar
samma kommando.

### T37 · Dokumentstädning, döda skript och bredare docs-only-CI

**Labels:** `documentation`, `package:ci` · **Prio:** P2 · **Effort:** S

**Problem.** Se 2.6. Dessutom: `scripts/ci-docs-scope.mjs:7-11` tillåter bara `README.md`,
`docs/release-readiness.md`, `docs/research/**/*.md` som docs-only, så varje annan doc-ändring
kör full 3-OS-CI. Dead-script-kandidater (noll referenser i trädet): `scripts/benchmark-
batch-transcribe.py`, `scripts/ci/build_metadata_fixture.sh`, `scripts/smoke-single-instance-
macos.sh`, `scripts/dictation_eval/duration_selector.py`; `diarization_sv/local_*_evidence.py`
refereras bara av egna tester. `src-tauri/gen/schemas/` spåras (4 filer, `desktop-schema.json`
= `macOS-schema.json`) med asymmetrisk allowlist i `ci-build-identity.mjs`. `site/` är ett
separat React-projekt som inget bygger eller lintar.

**Omfattning.** `docs/archive/` för `presenter-mode-design.md`, `windows-port-plan.md`,
`ci-performance.md`, `transcribe-review.md`, `testing/integrated-pr-review.md`,
`glossary-scoping-decision.md`; slå ihop `INSTALLING.md` + `docs/installation.md` + README-
sektionen till en; länka mötesdokumenten och `personal-glossary.md` från README:s
Documentation-sektion (eller `docs/README.md` som index); markera `meeting-review-usage.md`
rätt status; ta bort döda skript efter bekräftelse (lista i PR); bredda allowlisten till
`docs/**`, `*.md` på toppnivå, `.github/ISSUE_TEMPLATE/**`; besluta om `gen/schemas` ska
ignoreras; besluta `site/`:s hemvist (eget repo eller egen CI-lane).

**Acceptans.** Varje `docs/**/*.md` har minst en inbound-länk eller ligger i `archive/`;
`node scripts/ci-docs-scope.mjs --base … --head …` klassar en ren `docs/foo.md`-ändring som
docs-only; `git ls-files scripts | wc -l` minskat med de bekräftade döda.

### T38 · Git-hygien (engångs, ägaren kör; destruktiva delar kräver separat bekräftelse)

**Labels:** `chore` · **Prio:** P2 · **Effort:** S · **Relaterat:** #258

**Problem.** 89 worktrees varav 52 prunable; 259 lokala branches i primärcheckouten; 111
remote-branches varav 36 redan mergade i `origin/main` och 18 utan commits på 60 dagar; lokal
`main` låg 54 commits efter `origin/main` (den här utvärderingen gjordes först mot fel
revision innan det upptäcktes — samma risk för varje agent som startar från checkouten);
packfilen är 279 MB varav 342 MB okomprimerat är tre 60-minuters WAV-filer i historiken
(`flowdictate-tauri/benchmark/audio/{en,sv,no}_60min.wav`).

**Omfattning.** (1) `git worktree prune`, radera lokala branches som är mergade
(`git branch --merged origin/main`), `git fetch --prune`, fast-forwarda lokal `main`.
(2) Radera de 36 mergade remote-branches (`gh api` eller `git push --delete`), granska de 18
inaktiva med ägaren. (3) `delete_branch_on_merge` (T03) så det inte återkommer. (4) Beslut:
historierensning av WAV-blobbarna med `git filter-repo` ger ~75 % mindre clone men skriver om
alla SHA:n (bryter länkar i issues/Munin och alla befintliga worktrees) — rekommendation:
gör det inte nu; lägg i stället en `.gitattributes`/pre-commit-grind mot filer > 5 MB. (5) En
rad i AGENTS.md: "börja alltid från `origin/main`, inte lokal `main`".

**Acceptans.** `git worktree list | rg -c prunable` = 0; `git branch -r --merged origin/main |
wc -l` ≤ 3; lokal `main` = `origin/main`; grind mot stora filer i CI.

---

## 5. Metod, verifiering och begränsningar

**Vad som kördes.** Alla kommandon i avsnitt 2 kördes av huvudagenten i worktreet
`claude/quality-sprint-audit` på `a69d5d1` (clippy, fmt-check, `cargo test --workspace`,
svelte-check, Node-tester, code-health-tester, Python-tester, `swift test`, npm audit,
clippy-census med temporär `clippy.toml`, GitHub-API för ruleset/repo-inställningar/CI-tider,
git-statistik). Inget i applikationskoden ändrades; `clippy.toml` togs bort efter censusen.
Build-cachen delades med primärcheckoutens `target/` för att slippa 15 min whisper.cpp-bygge.

**Delegering (enligt AGENTS.md).** Sju read-only review-leaves kördes parallellt som Claude
`sonnet` (begärd effort `high`; runtime-observerad modellvariant okänd): app-crate, core,
CLI, frontend, repo/CI/docs, teststrategi, engine-host/protokoll. Huvudagenten (Fable)
verifierade de bärande påståendena i varje rapport mot koden innan de togs in: lås-sites och
`Drop`-impl, duplicerade konstanter, ORT-hostens teststatus, döda API-wrappers, MSRV mot
`cargo metadata`, `sbom.yml`-token, `reset()`-anropare, `after_timeout`-logik, SIGPIPE,
app→cli-importer, settings-driften, typad parse-miss → defaults, `unsafe impl`, PCM-städning,
settings→transcription-cykeln. Bedömning av leaves: alla sju `pass`; tre rättade felaktiga
premisser i mitt uppdrag (kommandoantal 88 inte 98, `meeting_jobs`/`windows_clipboard`/
`meeting_reprocessing` har tester i syskonfiler, Swift-tester körs i CI) vilket är inarbetat.
Skillnader mellan leaves i små tal (ORT-tester 23/33, källtext-tester 13/14) är avrundade i
texten.

**Inte verifierat.** Windows-beteenden (konsolflash, clipboard) — bara kodläsning. #320-
diagnosen är härledd ur koden, inte reproducerad. Att ORT-hostens tester går utan ORT-binär
antas från dess `Cargo.toml`-kommentar. Täckningssiffror är hämtade från
`docs/research/code-health-2026-10-09` (frusen bas `24cbf97`), inte omkörda. Inga
GitHub-issues har skapats; inga ändringar pushade.

**Nästa steg.** Fila T01–T07, T10, T13, T36 först (våg 0 + de P0 i våg 1), i den ordningen;
de är oberoende av varandra utom att T01 bör mergas före allt annat Rust-arbete.
