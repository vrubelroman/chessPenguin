# Changelog

All notable changes are listed here. Versions follow [Semantic Versioning](https://semver.org/).

## [Unreleased]

- Stockfish 19 is bundled with every package (unmodified official "universal" build, which picks
  the right code path for the CPU) and set up on the first start; a system `stockfish` is detected
  too. Settings → Engines offers engines found on the computer. Packages grow to about 95 MB.

- Game analysis (Shift+A): background evaluation of the whole main line with its own engine process,
  evaluation graph under the board, report tab with accuracy, inaccuracies, mistakes, blunders,
  average centipawn loss and accuracy per game phase; engine marks and better lines in the notation;
  "Write analysis to the game" (NAGs, comments, `[%eval]`, one undo step). Results are cached;
  `[%eval]` comments of a PGN are used without an engine.

## [0.1.0] — 2026-10-18

First public release.

- PGN library with background loading, search and sorting; legacy encodings and real-world PGN quirks.
- Game view: board, notation with variations and comments, keyboard navigation.
- Move entry, variations, promote/delete lines, undo/redo, copy PGN/FEN, save as.
- UCI engine analysis: multiple lines, arrows, evaluation bar, inline engine settings, line preview.
- 14 piece sets, 26 boards, sounds, dark/light theme, six UI languages.
- Packages: deb, rpm, AppImage, Flatpak, Windows zip.
