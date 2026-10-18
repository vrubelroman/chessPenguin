# Changelog

All notable changes are listed here. Versions follow [Semantic Versioning](https://semver.org/).

## [0.4.0] — 2026-10-19

- Large ChessBase databases (Mega Database: 11.7 million games) open in seconds: read in place from
  the `.cbh` records, a game is converted when opened. A filter panel on the right (player with
  suggestions, colour, opponent, tournament, annotator, years, Elo, result, ECO, moves, comments and
  variations) updates the list as you type; sorting and filtering run in the background.
- Fixed: a ChessBase game that cannot be read from its first move (e.g. Chess960) crashed the import.
- Fixed: ChessBase databases mixing UTF-8 and single-byte texts could be decoded as Cyrillic.
- Removed the in-app ✕ (close) button: the window manager or desktop provides window controls.

## [0.3.0] — 2026-10-18

- Open ChessBase databases (`.cbh` with companion files) and ChessBase archives (`.cbv`), and
  `.zip` / `.7z` archives holding PGN, `.cbh` or `.cbv` files. Games, variations, comments (with the
  right code page, including Cyrillic), symbols, coloured squares and arrows, set-up positions and
  book chapters are converted to PGN in memory; sources are read-only. Problems are listed under
  "Import notes" in the library.
- Comments keep paragraph breaks (a blank line starts a new paragraph).
- Game screen: the result next to the players; Lichess-style badges on the moved piece (engine
  judgement or the author's symbol); on a marked move, arrows show the refutation and the best
  answer; all engine arrows blue, only the live engine's while it runs.
- Library: games opened before are marked with a check (context menu to change).
- More looks: piece sets governor, kosal, metaltops; boards IC, NCF, Ice, Navy, Mint, Emerald,
  Orange, Rose, Dusk, Night and Sketch; sound sets Futuristic, NES, Wood, Glass, Clicks.
- Fixed: the checkmate sound was silent (a copied link instead of a sound file).
- Library: "+ Add database"; long database names are shortened on the cards.

## [0.2.0] — 2026-10-18

- Stockfish 19 is bundled with every package (unmodified official "universal" build, which picks
  the right code path for the CPU) and set up on the first start; a system `stockfish` is detected
  too. Settings → Engines offers engines found on the computer. Packages grow to about 95 MB.

- Game analysis (Shift+A): background evaluation of the whole main line with its own engine process,
  evaluation graph under the board, report tab with accuracy, inaccuracies, mistakes, blunders,
  average centipawn loss and accuracy per game phase; engine marks and better lines in the notation;
  "Write analysis to the game" (NAGs, comments, `[%eval]`, one undo step). Results are cached;
  `[%eval]` comments of a PGN are used without an engine.
- Screenshots in English and Russian.

## [0.1.0] — 2026-10-18

First public release.

- PGN library with background loading, search and sorting; legacy encodings and real-world PGN quirks.
- Game view: board, notation with variations and comments, keyboard navigation.
- Move entry, variations, promote/delete lines, undo/redo, copy PGN/FEN, save as.
- UCI engine analysis: multiple lines, arrows, evaluation bar, inline engine settings, line preview.
- 14 piece sets, 26 boards, sounds, dark/light theme, six UI languages.
- Packages: deb, rpm, AppImage, Flatpak, Windows zip.
