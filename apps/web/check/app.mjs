// The web version in headless Firefox: the page for phones, the program, opening a PGN
// by its button (Ctrl+O) and by dropping it, a game from the classics, moves, a
// download of the PGN. Screenshots go to <out-dir>; what is checked is printed.
//
//   node apps/web/check/app.mjs <app-dir> <out-dir>     (app-dir: apps/web/build.sh's)
import { spawn } from 'node:child_process';
import { copyFileSync, existsSync, mkdirSync, readdirSync, readFileSync, writeFileSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { launch } from './firefox.mjs';
import { sleep, tab } from './bidi.mjs';

const [appDir, outDir] = process.argv.slice(2).map((p) => resolve(p));
const root = fileURLToPath(new URL('../../..', import.meta.url));
mkdirSync(outDir, { recursive: true });
const port = 18766;
const server = spawn('python3', ['-I', join(root, 'apps/web/check/serve.py'), appDir, join(root, 'website/static'), String(port)], { stdio: 'ignore' });
const base = `http://127.0.0.1:${port}/app/`;

let failures = 0;
const check = (ok, what) => {
  console.log(`${ok ? 'ok  ' : 'FAIL'} ${what}`);
  if (!ok) failures++;
};
const errors = [];
const log = (line) => {
  if (/panicked|RuntimeError|did not start|\[error\]/.test(line)) errors.push(line);
};

const ff = await launch(19223);
const shot = async (t, name) => writeFileSync(join(outDir, name), await t.screenshot());
const key = (value, mods = []) => {
  const down = mods.map((m) => ({ type: 'keyDown', value: m }));
  const up = mods.map((m) => ({ type: 'keyUp', value: m })).reverse();
  return [...down, { type: 'keyDown', value }, { type: 'keyUp', value }, ...up, { type: 'pause', duration: 80 }];
};
const CTRL = '';
const RIGHT = '';
const click = (x, y, count = 1) => {
  const acts = [{ type: 'pointerMove', x, y }];
  for (let i = 0; i < count; i++) acts.push({ type: 'pointerDown', button: 0 }, { type: 'pointerUp', button: 0 });
  return [{ type: 'pointer', id: 'mouse', parameters: { pointerType: 'mouse' }, actions: acts }];
};
const started = (t) => t.waitFor(`document.getElementById('cp-status').hidden && !document.getElementById('cp-app').hidden`, 30000);

try {
  // ----- the page for phones and tablets, without the program ------------------------------
  {
    const t = await tab(ff.browser, 390, 844, log);
    await t.navigate(`${base}?phone=1&lang=ru`);
    await sleep(800);
    const text = await t.evaluate(`document.getElementById('cp-device').innerText`);
    check(/ChessPenguin на телефоне и планшете/.test(text), 'the device page in Russian');
    check(/Всё равно открыть веб-версию/.test(text), 'with «open the web version anyway»');
    const wasm = await t.evaluate(`performance.getEntriesByType('resource').some(e => e.name.endsWith('.wasm'))`);
    check(!wasm, 'the program is not downloaded on it');
    await shot(t, 'web-phone-ru.png');
    await t.navigate(`${base}?phone=1&lang=de`);
    await sleep(500);
    check(/auf Handy und Tablet/.test(await t.evaluate(`document.getElementById('cp-device').innerText`)), 'the device page in German');
    await t.navigate(`${base}?phone=android&lang=ru`);
    await sleep(800);
    check(/Скачать для Android/.test(await t.evaluate(`document.getElementById('cp-device').innerText`)), 'the Android variant');
    await shot(t, 'web-phone-android-ru.png');
    await t.navigate(`${base}?phone=ios&lang=en`);
    await sleep(800);
    check(/iPhone and iPad app is coming soon/.test(await t.evaluate(`document.getElementById('cp-device').innerText`)), 'the iPhone and iPad variant');
    await shot(t, 'web-phone-ios-en.png');
  }

  // ----- the program -------------------------------------------------------------------------
  const t = await tab(ff.browser, 1400, 860, log);
  await t.navigate(`${base}?lang=en`);
  await started(t);
  await sleep(2500);
  check(true, 'the program started (WebGL canvas, no error)');
  await shot(t, 'web-1-library.png');

  // Open a PGN by the button's shortcut: the browser's file chooser gets the file.
  const pgn = join(ff.profile, 'Test games.pgn');
  copyFileSync(join(root, 'examples/alekhine.pgn'), pgn);
  await t.actions([{ type: 'key', id: 'kb', actions: key('o', [CTRL]) }]);
  await t.waitFor(`!!document.getElementById('cp-open-files')`, 5000);
  const input = await ff.browser.send('script.evaluate', {
    expression: `document.getElementById('cp-open-files')`,
    target: { context: t.context },
    awaitPromise: false,
  });
  await ff.browser.send('input.setFiles', { context: t.context, element: { sharedId: input.result.sharedId }, files: [pgn] });
  await sleep(2500);
  await shot(t, 'web-2-opened.png');

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
  await sleep(2500);
  await shot(t, 'web-3-dropped.png');

  // The classics: a game opens from the table (double click), its moves on the board.
  await t.actions(click(Number(process.env.CLASSICS_X || 100), Number(process.env.CLASSICS_Y || 70)));
  await sleep(1500);
  await t.actions(click(300, Number(process.env.ROW_Y || 322), 2));
  await sleep(1500);
  await shot(t, 'web-4-game.png');
  const steps = [];
  for (let i = 0; i < 12; i++) steps.push(...key(RIGHT));
  await t.actions([{ type: 'key', id: 'kb', actions: steps }]);
  await sleep(800);
  await shot(t, 'web-5-moves.png');

  // Download the PGN of the game (the header's button).
  await t.actions(click(Number(process.env.DOWNLOAD_X || 1180), Number(process.env.DOWNLOAD_Y || 22)));
  await sleep(2000);
  const files = existsSync(ff.downloads) ? readdirSync(ff.downloads).filter((f) => f.endsWith('.pgn')) : [];
  check(files.length === 1, `a PGN downloaded: ${files.join(', ') || 'none'}`);
  if (files.length) {
    const text = readFileSync(join(ff.downloads, files[0]), 'utf8');
    check(/\[White "/.test(text) && /1\. /.test(text), 'the download is the game in PGN');
    writeFileSync(join(outDir, 'web-download.pgn'), text);
  }
  await shot(t, 'web-6-downloaded.png');

  // A move on the board of a new game (Ctrl+N): e2–e4 by two clicks.
  await t.actions([{ type: 'key', id: 'kb', actions: key('') }]);
  await sleep(500);
  await t.actions([{ type: 'key', id: 'kb', actions: key('') }]);
  await sleep(800);
  await t.actions([{ type: 'key', id: 'kb', actions: key('n', [CTRL]) }]);
  await sleep(1200);
  await shot(t, 'web-7-new-game.png');
  if (process.env.E2 && process.env.E4) {
    const [x2, y2] = process.env.E2.split(',').map(Number);
    const [x4, y4] = process.env.E4.split(',').map(Number);
    await t.actions(click(x2, y2));
    await sleep(300);
    await t.actions(click(x4, y4));
    await sleep(1000);
    await shot(t, 'web-8-e4.png');
  }

  // The settings and the marks survive a reload (the browser's storage).
  await t.navigate(`${base}?lang=en`);
  await started(t);
  await sleep(2500);
  await shot(t, 'web-9-reloaded.png');
} finally {
  await ff.stop();
  server.kill();
}
for (const e of errors) console.log('console:', e);
check(errors.length === 0, 'no panic or error in the console');
console.log(failures ? `${failures} failed` : 'all ok');
process.exit(failures ? 1 : 0);
