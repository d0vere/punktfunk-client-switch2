# Correzione bindgen per Skia / Switch 2

Log reale utente (CachyOS): `Ninja [1370/1370] link libskparagraph.a` riuscito.
`skia-bindings 0.153.3` ha poi fallito durante GENERATING BINDINGS:
`src/bindings.cpp:1:10: fatal error: 'cassert' file not found`.

- Root cause: il comando di libclang per `bindgen` vede il target AArch64
  ma non riceve le directory standard C++ della toolchain GCC devkitA64.
- `tooling/prepare_bindgen.sh` scopre include/c++/<version>, la directory
  aarch64-none-elf che contiene bits/c++config.h e il newlib sysroot.
- Il preflight Clang controlla <cassert>, <memory> e <vector> con il target
  aarch64-none-elf. Fallisce esplicitamente se non riesce a interpretarli.
- Il comando Cargo passa i flag a bindgen attraverso il nome variabile
  con trattini `BINDGEN_EXTRA_CLANG_ARGS_aarch64-nintendo-horizon` via `env`.
- `resume_skia.sh` riutilizza i sorgenti e target/Ninja già prodotti dalla
  build utente, oltre alla cache crates.io precedente. Nessuna pulizia
  forzata e nessuna compilazione Skia falsamente certificata.

## Verifiche reali

- 19 test Python host: PASS, 3 test C host: PASS.
- bash -n, py_compile, verifica patch: PASS.
- SDK devkitA64 e libclang del PC utente: **non disponibili qui**, preflight
  **non provato sul vero SDK**.
- Rust-Skia/Ganesh e linking finale: **non validati**.
- NRO: **non generata**.

## Fix regressione test in resume (17-11-2026)

L'esecuzione sul PC dell'utente ha rilevato un errore di isolamento:
`resume_skia.sh` esporta variabili `PF2_*` usate correttamente per il
riutilizzo degli oggetti, ma il test interno `test_reuse_does_not_clean_cached_build`
ereditava tali valori anziché partire dall'ambiente di fixture.
I test ora isolano solo il processo subprocess fittizio (sul quale
verificano la conservazione della sentinel). La build reale mantiene
inalterati i percorsi e non elimina gli artefatti precedenti.
19/19 Python PASS anche in ambiente volutamente contaminato; 3/3 C host PASS.
Cross build del titolo e prova su console: non eseguite in questo container.


## Nuovo link blocker da log utente (17-11-2026)

Correzione collisioni VI confermata; il nuovo rust-lld segnala `mmap`, `munmap`, `pread` non risolti in Skia e ICU. `native/pf2_skia_compat.c` integra soltanto per Switch 2 il compat layer dell’upstream, via `libpf2native.a`, con guard Rust per l’estrazione dell’oggetto. 29 test Python + 4 C host previsti/da rieseguire; cross-link e console non verificati qui. Vedere `report/SKIA_NEWLIB_LINK_FIX.md`.
