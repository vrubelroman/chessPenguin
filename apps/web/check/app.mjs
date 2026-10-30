// The web version in a headless browser (Firefox; Chromium with CP_BROWSER=chromium):
// the page for phones, the program, opening a PGN by its button (Ctrl+O) and by
// dropping it, a game from the classics, moves, a download of the PGN, a move on the
// board, the browser's Back. Screenshots go to <out-dir>; what is checked is printed.
//
//   node apps/web/check/app.mjs <app-dir> <out-dir>     (app-dir: apps/web/build.sh's)
import { copyFileSync, existsSync, mkdirSync, readdirSync, readFileSync, writeFileSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { session } from './steps.mjs';
import { sleep } from './bidi.mjs';

const [appDir, outDir] = process.argv.slice(2).map((p) => resolve(p));
const root = fileURLToPath(new URL('../../..', import.meta.url));
mkdirSync(outDir, { recursive: true });
let failures = 0;
const check = (ok, what) => {
  console.log(`${ok ? 'ok  ' : 'FAIL'} ${what}`);
  if (!ok) failures++;
};
// Places of buttons in a 1400×860 window.
const DOWNLOAD = [Number(process.env.DOWNLOAD_X || 1263), Number(process.env.DOWNLOAD_Y || 24)];
const E2 = [519, 703];
const E4 = [519, 505];

const s = await session(appDir, 18766);
const { t } = s;
const shot = async (name) => writeFileSync(join(outDir, name), await t.screenshot());
const text = () => t.evaluate(`document.getElementById('cp-device').innerText`);
const started = () => t.waitFor(`document.getElementById('cp-status').hidden && !document.getElementById('cp-app').hidden`, 30000);
try {
  // ----- the page for phones and tablets, without the program ----------------------------
  await t.navigate(`${s.base}?phone=1&lang=ru`);
  await sleep(800);
  check(/ChessPenguin на телефоне и планшете/.test(await text()), 'the device page in Russian');
  check(/Всё равно открыть веб-версию/.test(await text()), 'with «open the web version anyway»');
  const wasm = await t.evaluate(`performance.getEntriesByType('resource').some(e => e.name.endsWith('.wasm'))`);
  check(!wasm, 'the program is not downloaded on it');
  await shot('web-phone-ru.png');
  await t.navigate(`${s.base}?phone=1&lang=de`);
  await sleep(500);
  check(/auf Handy und Tablet/.test(await text()), 'the device page in German');
  await t.navigate(`${s.base}?phone=android&lang=ru`);
  await sleep(800);
  check(/Скачать для Android/.test(await text()), 'the Android variant');
  await t.navigate(`${s.base}?phone=ios&lang=en`);
  await sleep(800);
  check(/iPhone and iPad app is coming soon/.test(await text()), 'the iPhone and iPad variant');

  // ----- the program -------------------------------------------------------------------------
  await t.navigate(`${s.base}?lang=en`);
  await started();
  await sleep(2500);
  check(true, `the program started in ${s.b.name}`);
  await shot('web-1-library.png');

  // Open a PGN by the button's shortcut: the browser's file chooser gets the file.
  const pgn = join(s.b.profile, 'Test games.pgn');
  copyFileSync(join(root, 'examples/alekhine.pgn'), pgn);
  await t.choose([pgn], () => t.key('o', ['ctrl']));
  await sleep(3000);
  await shot('web-2-opened.png');

  // Drop a file on the window.
  const dropped = readFileSync(join(root, 'examples/classics.pgn'), 'utf8');
  await t.evaluate(`(() => {
    const dt = new DataTransfer();
    dt.items.add(new File([${JSON.stringify(dropped)}], 'Dropped classics.pgn', { type: 'application/x-chess-pgn' }));
    const c = document.getElementById('cp-canvas');
    for (const type of ['dragenter', 'dragover', 'drop'])
      c.dispatchEvent(new DragEvent(type, { dataTransfer: dt, bubbles: true, cancelable: true, clientX: 700, clientY: 400 }));
    return true;
  })()`);
  await sleep(3000);
  await shot('web-3-dropped.png');

  // A game from the table (double click), its moves on the board.
  await t.click(300, 322, { double: true });
  await sleep(1500);
  await shot('web-4-game.png');
  await t.key('right', [], 12);
  await sleep(800);
  await shot('web-5-moves.png');

  // Download the PGN of the game (the header's button).
  await t.click(...DOWNLOAD);
  await sleep(2500);
  const files = existsSync(s.b.downloads) ? readdirSync(s.b.downloads).filter((f) => f.endsWith('.pgn')) : [];
  check(files.length === 1, `a PGN downloaded: ${files.join(', ') || 'none'}`);
  if (files.length) {
    const body = readFileSync(join(s.b.downloads, files[0]), 'utf8');
    check(/\[White "/.test(body) && /1\. /.test(body), 'the download is the game in PGN');
  }

  // The browser's Back is «← Library».
  await t.evaluate(`history.back()`);
  await sleep(1500);
  await shot('web-6-back.png');
  check(await t.evaluate(`history.state === null || !history.state.cpGame`), 'Back left the game for the library');

  // A move on the board of a new game (Ctrl+N; N in a browser): e2–e4 by two clicks.
  await t.key('n');
  await sleep(1200);
  await t.click(...E2);
  await sleep(300);
  await t.click(...E4);
  await sleep(1000);
  await shot('web-8-e4.png');

  // An unsaved game: the browser asks before the page is left, and its draft is kept.
  check(await t.evaluate(`typeof window.onbeforeunload === 'function'`), 'unsaved: the browser asks before leaving');
  await sleep(1500);
  const draft = () => t.evaluate(`new Promise((ok) => {
    const q = indexedDB.open('chesspenguin');
    q.onsuccess = () => {
      const r = q.result.transaction('files').objectStore('files').get('/data/draft.json');
      r.onsuccess = () => ok(r.result ? JSON.parse(new TextDecoder().decode(r.result)).pgn : '');
    };
    q.onerror = () => ok('');
  })`);
  check(/1\. e4/.test(await draft()), 'the draft of the unsaved game is kept');

  // The settings and the shelf after a reload (the browser's storage). The question on
  // leaving is answered «Leave».
  await t.evaluate(`(window.onbeforeunload = null, true)`);
  await t.navigate(`${s.base}?lang=en`);
  await started();
  await sleep(2500);
  await shot('web-9-reloaded.png');
  check(/1\. e4/.test(await draft()), 'the draft is there after the reload (offered over the library)');
  if (await t.evaluate(`typeof window.showOpenFilePicker === 'function'`)) {
    // Chrome, Edge: the file chosen before the reload is read again by itself.
    check(s.logs.some((l) => /remembered file read again: .*Test games\.pgn/.test(l)), 'the chosen file is remembered (File System Access)');
  }
} finally {
  await s.stop();
}
for (const e of s.errors) console.log('console:', e);
check(s.errors.length === 0, 'no panic or error in the console');
console.log(failures ? `${failures} failed` : 'all ok');
process.exit(failures ? 1 : 0);
