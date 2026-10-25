# Third-party notices

The application is licensed under GPL-3.0-or-later (see `LICENSE`).

## Bundled assets

All assets below were downloaded unmodified from
https://github.com/lichess-org/lila at revision `cd2656d2787a766ba743da8da1dd3b24975e6329`
(2026-10-17). Authorship and license as stated in that revision's `COPYING.md`.

| Path in this repo | Source path | Author | License |
| --- | --- | --- | --- |
| `apps/desktop/assets/pieces/cburnett` | `public/piece/cburnett` | Colin M.L. Burnett | GPLv2+ |
| `apps/desktop/assets/pieces/merida` | `public/piece/merida` | Armando Hernandez Marroquin | GPLv2+ |
| `apps/desktop/assets/pieces/mpchess` | `public/piece/mpchess` | Maxime Chupin | GPLv3+ |
| `apps/desktop/assets/pieces/chessnut` | `public/piece/chessnut` | Alexis Luengas | Apache-2.0 |
| `apps/desktop/assets/pieces/fantasy` | `public/piece/fantasy` | Maurizio Monge | MIT |
| `apps/desktop/assets/pieces/celtic` | `public/piece/celtic` | Maurizio Monge | MIT |
| `apps/desktop/assets/pieces/spatial` | `public/piece/spatial` | Maurizio Monge | MIT |
| `apps/desktop/assets/pieces/rhosgfx` | `public/piece/rhosgfx` | RhosGFX | CC0-1.0 |
| `apps/desktop/assets/pieces/shapes` | `public/piece/shapes` | flugsio | CC BY-SA 4.0 |
| `apps/desktop/assets/pieces/kiwen-suwi` | `public/piece/kiwen-suwi` | neverRare | CC BY 4.0 |
| `apps/desktop/assets/pieces/firi` | `public/piece/firi` | James Faure | CC BY 4.0 |
| `apps/desktop/assets/pieces/pixel` | `public/piece/pixel` | therealqtpi | AGPLv3+ |
| `apps/desktop/assets/pieces/letter` | `public/piece/letter` | usolando | AGPLv3+ |
| `apps/desktop/assets/pieces/pirouetti` | `public/piece/pirouetti` | pirouetti | AGPLv3+ |
| `apps/desktop/assets/boards/*` | `public/images/board/*` (wood, wood2–4, maple, maple2, leather, marble, blue-marble, blue2, blue3, canvas2, metal, olive, grey, green-plastic, purple-diag, pink-pyramid) | the lila authors and pirouetti | AGPLv3+ |
| `apps/desktop/assets/sounds/sfx` | `public/sound/sfx` | Enigmahack | AGPLv3+ |
| `apps/desktop/assets/sounds/piano` | `public/sound/piano` | Enigmahack | AGPLv3+ |
| `apps/desktop/assets/sounds/futuristic` | `public/sound/futuristic` | Enigmahack | AGPLv3+ |
| `apps/desktop/assets/sounds/nes` | `public/sound/nes` | Enigmahack | AGPLv3+ |
| `apps/desktop/assets/boards/horsey.jpg` | `public/images/board/horsey.jpg` | the lila authors | AGPLv3+ |

The flat "IC" and "NCF" boards reproduce the two colours of `public/images/board/ic.png` and
`ncf-board.png`; the other flat boards are ChessPenguin's own colour pairs.

### From PyChess

Downloaded unmodified (files renamed `wk.svg` → `wK.svg`) from https://github.com/pychess/pychess
(`pieces/`, 2026-10-18), with the license file of each set:

| Path in this repo | Author / origin | License |
| --- | --- | --- |
| `apps/desktop/assets/pieces/governor` | the lila authors (Lichess "governor") | AGPLv3+ |
| `apps/desktop/assets/pieces/kosal` | Kosal Sen (philatype.com) | CC BY 4.0 |
| `apps/desktop/assets/pieces/metaltops` | PyChess (`license.txt`: "published under the GPL") | GPL |

### Kenney sounds

