# Switch 2: hardening patch / matrice di verifica (2026-11-18)

Questa patch si applica allo snapshot `punktfunk-switch2-sorgenti-20261118-162335-UTC.tar.gz`.
Il test host non equivale a una build ounce-devkit, al boot di un NRO o alla validazione su Nintendo Switch 2.

| Review | Intervento | Stato / limite residuo |
|---|---|---|
| F01 | Aggiornamento del guard `test_black_screen.py` alla pipeline Ganesh, nuovi test e gate build | Test host eseguiti; comportamento su console non verificato |
| F02 | Su ogni errore di submit/registrazione la presentazione si invalida; resize ricrea fence **e** semafori, wait fence limitato | Recupero soft da verificare su console; `VK_ERROR_DEVICE_LOST` resta irreversibile senza un nuovo `VkDevice` |
| F03 | `Display::read_back` legge i pixel dalla superficie Ganesh, non da un buffer CPU obsoleto | Disponibilità/readback GPU dipende dal driver; errori segnalati esplicitamente |
| F04 | `PF2_SCANOUT_PROBE=1` mostra un pattern scanout Vulkan prima del menu | Test ottico/manuale necessario; disabilitato di default |
| F05 | A/B sperimentale `PF2_ASYNC_GANESH_SUBMIT=1` sulla stessa coda | Wait nativo `vkQueueWaitIdle()` mantenuto; 120 Hz non certificati; profilo GPU ancora necessario |
| F06 | Cache video RGBA allocata solo quando utile; oltre 1080p una sola copia framebuffer principale | Un overlay 4K usa più CPU per ricomporre il frame; misurare RAM e FPS |
| F07 | Buffer audio PCM allineato riutilizzato e ingrandito solo quando serve | `audoutPlayBuffer()` supposto sincrono come nel percorso precedente; verificare SDK target |
| F08 | `pread` compatibilità Skia protetto da lock C11; test multithread host | Non è una `pread` POSIX atomica rispetto a chiamate esterne `read/lseek` sullo stesso open-file description |
| F09 | AV1 Main10/HDR non pubblicizzati nella build di default; `PF2_ENABLE_MAIN10=1` sperimentale | HDR10 nativo *non* implementato; l'input Main10 viene convertito a SDR |
| F10 | `StreamSpec.smoothing` governa il buffer da 1 a max 2 frame, con pulizia a ogni nuova sessione | Politica effettiva da validare con streaming a 60/120 Hz |
| F11 | Guard di range su slot input, motion e rumble | Verificare pad reali, disconnessione e ri-associazione |
| F12 | Documentazione lifetime delle copie decoder nei tipi FFI | Niente nuovo test SDK decoder o garanzia riguardo a versioni diverse |
| F13 | Rotazione boot trace a 512 KiB e timestamp, preservando 1 copia | Log SD non disponibile prima del mount; non eseguire da audio real-time |
| F14 | Documentazione e manifest aggiornati allo stato reale; protezione dalla build legacy | Ricostruire checkout Git unificato + cache per usare la build incrementale |
| F15 | CI host, regressioni C/Python ed esecuzione gate prima della build | CI non installa SDK proprietario e non verifica hardware / NRO |
| F16 | Validazione extent non nullo sia in swapchain sia in upload/present | Gestione dock/undock da riprodurre sulla console |
| F17 | Diagnostica suggerita e tracciamento dei limiti mDNS `9997` | Errore non riprodotto: impossibile affermare che sia risolto; raccogliere log e test LAN |

## Verifiche host

```sh
bash build-switch2.sh --test
```

Il test runner compila ed esegue i test nativi C su Linux con sanitizer e lancia i test Python.
Le verifiche statiche delle chiamate Vulkan non dimostrano la compilazione né l'assenza di deadlock sul device.

## Procedura Git

Da un checkout con modifiche salvate o working tree pulito:

```sh
git apply --check /percorso/punktfunk_switch2_hardening_20261118.patch
git apply /percorso/punktfunk_switch2_hardening_20261118.patch
bash build-switch2.sh --test
git add -A
git commit -m "Harden Switch 2 rendering, IO, audio, and host tests"
```

La build NRO vera e propria richiede l'ambiente ounce-devkit e il checkout unificato indicati in `README_SWITCH2_DEV.md`.
