# Rapporto di verifica — integrazione Rust Switch 2

## Stato aggiornato dal log del 17 novembre 2026

La build dell'utente sul proprio ounce-devkit ha superato la compilazione
Rust-Skia (GN/Ninja, bindgen), il crate `pf-nx-switch2-platform` e il frontend
ufficiale fino al link finale. Il fix precedente dei duplicati di `vi.rs`
è confermato; il nuovo errore del linker riguarda `mmap`, `munmap`, `pread`
non forniti da newlib. Questa revisione collega il modulo limitato
`pf2_skia_compat.c` tramite la libreria nativa e Rust anchor. I controlli
in questo container superano 29/29 Python e 4/4 C host, ma **nessun link
AArch64 con questa patch è stato eseguito qui; nessuna NRO generata**.
Il percorso `mmap` emulato non è equivalente all'implementazione kernel:
vedi `report/SKIA_NEWLIB_LINK_FIX.md`.


Data del pacchetto: 16 novembre 2026. La verifica qui descritta è riproducibile nel container in cui è stato creato questo archivio; è distinta dai risultati sulla macchina dell'utente.


## Aggiornamento dalla cross-build reale riportata dall'utente (17 novembre 2026)

L'utente ha eseguito il pacchetto Integration_Work sul proprio CachyOS:
- 10 test Python e 3 test C host superati;
- commit ufficiale pin verificato, sette sorgenti C cross-compilati in `libpf2native.a`;
- Rust nightly e crate ufficiali compilati fino a `skia-bindings v0.153.3`;
- **blocco osservato:** precompilato Skia per Horizon in HTTP 404 e fallback
  non patchato `third_party/zlib/BUILD.gn` -> `Unsupported ARM OS`.

La presente revisione aggiunge lo script per preparare il sorgente Skia
ufficiale con patch Horizon e wrapper ounce-devkit. Lo script e i wrapper sono
verificati con test host e analisi sintattica. **Non è stato possibile eseguire
nuovamente la cross-build Switch 2 in questo ambiente**; la risoluzione del
blocco GN sul computer utente resta da confermare con un nuovo log. Tutte le
altre lacune funzionali descritte sotto rimangono aperte.

## Gate effettivamente eseguiti

| Controllo | Esito osservato | Metodo |
|---|---|---|
| Repository ufficiale e commit pinned | Verificati tramite GitHub | API di GitHub, SHA `a332988f...` |
| Core pin | Verificato da manifest upstream e codice allegato | SHA `0fc1b6c...` |
| API libounce Switch 2 | Dichiarazioni lette da repository pubblico | `videoDecoder*`, Vulkan VI NN, audout, HID, socket/applet |
| Test Python | **13/13 PASS** (revisione Skia) | `python3 -m unittest discover -s tests -v` |
| NV12 → RGBA con capacità, colore, stride | **PASS host** | Clang `-Wall -Wextra -Werror -fsanitize=address,undefined` |
| Parser H.264 Annex B SPS/PPS | **PASS host** | Clang, ASan/UBSan |
| Strutture e offset C FFI | **PASS host** | `_Static_assert` C 64-bit |
| Patch Rust applicata e rollback su Git temporaneo | **PASS host** | test Python `test_integration.py`, confronto hash e rifiuto user edit |
| Script shell e Python | **PASS** | `bash -n`, `py_compile` |
| Git patch dei file nuovi | **PASS** | `git apply --check` su repository temporaneo |

## Sorgenti scritti ma NON validati sul target

| Componente | Stato |
|---|---|
| Adapter `Platform` Rust e entrypoint ufficiale | Integrati via patch, Rust build **NON eseguita** |
| OunceVK/Skia UI e presentazione | Codice C/Rust scritto; Ganesh GPU mancante; runtime **NON verificato** |
| Decode libounce AV1 e H.264 | Sorgenti scritti, hardware **NON verificato** |
| Audio e controller | Sorgenti scritti, campionamento/rumble **NON verificato** |
| Networking e lifecycle | Core Quinn ufficiale preservato, socket aperti prima di run, drain non implementato |
| Crossbuild C/C++ in questo ambiente | **NON eseguita** (SDK mancante) |
| Build Cargo nightly + Skia GN | **NON eseguita** (Rust e SDK non installati) |
| Linking ELF AArch64 completo | **NON eseguito** |
| Conversione e validazione NRO | **NON eseguita**, nessuna NRO distribuita |
| Test su Nintendo Switch 2 | **NON eseguiti**, console non accessibile |

