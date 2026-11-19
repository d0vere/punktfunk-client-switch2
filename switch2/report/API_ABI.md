# Verifiche contrattuali API/ABI (fonti pubbliche)

Sorgenti consultati: `punktfunk/client-switch` commit `a332988f7509bd4c6fbde7252bd1e2b874019f45`, core pinned `0fc1b6c115519551031c51a20c9dc598fb1ea57d`, `ouncebrew/libounce` (`nx/include/switch2/video/decoder.h`, `nx/include/switch2/services/viddec.h`, `nx/include/switch/services/audout.h`, `nx/include/switch/runtime/pad.h`, `nx/include/switch/services/applet.h`, socket e HID), `ouncebrew/ounce-devkit/examples/vulkan-clear/source/main.c`.

**Limite fonte:** i riferimenti SDK provengono da repository pubblici, non dagli header *installati* `ounce-devkit 20261114` sul PC dell'utente. Un ABI diverso deve comportare un errore di compilazione, non una reinterpretazione silenziosa di struct o simboli.

## Strutture C e Rust

`native/pf2_native.h` definisce handle opachi e POD FFI. `rust/crates/pf-nx-switch2-platform/src/ffi.rs` riproduce con `#[repr(C)]` le stesse strutture. Il test host `tests/test_ffi_layout.c` verifica con `_Static_assert`:

- `Pf2DecoderSettings` = 48 byte, offset codec-data 24, dimensione 32, max sample 40;
- `Pf2DecodedFrame` = 40 byte, PTS a offset 16, pointer Y a 24, UV a 32;
- `Pf2PadRaw` = 40 byte, pulsanti a 16 e ultima asse a 36.

Il test vale per host a 64 bit; **non sostituisce** la compilazione C/Rust AArch64 e una verifica sullo SDK.

## Codec

L'API native `VideoDecoderConfig` espone `ViddecCodec_H264 = 0`, `ViddecCodec_Av1 = 3`, `videoDecoderCreate/Submit/GetFrame/CopyFrame/ReleaseFrame/Flush/Close`; AV1 usa layout block-linear. Il backend C converte i risultati in NV12 pitch-linear, poi in RGBA e aggiorna il frame della UI attraverso il `VideoSink` ufficiale. Il core pinned espone `quic::CODEC_AV1`; il client Switch1 non lo annunciava e ora il mapping settings/capability/handshake è esteso dal patcher `apply_integration.py`.

**AV1/H.264 su console NON testati**. L'adapter pubblicizza AV1 e H.264 per permettere la prova del porting, non perché l'hardware sia stato verificato. Serve confermare le capability effettivamente disponibili e aggiungere self-test più rigorosi prima di una release pubblica; HEVC rimane disattivato.

## Vulkan, audio, input, lifecycle

La surface `VK_NN_vi_surface` è stata verificata nell'esempio pubblico OunceVK. Il presenter usa gli oggetti Vulkan e trasferisce frame RGBA dallo staging host, ma non integra Skia/Ganesh (usa raster CPU). `audout` supporta audio PCM 48kHz stereo. `pad*`, `hidInitializeVibrationDevices`, `hidSendVibrationValues`, `socketInitializeDefault` e `appletMainLoop` sono stati confrontati ai relativi header pubblici. Nessuna chiamata a binari o all'ABI di NVK Switch1 viene inserita nei nuovi moduli C.

La fase di build Rust eredita lo shim TLS/newlib originale `pf_nx_sys::rt::link_guard` e cambia il linking da libnx Switch1 ai componenti libounce, con verifica ancora pendente. Il teardown Quinn non è stato certificato: socketExit viene rimandato all'uscita dell'app, ma questo da solo **non garantisce un drain del protocollo**.
