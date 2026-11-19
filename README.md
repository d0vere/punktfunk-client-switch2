<div align="center">

<img src="packaging/icon_large.png" alt="punktfunk" width="130">

# punktfunk for Nintendo Switch 2

**Play your PC games on your Switch 2, streamed over your home network.**

[![Latest release](https://img.shields.io/github/v/release/punktfunk/client-switch?label=download)](https://github.com/punktfunk/client-switch/releases/latest)
[![CI](https://github.com/punktfunk/client-switch/actions/workflows/ci.yml/badge.svg)](https://github.com/punktfunk/client-switch/actions/workflows/ci.yml)

</div>

---

punktfunk runs your games on your PC and sends the picture and sound to your Switch 2. It is a native homebrew app: it opens from the Homebrew
Menu (no applet mode), uses the Switch's own video decoder, and works handheld or docked. Handheld has been more tested and it's the recommended mode to play.

## What you need

- A **Nintendo Switch 2**  running homebrew on *kaiHEN**.
- A PC with the **[punktfunk host](https://docs.punktfunk.unom.io/docs/install)** installed, on
  the same network.
- A good connection: 5 GHz Wi-Fi near the router, or a wired dock. A wired PC helps too.

## Install

1. Download **`punktfunk-switch2.nro`** from the [latest release](https://github.com/d0vere/punktfunk-client-switch2/releases).
2. Copy it to **`/switch/punktfunk/`** on your SD card.
3. **Hold R while opening any switch2 game**, then pick Punktfunk in the Homebrew Menu 

Then pick your PC on the **Hosts** screen, enter the PIN the host shows, and choose a game. 
You can check more in punktfunk [quickstart](https://docs.punktfunk.unom.io/docs/quickstart) documentation.

## What it does

- **1080p at 120 fps handheld, 4k at 60 fps docked.** Docking mid-game switches without
  reconnecting is still being working. 
- **Your controllers, as they are.** Joy-Cons and the Pro Controller, with HD rumble, gyro and
  the touch screen.
- **Low latency.** About 15–20 ms from the PC's screen to yours, on a good network.
- **Two presentation modes.** *Smoothness* gives a very smooth picture at an acceptable
  latency. *Low latency* cuts the delay further, at the cost of some smoothness.
- **Easy to set up, easy to use.** You pair once, and from then on it is one
  click to connect. The menus are built for a controller and a touch screen, so you never
  need a keyboard.
- **A modern, encrypted protocol.** The stream runs on punktfunk's own protocol, built on
  QUIC and encrypted end to end. It is designed for low latency and recovers from lost
  packets without freezing the picture.

| In a stream | Does |
| --- | --- |
| **Minus + A** | Quick actions: statistics, controller mouse, controller type |
| Hold **L + R + Plus + Minus** for one second | End the stream |

## Known limits

- Docked play has been tested less than handheld.

## Found a problem?

Open an [issue](https://github.com/d0vere/punktfunk-client-switch2/issues) and attach the log from
`/switch/punktfunk/logs/` on your SD card.

---


### Use of AI

This project uses AI for writing code. The rules the agents follow are in [AGENTS.md](AGENTS.md).

## Licence

The source in this repository is MIT OR Apache-2.0, at your option ([`LICENSE-MIT`](LICENSE-MIT),
[`LICENSE-APACHE`](LICENSE-APACHE)).

The built `punktfunk.nro` is GPL-3.0-or-later ([`packaging/licenses/GPL-3.0.txt`](packaging/licenses/GPL-3.0.txt)).
It links devkitPro's `switch-ffmpeg`, whose `nvtegra` hwaccel (the NVDEC driver) is
GPL-2.0-or-later. The Apache-2.0 code combines with GPL version 3 only. The NRO's corresponding
source is this repository at the release's tag, plus FFmpeg 7.1 with devkitPro's patches at
[the pinned recipe](https://github.com/devkitPro/pacman-packages/tree/5cf3a4da7665a04032565a3e788656cb485dd279/switch/ffmpeg).
`THIRD-PARTY-NOTICES.txt` is generated at build time from the dependency graph and ships beside the NRO.