Le API sono state verificate contro i repository pubblici al momento dell'analisi: **non** contro gli header esatti del pinned SDK dell'utente `20261114`. La compilazione C già riportata dall'utente era riferita al *precedente* archivio a sei moduli, non a questo adapter Rust.

## Checklist console, solo DOPO una build completa riuscita

1. Verificare che l'ELF sia AArch64 e che `elf2nro` parta dalla build corrente (`sha256sum` e report `work/dist`).
2. Avviare la NRO in modalità applicazione, verificare UI Skia leggibile e pairing verso host Punktfunk ufficiale; controllare display Vulkan e risoluzione in handheld e docked.
3. Negoziare H.264 e AV1 separatamente, verificare che i codec siano effettivamente supportati dal decoder, immagini non corrotte, SPS/PPS, frame pacing, colore e frame dropping su stream lunghi.
4. Verificare audio stereo, buffer/underrun, lip sync, pulsanti e stick, rumble e rimozione controller senza input rimasti bloccati.
5. Aprire HOME, sospendere/riprendere, cambiare dock, disconnettere durante traffico intenso e verificare che Quinn non perda la disconnessione o lasci socket/thread attivi.
6. Rieseguire più cicli di start/stop senza perdite di memoria o deadlock; registrare log, firmware, versione OunceVK e checksum binario.

**Conclusione:** progresso concreto nella struttura sorgente e nell'integrazione dell'applicazione ufficiale, ma **nessun superamento dichiarato dei gate target o console**. Una futura release completa richiede ancora Skia/Ganesh, QUIC drain e le verifiche realmente eseguite con SDK/hardware.

## Aggiornamento bindgen (17-11-2026)

Il log utente ha mostrato Ninja Skia `1370/1370` completati. L'errore reale
è diventato `fatal error: 'cassert' file not found` durante la generazione
Rust-Skia bindings. Questa versione prepara gli include C++ devkitA64,
verifica con Clang gli header in un preflight prima della build e aggiunge
riutilizzo opzionale del target Ninja precedente. Il preflight reale e il
linking Rust finale non sono attestati. 19 test host Python e 3 C PASS.


## Correzione ambiente test durante `resume_skia.sh` (17-11-2026)

L'utente ha trovato 1 fallimento su 19 test Python durante `./resume_skia.sh`:
`test_resume_skia.test_reuse_does_not_clean_cached_build`. Il fallimento
non proveniva da Clang/SDK ma dal test che ereditava i percorsi di cache
reali `PF2_*` e li comparava con quelli temporanei della fixture.
Corretto **esclusivamente** l'ambiente di prova: il resume continua a
trasmettere le variabili alla vera build Cargo. Un test Python con
`PF2_*` intenzionalmente valorizzate, e `build_full.sh --host-tests`,
ora riportano 19/19 PASS e 3/3 C PASS. Non sono state eseguite prove
SDK, cross linking o runtime Switch 2.

## Link status dal log utente (17-11-2026)

Il problema `<cassert>` è superato: bindgen preflight PASS, Rust frontend compilato
fino al link finale. Nuovo errore `rust-lld`: duplicati in `vi.rs` e
`libounce.a(default_window.o)`. Fix sotto cfg(pf2_switch2), host tests
aggiornati, nessuna cross-build/link NRO completati qui. Vedere
`report/LINK_VI_FIX.md`.


## Nuovo link blocker da log utente (17-11-2026)

Correzione collisioni VI confermata; il nuovo rust-lld segnala `mmap`, `munmap`, `pread` non risolti in Skia e ICU. `native/pf2_skia_compat.c` integra soltanto per Switch 2 il compat layer dell’upstream, via `libpf2native.a`, con guard Rust per l’estrazione dell’oggetto. 29 test Python + 4 C host previsti/da rieseguire; cross-link e console non verificati qui. Vedere `report/SKIA_NEWLIB_LINK_FIX.md`.


## Prima prova hardware realmente avvenuta (log utente, 2026-11-17)

L'utente ha completato la compilazione reale del client ufficiale su CachyOS con `ounce-devkit`.
ELF e NRO hanno superato i controlli strutturali e l'utente ha avviato l'NRO su Switch 2.
Il runtime originale inizia ma lo schermo rimane nero. Log: 0 font fallback,
50.961 secondi nel warm-up del menu, successivo `Suspend`. La nuova versione
introduce un frame diagnostico Vulkan, prova font tramite `pl:u`, skip del
warm-up solo su Switch 2 e metriche di pixel UI.

**La nuova revisione grafica non e' stata cross-compilata/testata qui su console.**
La qualificazione delle precedenti versioni in questo report e' storica.
Vedere `report/BLACK_SCREEN_DIAG.md` per le azioni e l'interpretazione.
