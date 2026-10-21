# Changelog

All notable changes are listed here. Versions follow [Semantic Versioning](https://semver.org/).

## [0.8.4] — 2026-10-21

- Move notation (Settings › Board & pieces): Latin letters (Nf3), figurines as on Lichess
  (♘f3), or the piece letters of the interface language — Russian Кр Ф Л С К, German
  K D T L S, French R D T F C, Spanish and Italian R D T A C. It applies to the move list,
  engine lines, the reference tab, board search and the analysis report; PGN files and
  copied games always stay in K Q R B N. Latin remains the default.
- Translation of game comments: when a game's comments are in another language, ChessPenguin
  offers to translate them into yours. Sign in with your ChatGPT Plus or Pro subscription (no
  key), or use an API key — DeepSeek, OpenAI, Google Gemini — or a model on this computer
  (Ollama, LM Studio). After the first consent, games in other languages are translated as you
  open them; [Original | Translation] (Ctrl+Shift+T) switches back. Moves are never translated:
  a translation that changes a move is not shown. The translation is never written to the PGN;
  a local cache keeps it, so a game opens translated again without the network. Settings ›
  Translation: the target language, when to translate, the connection, permissions, the cache.
- Arrows (→, ⇄) and the evaluation signs ⩲ ⩱ ⌓, shown as empty boxes before, now render.
- Piece animation, as on Lichess: a move slides on the board (castling moves both king and
  rook; jumping several moves slides every piece once), a captured piece fades out. Speed in
  Settings → Board: Off, Fast (120 ms), Normal (250 ms, the default), Slow (500 ms). A piece
  dropped by dragging is already in place and does not slide.

## [0.8.2] — 2026-10-21

- Windows: ChessPenguin no longer quits silently on a machine whose graphics do not
  cooperate (an old driver, a broken Vulkan, a virtual machine). A start that fails or dies
  before its first frame tries again without Vulkan, then with OpenGL, then with software
  drawing, and keeps the mode that worked. What still prevents the start is shown in a
  message and written to chesspenguin.log beside the settings. The C runtime is now inside
  the executable, so the Visual C++ Redistributable is not needed.
- The window "Games from Lichess and Chess.com" opens with an empty nickname, the cursor in
  it: the next search is usually for another player (a database already downloaded fetches
  its new games with "⟳ New games").

## [0.8.1] — 2026-10-20

- The side panel fits small screens (a laptop with 200 % scaling): the reference tab no longer
  slides under the board and is no longer cut off on the left. Its tables narrow their columns
  and drop the less important ones to fit, their headings wrap, and in a tall narrow window the
  board gives way so the reference keeps enough width.

## [0.8.0] — 2026-10-20

- Games from Lichess and Chess.com: anyone's games by nickname — yours, a pupil's, an
  opponent's, a champion's (no login; another nickname for Chess.com if it differs). See what
  was found, download the games into a database — both sites merged into one by default, or
  one per site — and later fetch the new ones with "⟳ New games" in the database header. The
  progress shows per site with the time left; the window may be closed meanwhile (the result
  then shows as a message). An interrupted download — stopped, failed, or the program closed —
  keeps what it got and continues where it stopped, without duplicates (also after a crash);
  games still being played (correspondence) are fetched once they end. Only standard chess is
  kept. The feature is offered only when the sites answer: ChessPenguin asks each for a public
  profile at start (nothing of yours is sent), so offline it stays hidden.

## [0.7.1] — 2026-10-20

- Updates: at start ChessPenguin asks GitHub for the latest version (can be turned off in
  Settings → About) and offers a newer one with a button next to Settings. The Windows zip,
  the AppImage and the Linux archive update themselves (download, checksum check, replace,
  restart); macOS downloads and opens the disk image with step-by-step instructions; deb, rpm,
  Arch and Flatpak packages are downloaded into Downloads and the one command that installs
  them is shown with a Copy button; the AUR package shows the helper command. "Skip this
  version" hides the button until the next one.
- The Flatpak may use the network (the update check).

## [0.7.0] — 2026-10-20

- «Справочник» (Reference) tab of a game: what was played in the current position in a
  database of the shelf (Mega for theory, an opening collection, a pupil's games), as two
  tables — the moves with games, share, results, White's score, average rating, year and
  the best players who played them, and the strongest (or newest) games that reached the
  position with how they went on. Click a move to see its games, double-click to play it;
  double-click a game to open it at this position (the current game stays a click away:
  "⬅ Back to …", unsaved edits included). It also tells how far the game follows the
  database (the novelty: the first move no other game of the database played) and finds the
  game itself there (to open, for example, an annotated copy).
- The board of a game always keeps the full height of the window (the side panel is never
  wider than the room beside it).
- Settings → Engines no longer offers to download Stockfish when one is already set up.
- Russian numbers use a decimal comma ("11,7 млн").

## [0.6.3] — 2026-10-19

- Import notes (for example, Chess960 games shown without moves) are a quiet grey "ℹ" now
  instead of an orange warning sign.
- Position search in a large ChessBase database without a usable `.cbb` search index: the
  program builds its own once (in the background, kept in the cache) and deep positions are
  then found in milliseconds instead of a pass over every game.
- Opening positions (the start, 1.e4, …) in a large ChessBase database: an index of the
  first ten moves of every game, built with the booster and kept in the cache, answers them
  four to eight times faster. It is held in memory only when the machine has room for it
  (about 600 MB for Mega Database); otherwise the search works as before.
- A big PGN file whose position table would not fit in memory now says so on the search
  board instead of running out of memory.
- `chesspenguin --bench <database>` prints timings of opening, filtering and position
  search for comparing machines.
- Settings → About: clicking the email address copies it instead of opening a mail app,
  which does nothing useful when none is installed.

## [0.6.2] — 2026-10-19

- Settings → About: the version, links to the website (in the interface language), the release
  notes and the source code, and the support contacts. Links open in the browser.

## [0.6.1] — 2026-10-19

- The library is no longer empty at the first start: two sample databases are on the shelf —
  classic games (38 famous games from Anderssen and Morphy to Kasparov and Carlsen, each with a
  short introduction; four with full comments, variations and arrows) and Alekhine's games (267
  games, moves only). Remove them from the shelf like any database; they do not come back.
- The first start uses the system language when it is one of the six (English, Russian, French,
  German, Spanish, Italian), otherwise English. A language chosen in the settings is kept.

## [0.6.0] — 2026-10-19

- Stockfish is no longer inside the packages (they shrink from about 95 MB to about 14 MB): the
  first time you analyse, ChessPenguin offers to download the official Stockfish 19 (checked
  before use, installed for your account only) and starts the analysis when it is there. The same
  button is in Settings → Engines, with progress, cancel and "remove" to free the space. A system
  `stockfish` is still found and offered; the Flatpak keeps Stockfish inside.
- macOS on Apple Silicon: a `.dmg` with ChessPenguin.app (not signed by Apple yet).
- Position search fixes (QA report 3): "in this move order" no longer reused from another move
  order of the same position; while a search runs, the arrow and the position a game opens at
  belong to the rows shown; in books the next move of a game ending in the position is no longer
  taken from a variation; average rating of a continuation only from games with both ratings.
- Narrow windows: the [Filters | Board] switch stays visible, conditions that do not fit become
  "+N more", columns hide by priority and the date becomes the year.
- Continuations are numbered through the whole list (keys 1-9 still play the first nine).
- Ctrl+Shift+Backspace clears all conditions with the position; a × removes the position in small
  databases; the cache of search results is bounded.

## [0.5.0] — 2026-10-19

- Find games by position: the "Board" pane next to the game list (Ctrl+B). Play moves and the list
  shows the games that reach the position by any move order, with the moves played next (games,
  results bar, average rating, last year); click a move to play it. Works together with the
  filters, sorts strongest games first, marks games reached by another move order (↪) and offers
  "only this order". Books and PGN collections are searched in variations too. A game opens at the
  found position. On the game screen "🔍 Find in database" (Ctrl+Shift+F) searches its position.
  Mega Database: 0.4 s for the starting position, under 0.1 s deeper in the opening.
- The game list's "Moves" column is now "Length".
- Filter fields and the search field have a × to clear them.
- "Open as a new game" from the search board opens at the last move played on it.

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
