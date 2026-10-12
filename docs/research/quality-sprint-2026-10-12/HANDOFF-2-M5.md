# Överlämning: kvalitetssprint Sagascript, del 2 (2026-10-12, till M5)

Fortsättning på `docs/research/quality-sprint-2026-10-12/HANDOFF.md` (gren `claude/quality-sprint-audit`).
**Den överlämningens regler gäller oförändrat:**
- eget worktree per ticket; en ticket = en gren = en PR mot `main`;
- ingen push till `main`, ingen release, signering eller installation;
- oberoende cross-model-review före "klar";
- regressionstest röd → grön för varje bugg; aldrig försvagade grindar;
- aldrig `.env*`/`secrets/**`; fixtures bara i tempkataloger eller via `SAGASCRIPT_SETTINGS_PATH`.

Den här filen beskriver läget när arbetet flyttades från molncontainern till M5, och hur man fortsätter.

## 1. Läge per ticket

### PR-stacken (våg 0 + delar av våg 1)
Alla PR:er går mot `main`. Ägaren mergar **i ordning, med merge-commit** (inte squash). Squash bryter `.git-blame-ignore-revs` (#352, och #361:s formateringscommit).

| Ordning | PR | Ticket | Gren / head | Status |
|---|---|---|---|---|
| 1 | #385 | #352 rustfmt + fmt-grind | `claude/352-rustfmt` ab64059 | klar för review, CI grön, Sonnet-review |
| 2 | #386 | #353 toolchain-pin 1.99.0, MSRV 1.88, Node 22 | `claude/353-toolchains` c1b267d | klar, CI grön |
| 3 | #387 | #354 workflow-härdning | `claude/354-workflow-hardening` 78d2143 | klar, CI grön (`Refs #354`: ägarens inställningar återstår, se §3) |
| 4 | #389 | #357 oanvända testsviter i PR-CI | `claude/357-ci-unrun-suites` f913cad | klar, CI grön |
| 5 | #390 | #382 ARCHITECTURE, recept, ADR 0001–0004 | `claude/382-architecture-docs` 07f8a24 | klar, CI grön, Fable-review |
| 6 | #391 | #359 poison-toleranta lås, ADR 0005 | `claude/359-poison-tolerant-locks` 1dc720e | klar, CI grön, Sonnet-review |
| 7 | #392 | #350 `npm run verify`, testgrupper, CI-paritet | `claude/350-local-verify` ec26364 | klar, CI grön, Sonnet-review (8 should-fix åtgärdade) |
| 8 | #393 | #355 en reqwest-stack, cargo-deny, npm audit | `claude/355-dependency-policy` 3d21eca | klar, CI grön, Sonnet-review |
| 9 | #394 | #356 clippy-policy med baslinje, död kod bort | `claude/356-clippy-policy` 48a792c | klar, CI grön, Fable-review |
| sidogren | #388 | #340 bevara ogiltig settings-fil (.bak) | `claude/340-settings-preserve` 14b8937 (på #385) | klar, CI grön, Fable-review. Merga när som helst efter #385 (test-merge mot kedjan är konfliktfri). |
| 10 | #396 | #361 del A1 (Vitest, typad IPC-mock, Prettier) | `claude/361-vitest-eslint` a6b39fa (på #394) | **draft**: Sonnet-review delvis klar, inga blockerande fynd; A2 väntar licensbeslut (se §2) |
| — | #395 | #367 Rust↔Swift-protokollkontrakt | `claude/367-protocol-contract` 684f546 (på c1322e9 = äldre #356-head) | **draft, WIP**: CI på 684f546 är första Swift-kompileringen (se §2) |

Stackade grenar uppdateras med **merge-commits** (t.ex. `Merge branch 'claude/350-local-verify' into claude/355-dependency-policy`), inte rebase. Då förblir grenarna ovanpå giltiga. Rebase görs bara på grenar utan ättlingar och utan öppen PR.

### Pushade WIP-grenar (ingen PR än, utom #367)

| Ticket | Gren / head | Bas | Läge |
|---|---|---|---|
| #365 settings: `schema_version`, migreringsmodul, lenient per-fält-parsning, fixturkorpus, ADR 0007 | `claude/365-settings-migrations` a6cc424 (3 commits) | #388 (14b8937) | Implementerad, 1486 tester gröna lokalt. **Fable-review: 1 blockerande, 1 should-fix** (se §2). Ingen PR än. |
| #366 engine-host-övervakning som återhämtar sig | `claude/366-engine-host-recovery` ac8361d (10 commits) | #391 (1dc720e) | Alla 8 punkter och acceptanstest (a)–(d) klara, rött → grönt. Ingen review och ingen PR än (se §2). |
| #367 Rust↔Swift-protokollkontrakt | `claude/367-protocol-contract` 684f546 (5 commits), PR #395 (draft) | c1322e9 (äldre #356-head) | Punkt 1–3 och 5 klara, 4 delvis, 6 ej påbörjad. Swift-koden kompileras för första gången i CI på 684f546 (se §2). |
| #363 delad testsupport + tester ut ur gud-filerna | `claude/363-test-support` 0c58428 (4 **WIP**-commits) | c1322e9 | **Okompilerat.** Bygglåset var låst i ~45 min (se §5). Crate + ersättningar skrivna, flytten av inline-tester ej gjord (se §2). |
| #361 del A1 | `claude/361-vitest-eslint` a6b39fa, PR #396 (draft) | #394 (48a792c) | Klar. Sonnet-review delvis klar, inga blockerande fynd. A2 väntar licensbeslut. |

### Ej påbörjade
- **Våg 0:** #358 (CI-dedupe: görs sist eftersom den rör allt i ci.yml).
- **Våg 1:** #341, #347, #360, #362, #364, #368, #342.
- **Våg 2:** #369, #370, #346 + #373, #343, #371, #372, #374.
- **Våg 3:** #344 + #375, #345, #379, #376, #377, #378, #348/#349, #380, #381.
- **Repo:** #383, #384 (destruktiva delar bara med ägarens uttryckliga bekräftelse).

## 2. Öppna arbetsuppgifter, i rekommenderad ordning
Säkerhetsregel för allt nedan: rebasa inte en pushad gren som har ättlingar. Uppdatera i stället med en merge-commit.

1. **Ägaren:** licensbeslut och merge av stacken (§3). Efter merges: `git merge origin/main` in i öppna grenar, så krymper diffarna.
2. **#366: review och PR.** Implementationen är klar (agentens rapport):
   - **Commits:**
     - `bc430e4`: medvetet röda acceptanstester och tre nya fake-host-lägen (`FAKE_CRASH_ON_START_FILE`, `FAKE_WEDGE_ONCE_FILE`, `FAKE_STOP_READING_ONCE_FILE`).
     - Därefter en fix-commit per punkt: stdin via skrivtråd med timeout och `terminate` utan skrivlåset; omstart av en host som fastnat (grace 5 s efter cancel, baserat på det slutliga svaret per request, inte `status.in_flight`); `Failed` med 60 s cooldown och sedan ett nytt försök; backoff som går att avbryta och inte håller låset; PCM-svep; `CREATE_NO_WINDOW` på Windows; inga `lock().unwrap` kvar; `load_progress`/`busy` dokumenterade som reserverade.
   - **Medvetet inga `reset()`-anrop.** `engine doctor` bygger sin egen klient i en egen process, motorinställningar byter redan ut den delade klienten, och det finns ingen manuell retry. Ärendets påstående stämmer alltså inte på den punkten.
   - **PCM-svepet:** varje katalog har en `owner.lock` med exklusivt fs2-lås. En katalog tas bort bara om:
     - namnet matchar exakt;
     - det är en riktig katalog (inga symlänkar);
     - markören finns;
     - svepet själv får låset.

     Döda kataloger döps först om till `sagascript-pcm-dead-*`. Bara antal loggas. Svepet kontrollerar inte filägare i Linux delade `/tmp`; det kan behöva läggas till.
   - **Grindar:**
     - Workspace 1473 passed / 0 failed / 2 ignored (på `c0af065`).
     - clippy (alla tre), lean CLI 266 och MSRV gröna på `7201f28`.
     - På slutcommiten `ac8361d`, som bara rör `tests/engine_host.rs`: fmt plus en körning av engine_host, 52 passed.
     - Flakighet: 7 rena körningar i rad.
   - **Kvar:**
     - Kör grindarna på `ac8361d`.
     - Oberoende review: stdin-skrivtråden, watchdog-tidsgränserna och svepets säkerhet. Nya standardvärden: 60 s cooldown, 5 s grace, 5 s skrivtimeout.
     - Windows-koden (`#[cfg(windows)]` i `process.rs` och Windows-vägarna i test (d)) kunde inte kompileras och verifieras först i CI.
   - **Basera om på kedjetoppen** med en merge-commit av `origin/claude/356-clippy-policy`. Kör `clippy-policy` (antalen får inte öka). Samordna med #367: striktare protokolltyper, och ORT-hosten svarar `bad_request` på en felformad hello. Öppna PR med `Closes #366`.
3. **#365: åtgärda reviewn och öppna PR.**
   - **BLOCKERANDE (fanns redan före PR:en, men PR:en och ADR 0007 lovar motsatsen):** en oparsbar post i `profile_models`, t.ex. ett modell-id från en nyare release, tas bort av `drop_invalid_profile_models` innan giltigheten bedöms (`store.rs` ~393–428). `check_settings_object` ger då `Ok`. Starten migrerar (steg 3 sätter `auto`) och skriver om filen **utan `.bak`**, så användarens val försvinner.
     - Åtgärd: räkna bortplockade `profile_models`-poster som `reset`. Då bevarar sparvägen originalbytes, och starten migrerar bara i minnet.
     - Testet `invalid_profile_model_does_not_reset_language_hotkey_or_other_settings` (`store.rs` ~1549) befäster idag det felaktiga beteendet. Skriv om det: rött före, grönt efter.
   - **SHOULD-FIX:**
     - **Scenario:** en fil med ett ogiltigt fält och väntande steg 5/6 (fil från ≤ 1.4.1). En fältuppdatering via `update`/`try_update` → `load_from` (utan maskinprob) skriver då gamla markörer och `schema_version: 4`. Nästa start, eller watchern efter ~1 s, kör steg 5/6 över val som användaren gjort däremellan (`engine_prewarm`, profilmodell).
     - **Åtgärd:** ge `update`/`try_update` maskinproben, eller spara de i minnet migrerade markörerna vid första save.
     - **Test:** användarval mellan reparation och omstart.
   - **Nits:**
     - ADR 0007 "Adding a migration": stryk "(or rely on `schema_version` alone …)". Varje värdeändrande steg måste vara markörgardat, eftersom nyare filer skrivs tillbaka som 6.
     - `load_from` faller tillbaka på `defaults_for` vid `migrate`-fel. Det är latent, men en framtida felande steg-funktion skulle återinföra #340.
     - Hela `hotkey_profiles` nollställs om en enda profil har ett okänt språk; per-post-lenient vore snällare.
   - **Sedan:**
     - Basera om på kedjan, så att #365 kommer efter #388 och efter #394. Enklast efter att ägaren mergat, annars en merge-commit av `origin/claude/356-clippy-policy` in i grenen. Väntad konflikt: `store.rs`, där #353 ändrade en rad till `FileExt::unlock(&lock_file)`.
     - Lägg till indexraden för ADR 0007 i `docs/adr/README.md`.
     - Kör `node scripts/clippy-policy.mjs`; nya `expect`/`unwrap` i produktionskod får inte öka baslinjen.
     - Kör `npm run verify`.
     - Öppna PR med `Closes #365` och redovisa review-fynden.
4. **#367: driv PR #395 till grönt** (agentens rapport sammanfattad):
   - **Läge och lokala kontroller:**
     - CI på 684f546 är den första kompileringen av Swift-ändringarna (test-macos) och av ORT-hostens nya golden-tester (test-linux). På M5: kör `swift test` i `src-tauri/engine-host/coreml` och `cargo test --locked` / `cargo clippy --all-targets --locked -- -D warnings` i `src-tauri/engine-host/ort` lokalt.
     - Ej körda: full workspace test/clippy, `node scripts/clippy-policy.mjs` och konformanssubsetet mot ORT-hosten.
   - **Klart:**
     - 7 gyllene transkript plus `fake-engine.json` i `engine-host/test-vectors/protocol/`. De täcker alla 8 op och alla 10 felkoder. `vector_tests.rs` gröna, och cargo-featuren `golden` gör läsaren återanvändbar.
     - Swift `WireErrorCode` och `WireProtocol.swift`. Ärendets påstående var fel: literalerna låg inte i EngineTypes.swift utan i ProtocolServer (19), CoreMLEngine (27), TdtDecoder (10), CompiledModelCache (5) och FloatTensor (4).
     - Swift-semantiken följer Rust:
       - `priority` krävs;
       - `hello` kräver `client.name`, `version` och `git_sha`;
       - tom op ger `unsupported`.
     - Borttagna serde-defaults: ErrorBody, HostInfo, Capabilities, LoadResult, TranscribeWindowResult och StatusResult. Ett test tar bort varje obligatoriskt fält och kräver avkodningsfel.
     - `engine-host-conformance.py --no-model` i test-macos, med motsvarande macOS-kontroll i verify.
     - Delad identitetslista `scripts/build-identity-generated-paths.txt` (ort/build.rs och ci-build-identity.mjs läser listan i stället för att anropa bash, på grund av Windows) och `check-engine-host-identity.sh --release`.
   - **Kvar:**
     - Rött → grönt-demon: döp om `vocab_size` i Swifts `WireProtocol.loadResult`; `swift test` ska då falla.
     - ARCHITECTURE §5-raderna för test-vectors och Conformance.
     - Fullständig konformans i release-gaten kräver modellnedladdning (~640 MB) och Swift-bygge där. Ta beslutet eller lämna den manuell, men dokumentera.
     - Punkt 6 (Swift-tester för TdtDecoder och readSamples, ≥ 60 % täckning): ej påbörjad. Överväg att låta #348/#349 ta den.
   - **Upptäckt:** `pumpLocked` i Swift-schemaläggaren räknar varje startat jobb dubbelt (`running.count + result.count < maximum`), så den kan starta färre jobb än lediga platser. Eget ärende eller fix med test.
   - **Beteendeskillnad mot den Rust-skrivna ORT-hosten:** felformad hello ger nu `bad_request` i stället för `protocol`, och kuvertfel ekar id. Kontrollera mot #366:s klient.
5. **#363: gör WIP:en kompilerbar** (agentens rapport):
   - **Kompilera och kör grindarna:**
     - Kör `cargo check --workspace --all-targets --locked`. Cargo.lock är **handredigerad** för det nya path-cratet; faller `--locked`, generera om låsfilen.
     - Rätta kompileringsfel, kör alla grindar och skriv om WIP-commitarna.
   - **Design:**
     - Separat crate `crates/sagascript-testsupport`: `TempDir`, `sha256_hex`, `isolated_cli_env`, `ORDINARY_OS_ENV`. Det beror inte på core, så ingen cykel uppstår.
     - `sagascript_core::test_support::meeting_transcript()` ligger bakom `cfg(any(test, feature = "test-support"))`.
   - **Ersättningar:**
     - `temp_dir`-platser: 56 → 18, varav 15 i `store.rs` (fas 2). Väntat slutläge 3 motiverade.
     - RAII-guards 9 → 1, sha256-kopior 4 → 0, env-scrub-loopar 2 → 0, 10 transkriptbyggare.
   - **Inte gjort: flytten av inline-tester till syskonfiler.** Recept från agentens torrkörning (som gick förlorad):
     - Flytta varje toppnivå-`#[cfg(test)]`/`#[cfg(all(test, …))]` med kommentarer till `foo_tests.rs`, lägg `#[cfg(test)] #[path = "foo_tests.rs"] mod tests;` sist i filen, och behåll diarization-cfg:erna exakt.
     - `commands.rs`: flytta även `reconcile_global_dictionary_after_conflict` och `set_initial_prompt_in`. `macos_mic::tests` blir `#[cfg(target_os = "macos")] mod macos_mic_tests` och kräver `pub(super)` på `authorization_status_label`, `AccessRequestDecision` och `access_request_decision`.
     - `main.rs`: flytta även `#[cfg(test)] use …{validate_hotkey, Language}` och `startup_hotkey_candidate`.
     - `transcribe.rs`: `mod tests`-deklarationen måste ligga sist. Testet `language_detection_never_downloads_an_encoder` läser källtexten fram till första `#[cfg(test)]`.
     - Lägg rustfmt-pass i en egen commit.
     - Normalisera testnamnens nya `tests::`-nivå vid jämförelse.
   - **Baslinje på c1322e9:** 1455 passed, 0 failed, 2 ignored, 1457 listade. Per binär: app 251, CLI lib 311, core lib 782, protocol 15. Ta om `--features diarization/record`, `--no-default-features` och lean CLI.
   - **Fas 2:** `store.rs`/`manager.rs` efter att #365 är klar.
6. **#361:**
   - A2 efter licensbeslutet: happy-dom, @testing-library/svelte, jest-dom och user-event; Settings-piloten (`test-settings-layout`, `test-settings-recovery`) med en mappning gammal → ny assertion; ESLint och `npm run lint`.
   - Reviewns nits:
     - `hotkey.js` i Rust-kommentarer;
     - `allowJs`/`checkJs`;
   - Reviewns overifierade punkter:
     - mocken mot `@tauri-apps/api` 2.10.1;
     - `CommandMap` för wrappers vars wire-namn skiljer sig från funktionsnamnet;
     - CRLF på Windows-CI;
     - dokumentationen.
   - Sedan fas B: B1 Settings, B2 FileTranscription och möten, B3 Onboarding och App (plan i `docs/testing/frontend.md`).
7. **Våg 1, resten:**
   - **#364** CLI-utdata och exitlager: efter #363:s flytt av `transcribe.rs`-testerna.
   - **#368** async och nedladdning: efter #363, eftersom den rör `download.rs`.
   - **#342** reprocessing-publicering: efter #363.
   - **#347** kontrakt mellan Rust och TS: efter #361 A1. Byt den handskrivna `CommandMap` i `src/test/ipc.ts` mot den genererade. Därefter **#360** (feltyper, ADR 0006).
   - **#362** och **#341:** efter #361 A2.
8. **Våg 2 och 3, samt #358** (CI-dedupe, när CI-ändringarna lagt sig) och **repo #383/#384**, enligt ursprungliga HANDOFF.md. Hårda beroenden:
   - #363 → #369/#370/#343/#371;
   - #361 → #341/#345/#379;
   - #359 → #344;
   - #346 → #343;
   - #342 → #374.

## 3. Väntar på ägaren
1. **Licensbeslut för #361** (blockerar A2, och därmed #341, #345, #362, #379). happy-dom ≥ 20.8.9 drar in BSD-2-Clause (`entities@7`). ESLint drar in BSD-2-Clause (espree, eslint-scope, …) och BlueOak-1.0.0 (`minimatch@10`). Alternativ, se [kommentaren på #361](https://github.com/Magnus-Gille/sagascript/issues/361#issuecomment-6123062674):
   1. lägg till båda i `reviewedNpmLicenses` (rekommenderas; dev-only, OSI-godkända);
   2. bara BSD-2-Clause (komponenttester ja, ESLint nej);
   3. oxlint i stället för ESLint.
2. **Merga stacken** i ordningen ovan, med merge-commit.
3. **Inställningar från #354:** `check-linux` som required check i rulesetet, `delete_branch_on_merge`, Dependabot alerts. Om pyke-CDN:en behövs i en molnmiljö: tillåt `cdn.pyke.io` (ort-sys).

## 4. Konventioner som etablerats (följ dem)
- **`npm run verify`** (`scripts/verify.mjs`) är den enda lokala kontrollistan.
  - `scripts/test-verify-ci-parity.mjs` kräver att **varje** kommando i ci.yml:s testlanes motsvaras av en verify-kontroll (`ci:`-listan) eller en allowlistad orsak. Varje jobb måste vara klassat, och även namnlösa steg räknas.
  - **Nytt CI-steg ⇒ ny verify-kontroll eller allowlist-post i samma PR.**
- **Node-testmanifestet** `scripts/node-test-groups.mjs`: varje `scripts/**/test[-_]*.mjs` måste ligga i exakt en grupp (`ui`/`ci`/`release`/`codeHealth`), annars faller `npm run test:frontend`.
- **Clippy-policy** (`node scripts/clippy-policy.mjs`, baslinje `src-tauri/clippy-policy-baseline.json`):
  - Antal `unwrap/expect/panic/too_many_lines/…` i produktionskod får **inte öka**.
  - Har man sänkt något: `--update` i samma PR.
  - Mäts bara på Linux, så på macOS hoppar verify över den. CI test-linux är grinden.
- **cargo-deny** (`src-tauri/deny.toml`) och `npm audit --audit-level=high` är grindar. En ny advisory gör alla PR:er röda: bumpa cratet eller lägg en `ignore` med id och skäl.
- **Licenser:** `scripts/generate-third-party-notices.mjs` inventerar även dev-beroenden. En ny licensfamilj är ett ägarbeslut.
- **Formateringscommits** (rustfmt i #352, Prettier i #361) ligger som rena, reproducerbara commits med SHA i `.git-blame-ignore-revs`. Vid rebase måste SHA:n uppdateras (eller formateringen göras om).
- **Prettier** gäller `src/`, `scripts/`, `tests/` utom `.svelte`, tills fas B tagit bort källtexttesterna.
- **ADR:** `docs/adr/` (0001–0005 accepterade). 0006 är reserverad för #360 (feltyper). 0007 skrivs i #365 men indexraden saknas, eftersom #365 ligger på en gren utan `docs/adr/README.md`.
- **PR-text:** svenska.
  - Sektioner: What, Why, Testing (rött före / grönt efter, exakta tal), Oberoende review (vilken modell, verifierat, åtgärdat, medvetet kvar), Avvikelser.
  - Avsluta med `Closes #N` (eller `Refs #N` för delleverans) och Claude Code-raden.
- **Commit-trailer:**
  ```
  Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
  Claude-Session: https://claude.ai/code/session_01VLB95dUMArEYVUQNerRDQb
  ```
  Ny session: använd dess egen session-URL.
- **Reviewer:** annan modell än författaren (Sonnet 5.5 eller Fable 5.1), read-only. Fynden åtgärdas i en separat commit och redovisas i PR-texten.

## 5. Fallgropar vi gick i
- **Avbrutna CI-körningar** (en ny push avbryter föregående) visar `check-linux/macos/windows: failure`. Det är aggregatorns reaktion på `cancelled`, inte ett riktigt fel. Kontrollera alltid den aktuella headens körning.
- **Döda testhjälpare på Windows:** en hjälpare som bara används av ett `#[cfg(unix)]`-test är död kod på Windows och fäller `clippy -D warnings` (#391). Gata hjälpare med samma cfg som deras enda anropare.
- **`rust-version = 1.88`** + clippy `incompatible_msrv`: inga std-API:er stabiliserade efter 1.88 (#386 föll på `File::unlock`; använd `FileExt::unlock`). Kör clippy efter att ha rört MSRV-känslig kod.
- **`continue-on-error`:** ett stegs `conclusion` i Actions-API:t blir `success` även när steget faktiskt föll. Läs `outcome` eller annotationer (Windows-smoke maskerades så; #389 rapporterar nu maskerade fel som `::warning`).
- **Delad `CARGO_TARGET_DIR` mellan worktrees** ger inaktuella artefakter för workspace-crates, eftersom cargo spårar dep-info relativt paketroten. **På M5: en target-katalog per worktree**, eller en wrapper som rör källorna vid worktree-byte.
- **Delad target-katalog och fake-hosten:** en annan worktrees bygge skrev över `target/debug/sagascript-fake-engine-host` mitt i en testloop. Kör därför testerna via `cargo test` med egen target-katalog, inte testbinären direkt.
- **Bygglåset deadlockade** i ~45 min när `locked.sh` (som håller låset) körde `bash -c '. sagaenv.sh && cargo …'`, eftersom `cargo()`-wrappern väntade på samma lås. På M5 behövs inget delat lås.
- **Disk:** testbinärer är ~370 MB styck. Två beroendegrafer (reqwest 0.12 resp. 0.13 i olika grenar) dubblar allt.
- **Samtidiga agenter i samma grenstack:** om underliggande PR:er ändras medan en agent arbetar, återspela agentens commits på den nya basen (cherry-pick), eller merga basen. Gör om formateringscommits i stället för att lösa konflikter i dem.
- **Containermiljön** (gäller inte M5): `cdn.pyke.io` var blockerad (ORT länkades dynamiskt), gh GraphQL var blockerad (använd `gh api` REST), och där fanns ingen Swift.

## 6. Uppföljningar som upptäckts (inte gjorda, föreslås som egna ärenden)
- **Beroenden:**
  - `cargo update -p anyhow -p memmap2` rättar två unsound-advisories (RUSTSEC-2026-0190 och -0186).
  - notify 7 → 8 och indicatif 0.17 → 0.18 tar bort två unmaintained transitiva crates.
- **npm:** `site/package-lock.json` har 24 sårbarheter (3 critical), `tooling/code-health/package-lock.json` 4 high. Ingen av dem ingår i appen eller i audit-grinden.
- **Död kod:**
  - `AppController::model_ready` / kommandot `is_model_ready` returnerar alltid false (ingen skriver fältet).
  - `AppState::Error` konstrueras aldrig.
- **Clippy-policy:** separata baslinjer för macOS och Windows. Idag mäts bara Linux.
- **Windows-smoke:** de maskerade felen (13 av 24 körningar sedan 2026-10-09). Hypotes: en icke-portabel whisper.cpp-build i den delade cachen.
- **`build.rs`** körs om vid varje bygge i worktrees som saknar `.git/packed-refs`.
- **Skört test:** `download::tests::stamp::unwritable_directory_does_not_fail_verification` avgör root via `$USER`.
- **Swift-schemaläggaren** `pumpLocked` räknar startade jobb dubbelt (från #367).
- **Från tidigare:**
  - självläkande `load_lock` i whisper;
  - panik-loggar till JSONL-apploggen;
  - unikhetstest för `ENGINE_LOAD_STATE` (#347).
- **#365, nit:** hela `hotkey_profiles` nollställs om en profil har ett okänt språk.

## 7. Komma igång på M5
```bash
git clone https://github.com/Magnus-Gille/sagascript.git && cd sagascript
git fetch origin '+refs/heads/claude/*:refs/remotes/origin/claude/*'
# Ett worktree per ticket. Varje worktree får sin egen src-tauri/target, så det
# behövs inget delat lås och inga inaktuella artefakter. Räkna med ~10–20 GB per worktree.
git worktree add ../wt/365 claude/365-settings-migrations   # osv. per gren
cd ../wt/365 && npm ci && npm run build                    # cargo bygger app-craten först när dist/ finns
npm run verify -- --list                                   # vad som körs; preflight säger vad som saknas
```

**Förutsättningar:**
- rustup; `rust-toolchain.toml` väljer 1.99.0. För `msrv`-kontrollen även `rustup toolchain install 1.88 --profile minimal`.
- Node 22, Python ≥ 3.10, Xcode 16+ med Swift 6 (`swift test`).
- `cargo install --locked cargo-deny --version 0.20.2`.

**Detta går nu att köra lokalt** men kunde inte köras i containern:
- `npm run verify` i sin helhet, inklusive `swift` (bygger engine-hosten och kontrollerar revisionen);
- macOS-clippy och macOS-tester för `cfg(target_os = "macos")`-kod;
- `cargo tauri build --debug` för TCC-tester (mikrofon, tillgänglighet).

**Signering:** bara via `APPLE_SIGNING_IDENTITY`, aldrig i `tauri.conf.json`. Ingen release eller notarisering.

**Försiktighet på en riktig användarmaskin:** testerna använder tempkataloger eller `SAGASCRIPT_SETTINGS_PATH` och får aldrig röra `~/Library/Application Support/…`, de riktiga inställningarna eller inspelningar. Kör inte den installerade appen mot testdata.

**GitHub:** `gh` med GraphQL fungerar normalt där. Prenumerera på PR-aktivitet igen i den nya sessionen.

**Commit-trailer:** använd den nya sessionens URL i `Claude-Session:`.
