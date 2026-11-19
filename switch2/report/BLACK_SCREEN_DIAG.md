# Primo test su Switch 2 — diagnosi dello schermo nero

I log utente del 17 novembre 2026 attestano che la build Rust e l'ELF/NRO verificati sono riusciti e che il runtime originale parte.
Il log di console riferisce:
- 00:45:40 avvio applicazione, OunceVK nativo, UI Skia raster
- 00:45:40 mDNS `os error 9997` (problema di rete separato)
- 00:45:40 `console: built with 0 system faces in 4 ms`
- 00:46:31 `console: warmed the tabs in 50961 ms`
- 00:46:31 `system: Suspend`

Non e' possibile dedurre dal solo log che il driver Vulkan non presenti immagini.

## Modifiche di questa prova

- Prima del log Rust, `NativeDisplay::new` presenta un frame opaco blu/verde con un quadrato bianco. Se resta completamente nero *anche in questa fase*, investigare VI/swapchain/display: non attribuire il problema ai font.
- Caricamento differito di pl:u (stessa implementazione ufficiale Switch 1 di `pf-nx-sys::pl`); il logger riporta il numero di font e i byte.
- Disabilitazione del warm-up sincrono di tutte le schede solo per `device.platform="switch"` e `device.model="Nintendo Switch 2"`. Gli altri target mantengono il codice upstream originale.
- Primo, secondo, terzo frame reali del menu: statistiche dei pixel Skia e esito di `vkQueuePresent`.

## Come interpretare i log

- `switch2: diagnostic boot frame submitted` + schermo nero anche all'avvio: `vkQueuePresentKHR` ha restituito successo, ma non c'e' prova della visibilita' del layer. Verificare `nwindow` / VI / visibilita' / focus e la modalita' di avvio.
- Frame diagnostico visibile e `nonblack-rgb=0/...`: il frontend Skia non disegna pixel colorati, controllare font, canvas, o rendering console.
- `nonblack-rgb` positivo e `switch2 present rc=0` ma menu nero: esaminare upload memoria, formati, sincronizzazione e finestra Vulkan.
- Nessun `switch2 present: frame=1`: il frontend e' bloccato o sospeso prima della prima presentazione di menu; guardare gli eventi HOME e il caricamento font.
- Il 9997 mDNS NON dimostra una causa del nero; e' un secondo problema da verificare dopo che il menu e' visibile.

Questa e' una build diagnostica, non una release validata. Non sono stati eseguiti test grafici su console in questo ambiente.
