# Changelog

All notable changes are listed here. Versions follow [Semantic Versioning](https://semver.org/).

## [Unreleased]

## [1.9.2] — 2026-10-27

- Desktop, a game against the computer: the header shows its time control («5+3», «∞»)
  where the result comes after it; the players' cards show the king of the chosen piece
  set; the player's clock turning red is heard once a game (a double tick, every sound
  set); «The game is on» says which game is left («Maia 1550 · you play white · 5+3 ·
  move 5»).
- Android: «+ Add» sits in the Files heading, as «+ New database» in My databases,
  instead of a button floating over the list.

## [1.9.1] — 2026-10-27

- macOS: Rodent IV plays. Its build had no home folder on macOS (its authors left it
  unwritten), found none of its personalities and offered no «Personality», so every game
  ended at once with «Rodent did not start: it does not start on this computer». The
  engine now takes its own folder, as on Linux and Windows; the program offers to
  download it again, and only the engine itself (250 KB) is fetched. Its build is now
  checked to offer its personalities from a folder like the one the program uses.

## [1.9.0] — 2026-10-27

- Android: a new version is offered at each start of the app, as on the desktop (it used to
  ask once a day, so a release out a day after the last check was not seen at once).
- macOS: an engine downloaded over an older one (Rodent IV from 1.7 to 1.8) no longer
  fails «it does not start on this computer»: a download is written beside and put in
  place, never rewritten in place (Apple silicon kills an executable changed that way).
- Desktop: a move can be taken back only in a blitz game against the computer; in rapid,
  classical and games without a clock «↶ Take back» (and Ctrl+Z) is gone.

## [1.8.0] — 2026-10-27

- macOS: Rodent IV out of its opening book searched garbage (a bug of its sources that
  only builds by clang show) and is now built with the fix; an installed Rodent IV is
  offered for download again, and only the engine itself (250 KB) is fetched.
- Android: Rodent IV joins Maia as an opponent. «Play» opens the sheet with the choice of
  Maia or Rodent IV and, for Rodent, one of its 13 characters or 22 famous players (Tal,
  Kasparov, Karpov…), each with its own strength scale (Rodent: 800–2800). The engine is
  inside both APKs; its opening books are one download of 59 MB, removable in Settings,
  and «No book» plays without them. Rematches and put-aside games keep the opponent.
- A game against the computer can be put aside and continued later: «⏸ Put aside» in
  the game, the main answer of «The game is on», and closing the window put it aside;
  a card on the library's shelf continues it with the clocks as they were. A game a
  crash interrupted is put aside too, no longer written into the database unfinished.
  Starting a new game over it asks first.
- The window no longer freezes when a database on a network drive (NFS) becomes
  unreachable, for example when the network drops during a game: whether the shelf's
  files are there, their sizes and permissions are looked at by a thread of their own,
  not in the frame.

## [1.7.0] — 2026-10-26

