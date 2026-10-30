// ChessBase databases in the browser (several files): a .cbh chosen alone asks for its
// folder, the folder's files make the database; all its files chosen at once; one too
// big for a browser is refused politely; Chrome remembers the folder.
//
//   node apps/web/check/chessbase.mjs <app-dir> <out-dir> <database-folder> [<folder-of-a-database-over-2-GB>]
//
// <database-folder>: a small ChessBase database's folder (.cbh, .cbg, …), only that
// database in it. Firefox by default, Chromium with CP_BROWSER=chromium (browser.mjs).
import { readdirSync, writeFileSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { session } from './steps.mjs';
import { sleep } from './bidi.mjs';

const [appDir, outDir, baseDir, bigDir] = process.argv.slice(2).map((p) => p && resolve(p));
let failures = 0;
const check = (ok, what) => {
  console.log(`${ok ? 'ok  ' : 'FAIL'} ${what}`);
  if (!ok) failures++;
};
// «📁 Choose the database's folder» in the window about a .cbh chosen alone (1400×860).
const ADD = [155, 20];
const FOLDER = (process.env.FOLDER || '833,471').split(',').map(Number);
const s = await session(appDir, 18768);
const { t } = s;
const shot = async (name) => writeFileSync(join(outDir, name), await t.screenshot());
const started = () => t.waitFor(`document.getElementById('cp-status').hidden && !document.getElementById('cp-app').hidden`, 30000);
const read = (name) => s.logs.filter((l) => l.includes(`database read: ${name}`));
const cbh = readdirSync(baseDir).find((n) => n.toLowerCase().endsWith('.cbh'));
const stem = cbh.slice(0, -4);
try {
  await t.navigate(`${s.base}?lang=${process.env.CP_LANG || "en"}`);
  await started();
  await sleep(2500);

  // A .cbh alone: the window asks for the database's folder; nothing is opened yet.
  await t.choose([join(baseDir, cbh)], () => t.key('o', ['ctrl']));
  await sleep(2000);
  await shot('cb-1-lone.png');
  check(read(stem).length === 0, 'a .cbh alone is not opened');
  // The folder: the database's files are found by their names.
  await t.chooseFolder(baseDir, () => t.click(...FOLDER));
  await sleep(4000);
  await shot('cb-2-folder.png');
  check(read(stem).length === 1, `the folder makes the database: ${read(stem)[0] || 'not read'}`);

  // All its files chosen at once.
  const all = readdirSync(baseDir).map((n) => join(baseDir, n));
  // By the shelf's «+ Add database».
  await t.choose(all, () => t.click(...ADD));
  await sleep(4000);
  await shot('cb-3-files.png');
  check(read(stem).length === 2, 'all its files chosen at once make the database');

  if (bigDir) {
    // A database over the limit: refused before it is read, with the way to the
    // desktop program.
    const big = readdirSync(bigDir).find((n) => n.toLowerCase().endsWith('.cbh'));
    await t.choose([join(bigDir, big)], () => t.click(...ADD));
    await sleep(2000);
    await t.chooseFolder(bigDir, () => t.click(...FOLDER));
    await sleep(3000);
    await shot('cb-4-too-big.png');
    check(read(big.slice(0, -4)).length === 0, 'a database over 2 GB is not read');
  }

  // The next visit: Chrome reads the remembered folder's database again by itself.
  await t.navigate(`${s.base}?lang=${process.env.CP_LANG || "en"}`);
  await started();
  await sleep(3000);
  await shot('cb-5-reloaded.png');
  if (await t.evaluate(`typeof window.showDirectoryPicker === 'function'`)) {
    check(s.logs.some((l) => l.includes('remembered file read again') && l.includes(cbh)), 'the folder is remembered (File System Access)');
  }
} finally {
  await s.stop();
}
for (const e of s.errors) console.log('console:', e);
check(s.errors.length === 0, 'no panic or error in the console');
console.log(failures ? `${failures} failed` : 'all ok');
process.exit(failures ? 1 : 0);
