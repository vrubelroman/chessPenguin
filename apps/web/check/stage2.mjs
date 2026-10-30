// «My games», the data in the browser and the files of a visit in headless Firefox:
// Ctrl+S puts a game of the samples into «My games» (kept in IndexedDB), a file kept as
// a copy opens after a reload, a file only opened asks to be chosen again.
//
//   node apps/web/check/stage2.mjs <app-dir> <out-dir>
//   (KEEP_X, KEEP_Y: where «Keep a copy in the browser» is, for a 1400×860 window)
import { copyFileSync, mkdirSync } from 'node:fs';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { session } from './steps.mjs';
import { sleep } from './bidi.mjs';

const [appDir, outDir] = process.argv.slice(2);
mkdirSync(outDir, { recursive: true });
const root = fileURLToPath(new URL('../../..', import.meta.url));
let failures = 0;
const check = (ok, what) => {
  console.log(`${ok ? 'ok  ' : 'FAIL'} ${what}`);
  if (!ok) failures++;
};

const s = await session(appDir, 18810);
const { t } = s;
const kept = `new Promise((ok) => {
  const q = indexedDB.open('chesspenguin');
  q.onsuccess = () => {
    const st = q.result.transaction('files').objectStore('files');
    const keys = st.getAllKeys(), values = st.getAll();
    values.onsuccess = () => ok(JSON.stringify(keys.result.map((k, i) => [k, new TextDecoder().decode(values.result[i]).slice(0, 2000)])));
  };
})`;
const files = async () => new Map(JSON.parse(await t.evaluate(kept)));
const step = (x) => s.run(x, outDir);
try {
  await step({ url: '?lang=en' });
  await sleep(2500);
  await step({ shot: 's2-1-library.png' });
  // A game of the samples into «My games».
  await step({ click: [300, 322], double: true });
  await sleep(1500);
  await step({ key: 's', mods: ['ctrl'] });
  await sleep(3000);
  await step({ shot: 's2-2-saved.png' });
  let f = await files();
  const mine = [...f.keys()].find((k) => k.startsWith('/data/my-games/'));
  check(!!mine && /Anderssen/.test(f.get(mine)), `the game is in «My games» in the browser's storage (${mine})`);
  await step({ click: [55, 15] });
  await sleep(1500);
  await step({ shot: 's2-3-my-games.png' });
  await step({ key: ',', mods: ['ctrl'] });
  await sleep(1000);
  await step({ shot: 's2-4-settings.png' });
  await step({ click: [1366, 77] });
  // Two files opened; one kept as a copy.
  const a = join(s.b.profile, 'Kept games.pgn');
  const b = join(s.b.profile, 'Visit games.pgn');
  copyFileSync(join(root, 'examples/alekhine.pgn'), a);
  copyFileSync(join(root, 'examples/classics.pgn'), b);
  for (const file of [b, a]) {
    await t.choose([file], () => step({ key: 'o', mods: ['ctrl'] }));
    await sleep(2500);
  }
  await step({ shot: 's2-5-opened.png' });
  if (process.env.KEEP_X) {
    await step({ click: [Number(process.env.KEEP_X), Number(process.env.KEEP_Y)] });
    await sleep(2000);
    await step({ shot: 's2-6-kept.png' });
    f = await files();
    check([...f.keys()].some((k) => k.startsWith('/data/copies/Kept games')), 'a copy of the file is kept in the browser');
  }
  // After a reload: «My games» and the copy are there, the other file is to be chosen again.
  await step({ url: '?lang=en' });
  await sleep(3000);
  await step({ shot: 's2-7-reloaded.png' });
  f = await files();
  check(!!mine && /Anderssen/.test(f.get(mine) || ''), '«My games» after a reload');
} finally {
  await s.stop();
}
for (const e of s.errors) console.log('console:', e);
check(s.errors.length === 0, 'no panic or error in the console');
console.log(failures ? `${failures} failed` : 'all ok');
process.exit(failures ? 1 : 0);
