# Sample databases

ChessPenguin puts these on the library shelf at the first start (they are built into
the program, see `apps/desktop/src/examples.rs`).

- `classics.pgn` — 100 famous games, in date order. The game records (players, event,
  date, result, moves) are historical facts. The first 38 carry comments and annotated
  variations written for ChessPenguin (covered by the project's licence,
  GPL-3.0-or-later); the other 62 (added 2026-10-28) are game records only, taken from
  the free player files of pgnmentor.com and, where possible, checked move by move
  against Wikipedia (only to compare; no text copied). Selection and sources:
  `docs/research/classics-sources.md`. A new version replaces the old one on the user's
  disk when the program is updated.
- `alekhine.pgn` — 267 games of Alexander Alekhine: game records only (tags and main-line
  moves). The commentary of the source collection was removed and is not distributed.
