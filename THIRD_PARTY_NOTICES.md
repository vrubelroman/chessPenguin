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

Notes:
- Lichess "standard" sounds are listed as non-free in `COPYING.md` and are intentionally not used.
- Piece sets and boards under CC BY-NC-SA (e.g. california, staunty, maestro, horsey) are not bundled: non-commercial terms are incompatible with the GPL.
- AGPLv3+ sound, piece and board files are combined with this GPLv3+ program under GPLv3 §13.

## Rust crates

Dependency licenses are recorded by `Cargo.lock`; key ones: shakmaty / pgn-reader (GPL-3.0-or-later),
egui / eframe (MIT OR Apache-2.0).
