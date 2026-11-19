# Provenienza e identità del porting (sorgente)

| Percorso | Provenienza |
|---|---|
| `work/official-client` (creato sul PC durante build) | Clone di `https://github.com/punktfunk/client-switch`, HEAD esattamente `a332988f7509bd4c6fbde7252bd1e2b874019f45` |
| Crate ufficiali `pf-nx-app`, `pf-nx-platform`, `pf-nx-sys`, `pf-nx-vk` | Dal clone pinned, con adapter aggiunto e link Switch1 non utilizzato dalla nuova root target |
| `upstream-pinned/{punktfunk-core,pf-client-core,pf-console-ui}` | File degli allegati PX52 dell'utente, NON sostituiscono il checkout completo |
| `rust/crates/pf-nx-switch2-platform/` | Nuovo codice Rust d'integrazione alle interfacce ufficiali |
| `native/` | Nuovo codice C per le API native Switch 2, senza derivazioni di frontend/protocolli Azkali o Moonlight |
| `tooling/`, `tests/`, `*.sh` | Nuovi script di staging/build/rollback e test statici/host |
| `patches/{native-additions,rust-additions}.patch` | `git diff` stile new-file riproducibile, per revisione delle aggiunte |
| `work/official-integrated.diff` (generato) | `git diff --binary` delle modifiche al checkout ufficiale dopo la preparazione |

L'applicazione ufficiale resta l'unico punto di partenza. Nessun ELF/NRO precompilato da progetti precedenti viene incorporato. L'archivio non contiene il checkout completo ufficiale, né compilati target, SDK, cache Cargo o binari Nintendo. I checksum documentano i sorgenti *qui distribuiti*, non un file NRO inesistente.

Il manifest Cargo upstream conserva il pin del core `0fc1b6c115519551031c51a20c9dc598fb1ea57d`. Lo script modifica Cargo.toml/Cargo.lock in modo controllato: il lock è **modificato in copia** per aggiungere il nuovo crate, mentre il blob Git dell'originale viene verificato e può essere ripristinato con rollback. I vecchi crate restano nel workspace Git ma la root title Horizon dipende dal nuovo backend `pf-nx-switch2-platform` e non dal backend Switch1.
