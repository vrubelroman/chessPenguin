<p align="center">
  <img src="packaging/icons/hicolor/256x256/apps/io.github.vrubelroman.chessPenguin.png" width="128" alt="ChessPenguin icon">
</p>

<h1 align="center">ChessPenguin</h1>

<p align="center">
  A chess database for Linux, Windows, macOS and Android: open PGN and ChessBase collections,
  read annotated games, enter your own, analyse them with Stockfish, prepare against an
  opponent and play against the computer.
</p>

<p align="center">
  <a href="https://github.com/vrubelroman/chessPenguin/releases/latest">Download</a> ·
  <a href="README.ru.md">Русский</a>
</p>

![A game with its review](docs/screenshots/game.png)

## Features

- **PGN library** — keep several databases at hand, search and sort games, reopen where you left off.
- **ChessBase and archives** — open ChessBase databases (`.cbh`) and archives (`.cbv`), and `.zip` /
  `.7z` files with PGN or ChessBase inside: games, variations, comments, symbols, arrows and the book
  chapters, read-only (export to PGN by saving a game).
- **Real-world PGN** — nested variations, comments, NAGs, games from a FEN position, legacy
  encodings (windows-1251, windows-1252, KOI8-R, CP866) and teaching null moves (`--`).
  Source files are never modified.
- **Notation** — main line as a two-column table, comments and variations as blocks,
  mouse and keyboard navigation.
- **Stockfish included** — analysis works right after installation; other UCI engines can be added.
- **Reference** — the moves played from the current position in a database of your shelf
  (a big one for theory) with their statistics, how long the game stays in it, the
  strongest games that reached it.
- **Search by position** — set up moves on a board and filter a database of millions of
  games by the position, next to filters by player, years, rating, opening and result.
- **Engine analysis** — any UCI engine, up to 5 lines, best-move arrows, evaluation bar;
  search time, threads and memory are set right in the analysis panel.
- **Game review** — evaluate the whole game in the background: evaluation graph under the board,
  accuracy, inaccuracies, mistakes and blunders per player and per game phase (Lichess method),
  **brilliant (!!) and great (!) moves** calibrated on chess.com's marks of famous games, better
  moves in the notation and marks on the board; write it into the game as NAGs, comments and
  `[%eval]` (undoable). The review always runs the same engine search, so a game gets the same
  marks on every computer and phone.
- **Preparing against an opponent** — pick a player of a database and see their opening
  repertoire for each colour: a tree of their moves with opening names, games, score and the
  last game, a table of openings and variations with their first moves, where they leave
  theory; on request the engine finds their opening mistakes, by opening, with the ones they
  repeat. Every table sorts by its columns.
- **Engine lines on the board** — step through a line without touching the game, add it as a
  variation with one click (undoable).
- **Editing** — enter moves and variations, promote or delete lines, undo/redo,
  copy PGN/FEN, save a game to a new file.
- **Your comments** — write a comment after any move or at the start of the game, in the
  main line or inside a variation, right in the notation; put symbols on moves and
  positions; draw arrows and circles on the board with the right mouse button. Existing
  book comments can be edited; their embedded commands and bytes are kept when untouched.
- **Your own databases** — create an empty PGN database, add any game to it (a new one, a
  pasted one, a game from a ChessBase or online database), save your changes back
  (Ctrl+S), edit the game data (players, event, date, result, annotator), delete games with
  Undo. Any PGN file on the shelf takes games; writes happen only when you ask, by appending
  to the file or rewriting it atomically. Online and ChessBase databases stay read-only; a
  file in a legacy encoding gets a UTF-8 copy.
- **Paste a line** — moves or a whole PGN game from the clipboard go into the open game as a
  variation, from where they part from it, with their comments and variations (Ctrl+V).
- **Learn an opening** — replay from memory the moves of both sides of a game up to a chosen
  move; mistakes are counted, the runs of each line remembered. Or play on from any move
  against Maia.
- **Play against the computer** — Maia, a neural network trained on human games, plays like a
  human of the level you choose, mistakes included (600–2600 on her own scale); Rodent, a
  strong engine with a character: styles, characters and 22 famous players from Morphy to
  Kasparov (Rodent V and Rodent IV, 800–3000). Lichess
  time controls (3+0 to 30+20, your own, unlimited), clocks, premoves, take-backs, draw
  offers; the opponent offers to resign when lost. Played games go by themselves into the
  player's database. Maia (316 MB) and Rodent (48 and 70 MB) are downloaded on request.
- **Looks** — 17 piece sets, 37 boards (flat colours and textures), 7 sound sets,
  dark and light themes.
- **Online games** — download anyone's games from Lichess and Chess.com by nickname (yours, a pupil's, an opponent's; no login) into a database, both sites merged or one per site; new games arrive by themselves.
- **100 classic games** built in — from Anderssen and Morphy to Carlsen, the famous ones
  with short introductions; the sample databases update with the program.
- **Android app** — databases and games in the same formats, moves on the board, your own
  databases, game review with !! and !, play against Maia, learning an opening.
- **Languages** — English, Русский, Français, Deutsch, Español, Italiano, Português, Oʻzbekcha, Қазақша.

