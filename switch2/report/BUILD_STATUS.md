# BUILD STATUS — 2026-11-17

Prova reale precedente: il log utente contiene `Finished release`, i checksum strutturali di ELF e NRO e il nome `punktfunk-switch2.nro`. Il firmware avvia il titolo, ma non visualizza il menu.

Questo pacchetto e' **sorgente diagnostico nuovo**, non la NRO gia' provata: i suoi cambiamenti devono essere cross-compilati sul PC dell'utente. L'ultima prova reale mostra 0 font fallback, 50.961 s nel warm-up e `Suspend` subito dopo; non dimostra la causa grafica definitiva. L'errore mDNS 9997 rimane non risolto.

Test qui: 33 Python, 4 C host, controllo delle patch e archivio. Toolchain Rust/SDK e console Switch 2 non disponibili in questo ambiente.
