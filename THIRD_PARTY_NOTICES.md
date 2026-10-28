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

| Set | Move | Capture | Check | Checkmate | Defeat | Success |
| --- | --- | --- | --- | --- | --- | --- |
| wood | impactWood_light_002 | impactWood_heavy_002 | impactPlank_medium_002 | impactBell_heavy_001 | minimize_008 | confirmation_004 |
| glass | impactGlass_light_001 | impactGlass_heavy_001 | glass_002 | impactBell_heavy_000 | minimize_004 | confirmation_002 |
| clicks | click_002 | drop_002 | confirmation_001 | maximize_006 | minimize_006 | confirmation_003 |

`apps/desktop/assets/sounds/common/Wrong.ogg` (a wrong move while learning an opening, every
set) is Kenney's `error_003` of "Interface Sounds" (CC0), made the same way.
`apps/desktop/assets/sounds/common/LowTime.ogg` (the player's clock getting low, every set) is
two of Kenney's `tick_004` of "Interface Sounds" (CC0) 180 ms apart, 4 dB quieter, re-encoded
as Ogg Vorbis.

The Defeat and Success sounds of the Lichess sets (`Defeat.ogg` and `Victory.ogg` of
`public/sound/sfx`, `piano`, `futuristic`, `nes`, by Enigmahack, AGPLv3+) were downloaded
unmodified on 2026-10-25.

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

## Rodent V (an opponent with a style)

**Rodent V 1.2** (Naman Thanki and Paweł Koziol, based on Sungorus by Pablo Vázquez) is not
part of ChessPenguin. On the user's request the program downloads the authors' unmodified
release archive `release-1-2.zip` from https://github.com/nescitus/Rodent-V/releases/tag/Rodent_v_1.2
(SHA-256 pinned in `apps/desktop/src/rodent_download.rs`) into the user's data directory and
takes from it the engine for the platform and its `personalities/`, `nets/` and `books/`. It
runs as a separate program through UCI.

- License: GNU General Public License version 3 (https://github.com/nescitus/Rodent-V/blob/main/LICENSE);
  source: https://github.com/nescitus/Rodent-V.
- The opening books of the release come with it as the authors publish them.

## Rodent IV (characters and famous players)

**Rodent IV 0.33** (Paweł Koziol and Bernhard C. Maerz, based on Sungorus by Pablo Vázquez) is
not part of ChessPenguin. Its authors publish no builds for Linux and Windows, so the engine
is built from their sources at commit `e8d84c8c8c189a1cf4eb27c578fc573af3b916d2` of
https://github.com/nescitus/rodent-iv with one fix of ours, `packaging/rodent-iv/home-dir.patch`
(on Linux the engine now and then took a wrong home folder and played without its
personality), by `.github/workflows/rodent-iv.yml`, and published as the release
`rodent-iv-0.33` of this repository. On the user's request the program downloads
that build (SHA-256 pinned in `apps/desktop/src/rodent4_download.rs`) and the engine's
personalities and opening books from the authors' repository at the same commit (each file's
SHA-256 pinned in `crates/play/src/rodent4_files.rs`) into the user's data directory. It runs
as a separate program through UCI.

On Android the engine is built from the same commit with the same fix and two more,
`packaging/rodent-iv/think-thread.patch` (the search thread read its position from a dead
stack frame; clang builds searched garbage) and `packaging/rodent-iv/android.patch` (bionic has
no `wordexp`), by `androidApp/scripts/rodent4.sh`; the executable and the personalities are in
the APK (both builds), with the license, the authors' README and the credits of the books
(`licenses/rodent-iv` among the APK's assets). The opening books the characters use are
downloaded on the user's request from the authors' repository at the same commit, each file
checked against its SHA-256.

- License: GNU General Public License version 3 (https://github.com/nescitus/rodent-iv/blob/master/LICENSE);
  source: the commit above.
- The opening books come as the authors publish them, with their own credits (`#readme.txt`
  in each folder of `books/`).
- Three personalities (Strangler, Spassky, Deborah) name their book by a path not in the
  authors' files; the program puts a copy of the book under that name (on Android the book is
  saved under that name).

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

## Opening names (preparing against an opponent)

`crates/prep/data/a.tsv` … `e.tsv` are the five files of the Lichess opening table
https://github.com/lichess-org/chess-openings (the lichess.org contributors), downloaded
unmodified at revision `f06a50d5c7c140e50b068049fa31d289e2c3d347` (2026-10-27) on 2026-10-28 and
embedded in the program. Its README: "As a collection of facts, this data set is in the public
domain … released under the CC0 Public Domain Dedication" (CC0 1.0). SHA-256 of the files:

| File | SHA-256 |
| --- | --- |
| `a.tsv` | `3282e4c9155289a29224f9a85fba0decb46efa35c2fa5e39662d2ac48fb0f793` |
| `b.tsv` | `4bf2269a8e7571a00a7b346bae4fa86a241a6e7e86beab01783c81acb1622b63` |
| `c.tsv` | `45e843c7b7e79a0d04d5bda879c9bd23f5a10974488bd8615e4f4c5d0749986f` |
| `d.tsv` | `1cf4057a8aece94f27bb3f278ad410ee2844831a642364eeceaafa091dbcceda` |
| `e.tsv` | `48c6f6645b031a5937c8386839debab0e2e48837251687ba1afb6369677f8546` |

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
The moves of the notation and the move marks on the board are in Ubuntu Bold (Ubuntu Font
Family 0.83, © 2010–2011 Canonical Ltd., Ubuntu Font Licence 1.0), from
the official release `https://assets.ubuntu.com/v1/fad7939b-ubuntu-font-family-0.83.zip`
(SHA-256 456d7d42…05b42, as in Arch Linux's `ttf-ubuntu-font-family`):
`apps/desktop/assets/fonts/Ubuntu-B.ttf`, licence `UFL.txt` beside it.