`apps/desktop/assets/sounds/wood`, `glass` and `clicks` are made from Kenney's "Impact Sounds" and
"Interface Sounds" (https://kenney.nl, CC0 1.0): leading silence trimmed, loudness matched to the
other sets, re-encoded as Ogg Vorbis.

| Set | Move | Capture | Check | Checkmate | Defeat |
| --- | --- | --- | --- | --- | --- |
| wood | impactWood_light_002 | impactWood_heavy_002 | impactPlank_medium_002 | impactBell_heavy_001 | minimize_008 |
| glass | impactGlass_light_001 | impactGlass_heavy_001 | glass_002 | impactBell_heavy_000 | minimize_004 |
| clicks | click_002 | drop_002 | confirmation_001 | maximize_006 | minimize_006 |

The Defeat sounds of the Lichess sets (`Defeat.ogg` of `public/sound/sfx`, `piano`,
`futuristic`, `nes`, by Enigmahack, AGPLv3+) were downloaded unmodified on 2026-10-25.

Notes:
- Lichess keeps a set's checkmate sound as a link to its check sound; ChessPenguin plays the check
  sound for checkmate in the Lichess sets.
- Lichess "standard" sounds are listed as non-free in `COPYING.md` and are intentionally not used.
- Piece sets and boards under CC BY-NC-SA (e.g. california, staunty, maestro, horsey) are not bundled: non-commercial terms are incompatible with the GPL.
- AGPLv3+ sound, piece and board files are combined with this GPLv3+ program under GPLv3 §13.

## Maia-3 (play against the computer)

`crates/maia` computes the Maia-3 network (CSSLab, University of Toronto: Reid McIlroy-Young,
Ashton Anderson and co-authors) in Rust. Its architecture and input encoding follow the
reference implementation https://github.com/CSSLab/maia3 (AGPL-3.0), translated to Rust and
checked against it (`scripts/maia-reference.py`, `crates/maia/tests/reference.json`); as with the
AGPL sounds above, it is combined with this GPLv3+ program under GPLv3 §13.

The network weights are **not** part of ChessPenguin. On the user's request the program
downloads the authors' own file `maia3-5m.pt` from https://huggingface.co/UofTCSSLab/Maia3-5M
(revision and SHA-256 pinned in `apps/desktop/src/maia_download.rs`) into the user's data
directory. The weights' licence is not stated by the authors.

## Stockfish (engine)

**Stockfish 19** (tag `sf_19`) is the unmodified official "universal" build from
https://github.com/official-stockfish/Stockfish/releases/tag/sf_19. The Flatpak bundles it;
the other packages do not: on request the program downloads the same official archive for the
platform into the user's data directory and checks its SHA-256 (pinned in
`apps/desktop/src/engine_download.rs` and `packaging/stockfish.sh`) before using it.

- License: GNU General Public License version 3 (`Copying.txt`, installed as
  `stockfish-Copying.txt` next to this application's license); authors: the Stockfish
  developers (`AUTHORS`, installed alongside).
- Source code: https://github.com/official-stockfish/Stockfish/tree/sf_19 ; every GitHub release of
  ChessPenguin also carries `stockfish-sf_19-source.tar.gz`. The neural networks embedded in the
  binary are published by the Stockfish project (see its `README.md` and `src/evaluate.h`).
- Stockfish's networks are trained on data provided by the Leela Chess Zero project under the
  Open Database License (ODbL).
- ChessPenguin runs Stockfish as a separate process over the UCI protocol.

## ChessBase formats

- `crates/chess-import/src/cbv.rs` reads ChessBase archives following uncbv
  (https://github.com/antoyo/uncbv, Antoni Boucher, GPL-3.0-or-later); the code is a rewrite.
- The ChessBase database layout (`crates/chess-import/src/chessbase/`) follows the public format
  description in https://github.com/Yarin78/morphy (`format/v1`), used as a source of facts; no code
  is taken from that project.
- ChessBase is a trademark of ChessBase GmbH; ChessPenguin is not affiliated with it.

## Rust crates

Dependency licenses are recorded by `Cargo.lock`; key ones: shakmaty / pgn-reader (GPL-3.0-or-later),
egui / eframe (MIT OR Apache-2.0), zip (MIT), sevenz-rust2 (Apache-2.0), sysinfo (MIT),
sys-locale (MIT OR Apache-2.0), whatlang (MIT, detecting the language of comments).

egui's bundled fonts are part of the program: Ubuntu Light (Ubuntu Font Licence 1.0), Hack
(MIT and Bitstream Vera licence), Noto Emoji (SIL OFL 1.1), emoji-icon-font (MIT).
Three evaluation signs (⩲ ⩱ ⌓) come from Noto Sans Math (Copyright 2022 Google LLC, SIL OFL 1.1),
cut down to those glyphs: `apps/desktop/assets/fonts/NotoSansMath-nag.ttf`, licence beside it.
The figurines of the move notation (♔♕♖♗♘) are the five chess glyphs of egui's
emoji-icon-font (MIT), cut out to `apps/desktop/assets/fonts/figurines.ttf`.
