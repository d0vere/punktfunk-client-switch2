# Punktfunk Switch 2 — stato del porting e diagnosi (18 novembre 2026)

> Questo repository contiene un porting **sperimentale** del client ufficiale,
> non una release Switch 2 validata. Una precedente NRO si avviava ma mostrava
> schermo nero. Questa patch non dimostra da sola che il difetto sia risolto.

## Checkout e build attuali

Il progetto usa la radice Git unificata documentata in `README_SWITCH2_DEV.md`.
Non usare `resume_skia.sh`, `stage.py`, `apply_integration.py` o gli script di
migrazione legacy su un checkout già unificato: il loro scopo è diverso.

```sh
# Eseguibile anche senza SDK, .git e cache di compilazione:
bash build-switch2.sh --test

# Richiede la toolchain ounce-devkit e le cache del checkout unificato:
bash build-switch2.sh
```

Il gate host verifica Python e quattro componenti C: **non** compila il
frontend Rust per Horizon, non produce una NRO e non esegue API Vulkan/Audio
su Switch 2. L'esito corrente è visualizzato dalla sua uscita, non dal
`switch2/manifest.json` (metadati storici e pin). Non ripristinare la directory
`.pf2-cache` dall'archivio sorgenti: va fornita dal checkout/toolchain locale.

## Modalità diagnostica schermo nero

```sh
# Abilita una sola presentazione raster blu/verde con quadrato bianco,
# prima del disegno UI, per 300 ms. NON usata in build normale.
PF2_SCANOUT_PROBE=1 bash build-switch2.sh
```

Se il test pattern non compare, controllare finestra VI, swapchain e risultati
`vkQueuePresentKHR`; la mancata comparsa non identifica da sola la causa.
Se il test appare ma la UI no, raccogliere le righe `switch2 ganesh`,
`switch2 skia ui`, `switch2 present` e `switch2 fonts` dai log.
La pipeline principale usa Ganesh/Vulkan e fallback raster. Gli screenshot
`read_back()` provano a leggere la superficie Ganesh reale; se il driver
non consente il readback, il test-run ora registra un errore esplicito anziché
ritornare uno screenshot CPU non valido.

I log iniziali vengono appesi a `sdmc:/switch/punktfunk-boot.log`, con
rotazione oltre 512 KiB verso `punktfunk-boot.log.1` e timestamp Unix.
Conservare entrambi i file e il log applicativo in
`sdmc:/switch/punktfunk/logs/` durante una prova.

## Main10 e HDR

La release predefinita pubblicizza **solo input SDR/8-bit**. L'AV1 Main10
(P010) è sperimentale e si abilita esplicitamente con:

```sh
PF2_ENABLE_MAIN10=1 bash build-switch2.sh
```

Questo permette di accettare un flusso Main10/PQ e convertirlo in SDR:
**non abilita HDR10 nativo in uscita TV**. Per dichiarare supporto Main10
occorre verificare su console sequenze PQ/BT.2020 e range limitato/pieno.

## Recovery / prestazioni e rete

- Un submit Vulkan fallito invalida la presentazione: prima del nuovo frame
  si ricostruiscono semafori **e fence**. Un vero `VK_ERROR_DEVICE_LOST`
  resta non recuperabile senza creare un nuovo device; il client lo registra.
- Restano sincronizzazioni CPU/GPU bloccanti per preservare l'ownership della
  singola coda. `PF2_ASYNC_GANESH_SUBMIT=1 bash build-switch2.sh` è un'opzione
  **sperimentale** per togliere il wait CPU di Ganesh; il wait del presenter
  resta presente. Non promettere 120 fps stabili senza profiling su console.
- Il fallback raster evita il buffer video addizionale se lo stream non ha
  overlay oppure oltre 1080p; alle risoluzioni alte ricompone i pixel a ogni
  frame con overlay, riducendo RAM ma potenzialmente peggiorando il framerate.
- La funzione `pread` usata da Skia/newlib serializza le chiamate attraverso
  questo shim; non è atomica rispetto a `read/lseek` effettuati altrove sullo
  stesso file descriptor. Non condividere tali fd con altri consumatori.
- L'errore precedente `mDNS os error 9997` non è stato riprodotto né risolto
  in modo dimostrabile. Usare i log già disponibili per confrontare
  bind/FIONBIO Rust e C, test IP manuale e discovery multicast sulla stessa LAN.

## Checklist minima sulla console

1. Avvio a freddo con e senza `PF2_SCANOUT_PROBE`; verificare test pattern,
   menu, font, touchscreen e primo screenshot (vedere eventuale errore readback).
2. Riavvio senza cancellare profili, pairing o impostazioni.
3. HOME, sospensione/ripresa, dock/undock e cambio risoluzione 720p/1080p/4K;
   controllare che il presenter si riprenda senza attese indefinite.
4. Stream H.264 e AV1 8-bit, frame-time 60/120 Hz, latenza, drop e rumble 0..3.
5. Verifica rete via IP e poi via mDNS; raccogliere errno e BSD result.
6. Solo se attivato esplicitamente, AV1 Main10 con tone mapping SDR.

I gate host e CI sono **regressioni statiche**. Solo una build AArch64 con SDK
corrispondente e queste prove in presenza dell'hardware possono qualificare
una release Switch 2.
