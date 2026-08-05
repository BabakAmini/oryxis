# Downloaded fonts: provenance and licenses

Fonts in this directory ship inside the binary (see `OFL.txt` for the
bundled Noto and SauceCodePro / Symbols Nerd Font notices). The fonts
below are NOT bundled: the app downloads them on demand, verifies them
against SHA-256 pins baked into `crates/oryxis-app/src/fonts.rs`, and
caches them under `~/.oryxis/fonts/`. Each downloaded TTF keeps its
own embedded license metadata; this file records provenance and
license terms for the pinned set.

## CJK language fonts

Noto Sans KR / SC / JP / TC, fetched from `google/fonts` at commit
`c89741abbf4eeabce432c3ed2fd7dc28b022701e`.
Copyright The Noto Project Authors. Licensed under the SIL Open Font
License, Version 1.1 (full text in `OFL.txt`).

## Terminal font pack (issue #109)

Nerd Fonts patched builds, fetched from `ryanoasis/nerd-fonts` at
commit `fa7b859994228a9c8759f99c55a8d31ee92a1b5e` (the v3.4.0 tag).
The Nerd Fonts patcher additions are MIT licensed, Copyright (c) 2014
Ryan L McIntyre (https://github.com/ryanoasis/nerd-fonts). Each
patched family keeps its upstream license:

| Family (picker name) | Upstream | License |
|---|---|---|
| JetBrainsMono NF | JetBrains Mono, Copyright 2020 The JetBrains Mono Project Authors | OFL 1.1 |
| CaskaydiaCove NF | Cascadia Code, Copyright (c) 2019 Microsoft Corporation | OFL 1.1 |
| FiraCode Nerd Font | Fira Code, Copyright 2014-2020 The Fira Code Project Authors | OFL 1.1 |
| Hack Nerd Font | Hack, Copyright 2018 Source Foundry Authors (Hack Open Font License / Bitstream Vera license) | MIT-style + Bitstream Vera |
| MesloLGS Nerd Font | Meslo LG, Copyright 2009-2013 Andre Berg (based on Menlo/Bitstream Vera/DejaVu) | Apache 2.0 |
| RobotoMono Nerd Font | Roboto Mono, Copyright 2015 The Roboto Mono Project Authors | Apache 2.0 |
| UbuntuMono Nerd Font | Ubuntu Mono, Copyright 2010-2023 Canonical Ltd | Ubuntu Font Licence 1.0 |
| Iosevka NF | Iosevka, Copyright 2015-2024 Renzhi Li (aka. Belleve Invis) | OFL 1.1 |

The OFL 1.1 full text is in `OFL.txt`. The other license texts travel
embedded in the downloaded TTFs' name tables and in the upstream
repositories linked above.