- Rodent's list of styles gets Rodent IV's: 13 characters (Amanda, Defender, Strangler…)
  and 22 famous players (Alekhine, Botvinnik, Fischer, Karpov, Kasparov, Petrosian, Tal…),
  each with its opening books. The opponent is just «Rodent»; which engine plays follows
  the style chosen (Rodent IV's strength: 800–2800). Rodent IV is downloaded once on
  request: built by us from its authors' sources, with its characters and books from their
  repository — on macOS too, where Rodent was not available before.

- Android: the lists look richer. Games are divided by thin lines, each side has its
  colour disc, the winner's «1» stands on a tile, the opening code (ECO) on a tag of its
  own, and the game you came back from is marked. Search also finds by the ECO. On
  «Databases» each kind has its colour (Catppuccin: PGN, ChessBase, archives, downloaded,
  your own), groups are cards, and a game put aside shows its position. Settings are
  grouped the same way, with icons.

## [1.6.0] — 2026-10-26

- Games against the computer are written by themselves into the player's database,
  «Roman — games against the computer» (without a name, «Games against the computer»),
  made at the first game in the folder of new databases. The play dialog says where the
  games go; the result strip says «Written to the database, No. n · Open». A game the program
  stopped in (a crash, a power cut) is written at the next start, unfinished. Several
  players of one computer each get their own database (the ▾ list by the name). It can
  be turned off: Settings → Interface → Your databases. Android: the same in «My
  databases» (♞), the result card says where the game went; a game put aside and let
  go is written unfinished; Settings → «Write games against Maia…».
- Play against Rodent V (desktop, Linux and Windows): a strong engine with a style —
  balanced, attacking, chaotic, restricting, classical, or Tal. It is downloaded once on
  request (48 MB, the authors' release).
- The opponent's strength is on the engine's own scale, as its authors give it: Maia
  600–2600, Rodent V 800–3000 (its UCI_Elo). The dialog says that these are the engines'
  ratings, not a person's Elo. (1.5's slider showed Maia's strength measured on the
  Lichess blitz scale; a remembered strength keeps its number.) Android too: Maia
  600–2600. The engine's strength is in its name in the game («Maia 1550»),
  with no Elo tag for it.
- Offer Maia a draw (desktop and Android): she accepts when she is clearly worse, or
  when the position is about level (at most +0.2 for her) and has kept its character for
  her last three moves; after a refusal, again two of her moves later. She judges with
  Stockfish, as for her resignation.

## [1.5.0] — 2026-10-26

- Play against Maia: the large model (79M parameters, 316 MB, downloaded once from the
  authors' page) plays every strength; it is stronger and closer to how people play.
  The slider shows her measured strength on the Lichess blitz scale (1100–2100; at the
  top she chooses her moves more surely). The small model, when it is there, still
  plays up to 1800. Maia thinks on half the cores; her clock counts the computing when
  it takes longer than her pause.
- Game analysis: «Analyse» starts at once with the options chosen the last time; the
  options dialog comes the first time and with «Analyse again…».
- Android: moves on the board (tap a piece and a square, or drag; a pawn becomes a
  queen), variations, «Make it the main line» and «Delete» on a long press, «Undo».
  A new game from the start; your own databases on the phone (create, rename, export
  the file, delete) and saving games into them, with the game's data.
- Android: play against Maia (the small model, 21 MB, downloaded once; no clocks),
  from the start or «Play on from here» after a move; take back, resign, Maia offers
  to resign when lost; the game is put aside with «back» and goes on later. Learn an
  opening up to a move, as on the desktop. Sounds, with a setting.
- The opening trainer and the rules of a game against Maia are shared by both apps.

## [1.4.0] — 2026-10-25

- The library's title row in three groups: databases | a game to enter | Play.
- Paste a line from the clipboard (Ctrl+V on a game, or the move menu): moves («Nf3 d5»,
  «12...Rd8», UCI) go after the selected move or instead of it; a PGN game goes where its
  moves part from this game — along any branch, or by another move order — with its
  comments, symbols and variations and the players' names at the end. A game that shares
  no position with this one is not pasted, with a message; a line starting before the
  selected move is asked about. One Ctrl+Z takes it all back.
- Play on from here (the move menu, Ctrl+G on a game): a game against Maia from the
  position, its moves up to it kept (Maia sees the history, repetitions count); the
  game you came from waits under it, «⬅ Back to the game» returns to it. The side to
  move plays by default; a rematch keeps the colour.
- Learn the opening (the move menu): replay from memory the moves of both sides up to
  the chosen move. A wrong move is counted, shown for a moment with a short sound and
  taken back; from the third mistake on a move you may ask to see it. At the end: the
  mistakes by move, the runs of the line (last, best, clean in a row, recent), «Again».
  The runs are kept by the line itself, across games.

## [1.3.0] — 2026-10-25

- Play against the computer (desktop): «♞ Play» in the library (Ctrl+G) starts a game
  against Maia — a neural network (Maia-3, CSSLab, University of Toronto) that plays like
  a human of the chosen rating, 600–2600, mistakes included. Your name and rating
  (optional; «Weaker · As me · Stronger» choose the strength relative to it), colour,
  Lichess time controls from 3+0 to 30+20 (10+5 by default), custom, or no time limit. During the game: the
  board, the notation, both clocks (time runs after each side's first move, increment
  after the move, low time in red), take back, resign; no engine or analysis. Maia
  thinks like a human — a pause that depends on her clock — and never loses on time. The
  game ends by mate, stalemate, no material, threefold repetition, fifty moves, time or
  resignation; then it is an ordinary game with the result strip: Analyse, Rematch, New
  game, Save to a database (tags: players, ratings, TimeControl, Termination, [%clk]).
  Maia is computed in plain Rust, about 0.1 s a move on one core; its weights (21 MB,
  the authors' own file from huggingface.co) are downloaded once on request.
- Premoves against Maia, as on Lichess: while she thinks, set your next move; it is
  played at once after hers when legal.
- Maia resigns: when her position is lost by twelve pawns or more for two of her turns
  (judged by Stockfish, unseen), she offers to resign — accept the win, or play to the
  end.
- Settings › Board: «Always promote to a queen» (off: the piece is chosen).

## [1.2.0] — 2026-10-25

- Your own comments in a game (desktop): Enter on a move (or the right-click menu, a
  double click on a comment, the ☰ menu) opens an editor right where the comment stands
  in the notation — after any move of the main line or of a variation, or at the start of
  the game; Enter — done, Esc — cancel, Shift+Enter — a paragraph; going to another move
  writes the draft. Symbols of the move (! ? !! ?? !? ?!, also Ctrl+1…6) and of the
  position (= ∞ ⩲ ⩱ ± ∓ +− −+) in the editor and in the move's menu, one click on, one
  click off. Arrows and circles drawn with the right mouse button on the board (green;
  Shift red, Alt blue, Shift+Alt yellow; the same shape again takes it off) go into the
  comment as `[%cal]`/`[%csl]`. Existing comments, book ones too, can be edited and
  deleted (Ctrl+Z puts them back); their `[%clk]`, `[%eval]` and board commands are kept,
  an untouched comment keeps its bytes. One edit is one undo step; the game is marked
  modified and Ctrl+S saves it. Paragraphs of comments survive writing now.
- The strip «Engine line preview … Add as variation · Exit» above the notation is gone:
  stepping through a line of the report or of the engine panel just shows it on the
  board, Esc leaves it, and a small «+ Add as variation» stands at the end of the line
  being shown. Moves of variations and of the report's better lines keep their size under
  the mouse.

## [1.1.0] — 2026-10-25

- Your own databases (desktop): «✚ New database» on the shelf makes an empty PGN file
  (Documents/ChessPenguin by default, the folder is shown and can be changed); any game —
  new, pasted, from a ChessBase, an online or any other database — is added to a PGN of
  your choice with «Add to database…» (Ctrl+Shift+S, the 💾 button, the ☰ menu); Ctrl+S
  saves the changes back into the game's own PGN (replacing its record) or, for a game
  from a read-only source, adds it to a database. The data of a game (players, Elo, event,
  site, date, round, result, annotator from «Your name» in Settings) are asked when a new
  game is first added and can be edited any time (F2, the title, the ☰ menu). A game that
  is already in the target (same players, date and moves) can replace its record or go in
  as a new one. Games are deleted from the table (Delete, the row menu) after a question,
  with Undo for six seconds (Ctrl+Z). Online databases and ChessBase files stay read-only;
  a PGN in a legacy encoding is offered a UTF-8 copy next to it. Writes are explicit:
  appending to the file, or rewriting it through a temporary file and a rename, with a
  check that the file did not change on the disk meanwhile. The game's header names its
  database and number (🔒 read-only, 🌐 online); the card menu has «Show in folder»;
  «Remove from list» says where the file stays. A new game gets today's date.
- Library: «+ Add database», «✚ New database» and «🌐 Online games» moved from the row of
  database cards to the title row, beside «Databases» (folded into «+ ▾» and «🌐» in a
  narrow window); the cards row holds only databases. An empty shelf shows the three
  actions as large cards in the middle. The table header of a database that takes games
  has «+ Game»: a new game meant for that database — its header says so and Ctrl+S adds
  it there straight away. «Add to database» lists only databases that take games; a PGN
  in a legacy encoding gets its UTF-8 copy from its card's menu. The built-in sample
  databases are read-only. An empty database shows no filter panel.

## [1.0.3] — 2026-10-24

- A new icon — a penguin in a crown — on the desktop, Android and the website.

## [1.0.2] — 2026-10-24

- Android: the update card on the main screen has a × that stops the offers (with "Undo";
  they come back with "Offer new versions" in Settings → About, where a new version can
  always be installed); the update check and the counting go on. No "What's new" link to
  GitHub anywhere in the app's update screens.

## [1.0.1] — 2026-10-24

- The update check goes through the ChessPenguin site (GitHub when the site does not
  answer), which counts installations that run: once a day, only a random number of the
  installation, the version and the platform. With the check at start turned off,
  nothing is sent unless one checks by hand. The site's statistics show installations by day, platform and version.

## [1.0.0] — 2026-10-24

- Three more interface languages on the desktop, on Android and on the website: Brazilian
  Portuguese, Uzbek (Latin script) and Kazakh (Cyrillic script) — nine in all. The
  system's language is picked up; Portuguese notation uses its own piece letters.
- Android: the players stand above and below the board, as on Lichess (Black above,
  swapped with the board), with their ratings and their share of the result; the top bar
  names the event, the year and the round. In the list of games White and Black have a
  line each, so neither name is cut.

## [0.16.2] — 2026-10-24

- Desktop: narrow windows (half a screen, a tiling window manager) no longer overlap:
  - the reference tab, the engine settings and the game's title fit their place (the
    reference tab slid under the board from a 1200-point window down);
  - in a narrow window the filters open over the list of games, and the search board
    takes the place of the list (Esc goes back);
  - a narrow tall window shows the notation under the board;
  - the header of the list and the shelf shorten what does not fit, or take a second
    line; the window can be made as small as 640×480.
- Desktop: Escape in the "Game modified" question quits nothing: when closing the program
  it is Cancel (leaving a game, it still goes on without saving).

## [0.16.1] — 2026-10-24

- Desktop: the filter panel no longer widens while a name is typed; the rows of «Game»
  (years, rating, moves, ECO) are aligned on one grid.
- Desktop: Escape in a game with unsaved edits asks about saving (the question used to
  close at once, so Escape seemed to do nothing); Escape in the question goes on without
  saving, Cancel stays in the game.

## [0.16.0] — 2026-10-24

- Desktop: **every database gets the filters and the position search of a ChessBase
  database** — PGN files of any size, downloads from Lichess and Chess.com, archives and
  small ChessBase databases. Player and opponent (with the colour), tournament or place,
  annotator, years, rating, result, ECO, length, «with comments» (clock and evaluation
  marks such as `[%clk]` do not count) and «with variations»; names suggested as you
  type; the table sorts by any column. Fields a file does not have stay in the panel,
  and an empty result says when no game of the database has that field. Dates are read
  with dots, dashes or slashes (`2026-10-18`), the length of a game is counted when the
  file has no `PlyCount`.
- Desktop: the quick search is in the header of every database (Mega too); each word may
  be found in a different field ("carlsen 2024"). Ctrl+F goes there, again to the filters.
- Desktop: games downloaded from Lichess and Chess.com are shown newest first (by the
  date and time they were played); on Android the count over a downloaded database's
  games grows during its download.
- Names are found with «е» for «ё» («Артем» finds «Артём»).
- The memory a PGN's position search needs is reckoned from its moves, not from the size
  of the file (a file with clock comments was thought to need 14 times too much).

## [0.15.0] — 2026-10-23

- Android: **the app updates itself.** Once a day (a switch in About turns it off; «Check
  now» is there too) it asks GitHub for the latest release. A newer one shows as a card
  over the list of databases: «Update» downloads the small build (about 9 MB; the engine
  already on the phone stays — the build with Stockfish inside keeps it first), checks it
  against the release's SHA-256 and hands it to Android to install. The first time Android
  asks to allow installs from ChessPenguin; the app leads there and back. The download
  goes on with the app in the background, and the next start says what was updated.
- The release page on GitHub now lists what is new in that version (from this file); the
  apps' and the site's «What's new» lead there.
- The desktop and Android apps share one check for new releases.

## [0.14.0] — 2026-10-23

- Android: **games from Lichess and Chess.com** — «Add a database» → «Games from Lichess
  and Chess.com»: any player's games by nickname (yours, a pupil's, an opponent's; no login,
  another nickname for Chess.com if it differs) become a database on the phone, both sites
  merged into one by default. «Download new games» — in the database's menu and over its
  games list — fetches only the games played since; a download that was stopped, failed or
  cut off by a closed app continues where it stopped, without duplicates. The games of such
  a database are listed from the most recent one. The download goes on when the screen is
  left; the list shows its progress and says how it ended. Only standard chess is kept. The
  app asks the sites only when told to (both builds may now use the internet for this).
- The downloads of games from Lichess and Chess.com are one shared part of the desktop and
  Android apps.
- Android: the status bar and the navigation bar follow the theme chosen in the app: with
  the dark theme on a light phone their icons are light again (they stayed dark on dark).

## [0.13.0] — 2026-10-23

- Android: **game report** — ⋮ → «Game analysis» lets Stockfish evaluate every move of the
  main line, as on the desktop: inaccuracies, mistakes and blunders marked in the notation,
  an evaluation graph under the board (tap or drag to go to a move), each player's accuracy
  and average centipawn loss in the report, the better move of every error shown on the
  board. Games with `[%eval]` comments (Lichess exports) and games analysed before show
  their report at once. The live engine waits while the game is analysed.
- The game report's evaluations, judgements, accuracy and cache are now one shared part of
  the desktop and Android apps, so both give the same numbers.

## [0.12.0] — 2026-10-23

- Android: **paste PGN** — when the clipboard holds text, «Add a database» offers to paste
  it: the games are kept in the app as a database and opened (one game on the board at
  once). Text with no game in it is said so.
- Android: **open with ChessPenguin** — PGN, ZIP, 7z and ChessBase archives (`.cbv`) open
  from other apps (Telegram, a file manager, «Share»): the file is copied into the app,
  put on the list and opened. The same file again opens the same database.
- Android: the engine button turns the engine off again (in 0.11.1 it only turned it on).
- Android: a smoother screen while the engine thinks — its words are shown up to five
  times a second and redraw only the engine strip and the arrows; Stockfish runs below the
  app's priority and takes 2 threads by default (a value chosen in Settings stays).
- PGN: a game whose tags follow the previous game's result on the same line (`… 1-0
  [Event "B"]`, as pasted text often has it) now shows its players in the games list too.

## [0.11.1] — 2026-10-23

- Android: **boards and pieces** — Settings → «Board and pieces» offers the 37 boards and
  17 piece sets of the desktop app, seen at once on a board above them.
- Android: the games list shows each game's number in the database (it stays the same
  when a search narrows the list).
- Android: the engine turns on only with its button in each game, and turns off when you
  leave the game or the app goes to the background.
- Android: the engine download no longer says there is not enough space on a phone with
  plenty of it (the free space was asked of a folder not made yet).

## [0.11.0] — 2026-10-22

- Android: **add a folder** — every database in it and in the folders inside it (two
  levels) is offered with a tick box: PGN files, archives, ChessBase archives (`.cbv`) and
  ChessBase databases (`.cbh` with the files beside it), which a single picked file could
  not open. A ChessBase database without its `.cbg` is shown but not added; databases
  already on the list are marked. Picking the folder again offers what is new. Errors say
  in plain words when a file was moved or a part of a ChessBase database is missing.
- Desktop, keys in a game: ↑ goes to the end, ↓ to the start (as End and Home); variations are
  switched with Shift+↑/↓. Escape first closes what is open (a preview of an engine line,
  a picked-up piece); then it goes back to the game this one was opened from, or to the
  library. A window, menu or dialog over the game keeps Escape for itself, and a game with
  unsaved changes still asks first.

## [0.10.0] — 2026-10-22

- **ChessPenguin for Android, alpha** (12 and newer): add databases of the same formats (PGN, ChessBase
  `.cbv`, `.zip`/`.7z` archives), the desktop's sample games from the first start, a list of
  games with a search, the game with the notation under the board (variations, comments, NAG,
  comment arrows), paging games, and Stockfish 19 with its evaluation, lines and a preview of
  a line on the board. Two APKs: Stockfish built in, or Stockfish downloaded in the app.
  Files are only read, never changed. The engine's download goes on in the background, with
  the app closed too. In English, Russian, German, French, Spanish and Italian.

## [0.9.0] — 2026-10-22

The 0.8 series together — the reference tab, games from Lichess and Chess.com, self-update,
translated comments, move notation and piece animation — with these fixes:

- Symbols that had no glyph and showed as empty boxes now render: the "✕" beside the
  Chess.com nickname, the start-position mark in board search, the check marks of the
  translation texts. A test checks every character the interface shows against the bundled
  fonts.

## [0.8.6] — 2026-10-21

- The side panel no longer widens by itself while a game's comments are translated (the mark
  of a translated comment made its row a pixel wider than the panel, frame after frame), so
  moves in comments no longer jump when the mouse moves over them.
- Comments inside variations have the same size as the other comments.
- No scroll bar pops up over the engine lines: a long line is cut at the edge (it still scrolls
  with Shift+wheel or by dragging).
- A game whose comments go in one request shows just "Translating…": the count
  ("12 of 40") is kept for long games sent in several parts.

## [0.8.5] — 2026-10-21

- Translation with a ChatGPT subscription works: the list of models the subscription answers
  with is now understood (it differs from the API's), so requests no longer go out without a
  model and fail with "error 400". OpenAI's preferred model is used. A failed check now shows
  in Settings › Translation, and translation errors are written to chesspenguin.log.
- Comments of a sentence or two in another language are offered for translation too, and text
  in brackets ("[source: …]") is translated as well.

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