| Preparing against an opponent | Game review: report, !! and ! |
| --- | --- |
| ![Preparing against an opponent](docs/screenshots/prep-openings.jpg) | ![Game review](docs/screenshots/review.png) |
| **Engine analysis** | **Reference: what was played here** |
| ![Engine analysis](docs/screenshots/engine.jpg) | ![Reference](docs/screenshots/reference.jpg) |
| **Library with filters** | **Search by position** |
| ![Library](docs/screenshots/library.png) | ![Search by position](docs/screenshots/position-search.jpg) |
| **Play against the computer** | **Android** |
| ![Play](docs/screenshots/play.jpg) | ![Android](docs/screenshots/android.jpg) |
| **Boards, pieces and a light theme** | |
| ![Looks](docs/screenshots/looks.jpg) | |

## Install

Packages for every release are on the [Releases](https://github.com/vrubelroman/chessPenguin/releases) page.

| Format | Command |
| --- | --- |
| Debian / Ubuntu | `sudo apt install ./chesspenguin_<version>_amd64.deb` |
| Fedora / openSUSE | `sudo dnf install ./chesspenguin-<version>-1.x86_64.rpm` |
| Arch Linux | `sudo pacman -U ./chesspenguin-<version>-1-x86_64.pkg.tar.zst` |
| Any distribution | `chmod +x ChessPenguin-<version>-x86_64.AppImage && ./ChessPenguin-<version>-x86_64.AppImage` |
| Flatpak | `flatpak install --user ChessPenguin-<version>-x86_64.flatpak` |
| Windows | unzip `ChessPenguin-<version>-windows-x86_64.zip`, run `ChessPenguin.exe` |
| macOS (Apple Silicon) | open `ChessPenguin-<version>-macos-arm64.dmg`, drag ChessPenguin to Applications |
| Android 12+ | open `ChessPenguin-<version>-android.apk` (Stockfish built in) or `…-android-noengine.apk` (Stockfish downloaded in the app) on the phone |

On macOS the app is not signed by Apple yet: the first time, right-click ChessPenguin in
Applications and choose **Open** (on macOS 15: System Settings → Privacy & Security → Open Anyway).

**Stockfish** is one click away: the first time you analyse, ChessPenguin offers to download the
official Stockfish 19 (about 80 MB, checked before use, installed for your account only); the
button is also in **Settings → Engines**. A `stockfish` installed on the system is found
automatically, any other UCI engine can be added from a file. The Flatpak includes Stockfish.

The **Android app** opens databases and games (the same formats), with the notation under the
board and Stockfish. Moves are made on the board (variations, undo); your own databases live on
the phone; game review with !! and !; play against Maia and Rodent IV; learn an opening up to a
move. The two APKs install over each other; databases and settings are kept.

### Build from source

Rust 1.97 or newer. On Debian/Ubuntu: `sudo apt install pkg-config libasound2-dev libxkbcommon-dev libwayland-dev`.

```sh
cargo run --release -p chesspenguin -- examples/classics.pgn
./scripts/install.sh              # install for the current user (~/.local) with Stockfish; --no-stockfish, --uninstall
```

## Keyboard

| Where | Keys |
| --- | --- |
| Library | ↑/↓ select · Enter open · Ctrl+O add database · Ctrl+Shift+N new database · Ctrl+N new game · Ctrl+F search · F2 game data · Ctrl+Shift+S add the game to another database · Delete delete the game (Ctrl+Z puts it back) |
| Game | ←/→ move · ↑ end · ↓ start (also End/Home) · Shift+↑/↓ switch variation · F flip · L engine · Shift+A game analysis · Esc leave a preview, then back to the previous game or the library (unsaved edits: Esc again leaves without saving) |
| Editing | Ctrl+Z / Ctrl+Y undo/redo · Ctrl+Shift+C copy PGN · Ctrl+S save to the game's database (or add to one) · Ctrl+Shift+S add to a database · F2 game data · Ctrl+W back to library |
| Comments | Enter comment on the move (Enter done · Esc cancel · Shift+Enter paragraph) · Ctrl+1…6 the move's symbol ! ? !! ?? !? ?! · right mouse button on the board: an arrow (drag) or a circle (click); Shift red, Alt blue, Shift+Alt yellow |

## Project layout

- `crates/chess-core` — game tree, tolerant PGN import/export.
- `crates/uci` — UCI engine process supervisor.
- `apps/desktop` — the egui application.
- `crates/mobile-api`, `androidApp` — the core for the Android app and the app itself.
- `packaging/` — desktop entry, AppStream metadata, icons, deb/rpm, AppImage and Flatpak recipes.

The Android app is `androidApp/` (Kotlin, Jetpack Compose) over the same Rust core
(`crates/mobile-api`, UniFFI); `androidApp/scripts/release.sh` builds the two signed APKs.

Releases are built by GitHub Actions when a `v*` tag is pushed (see `.github/workflows/release.yml`).

## License

GPL-3.0-or-later. Third-party piece sets, boards and sounds come from
[Lichess](https://github.com/lichess-org/lila) under their own free licenses —
see [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).
