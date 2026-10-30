// The game review of the web version against the desktop program's, on games of the
// chess.com check set (crates/game-review/chesscom/): the browser reviews them (Stockfish
// 19 wasm in workers), its evaluations are read out of its storage and compared with the
// ones `chesscom_check` keeps (target/chesscom-check/v1): the same search must give the
// same evaluations, so the same !!/!.
//
//   node apps/web/check/review.mjs <app-dir> <out-dir> <native-cache-dir> <game.pgn>...
import { mkdirSync, readFileSync, writeFileSync, existsSync } from 'node:fs';
import { basename, join } from 'node:path';
import { session } from './steps.mjs';
import { sleep } from './bidi.mjs';

const [appDir, outDir, nativeDir, ...games] = process.argv.slice(2);
mkdirSync(outDir, { recursive: true });
const all = join(outDir, 'chesscom-games.pgn');
writeFileSync(all, games.map((g) => readFileSync(g, 'utf8').trim()).join('\n\n') + '\n');

const s = await session(appDir);
const { t } = s;
const stored = `new Promise((ok, no) => {
  const q = indexedDB.open('chesspenguin');
  q.onerror = () => no('no db');
  q.onsuccess = () => {
    const st = q.result.transaction('files').objectStore('files');
    const keys = st.getAllKeys(), values = st.getAll();
    values.onsuccess = () => ok(keys.result.map((k, i) => [k, new TextDecoder().decode(values.result[i])])
      .filter(([k]) => k.startsWith('/cache/analysis/')));
  };
}).then((r) => JSON.stringify(r))`;
const readStored = async () => JSON.parse(await t.evaluate(stored));
let failed = 0;
try {
  await s.run({ url: '?lang=en' }, outDir);
  await sleep(2000);
  // The games in one file, reviewed one by one.
  await s.run({ key: 'o', mods: ['ctrl'] }, outDir);
  await t.waitFor(`!!document.getElementById('cp-open-files')`, 5000);
  await t.setFiles('cp-open-files', [all]);
  await sleep(3000);
  await s.run({ shot: 'review-0-opened.png' }, outDir);
  for (let i = 0; i < games.length; i++) {
    const before = (await readStored()).length;
    await s.run({ click: [300, 214 + 27 * i], double: true }, outDir);
    await sleep(1500);
    await s.run({ key: 'a', mods: ['shift'] }, outDir);
    // The first time the engine is offered: downloaded, the review starts by itself.
    if (i === 0) {
      await sleep(1000);
      await s.run({ shot: 'review-0-offer.png' }, outDir);
      await s.run({ key: 'enter' }, outDir);
      await sleep(5000);
      await s.run({ shot: 'review-0-downloading.png' }, outDir);
    }
    const t0 = Date.now();
    for (;;) {
      await sleep(2000);
      if ((await readStored()).length > before) break;
      if (Date.now() - t0 > 600000) throw new Error(`no review of ${games[i]}`);
    }
    console.log(`${basename(games[i])}: reviewed in ${((Date.now() - t0) / 1000).toFixed(0)} s`);
    await s.run({ shot: `review-${i + 1}.png` }, outDir);
    await s.run({ click: [55, 15] }, outDir);
    await sleep(1000);
  }
  // The comparison.
  const fromBrowser = await readStored();
  for (const [key, text] of fromBrowser) {
    const name = basename(key);
    writeFileSync(join(outDir, name), text);
    const web = JSON.parse(text);
    const nativePath = join(nativeDir, name);
    if (!existsSync(nativePath)) {
      console.log(`FAIL ${name}: no native evaluations to compare with`);
      failed++;
      continue;
    }
    const native = new Map(JSON.parse(readFileSync(nativePath, 'utf8')).evals);
    let same = 0, differ = 0, missing = 0;
    for (const [k, e] of web.evals) {
      const n = native.get(k);
      if (!n) { missing++; continue; }
      if (JSON.stringify(n) === JSON.stringify(e)) same++;
      else {
        differ++;
        if (differ <= 3) console.log('   differs', k.slice(-40), JSON.stringify(n).slice(0, 160), '|', JSON.stringify(e).slice(0, 160));
      }
    }
    const ok = differ === 0 && missing === 0 && same > 0;
    if (!ok) failed++;
    console.log(`${ok ? 'ok  ' : 'FAIL'} ${name}: ${same} positions the same${differ ? `, ${differ} differ` : ''}${missing ? `, ${missing} not in the native set` : ''} (engine ${web.meta.engine}, ${web.meta.nodes} nodes)`);
  }
} finally {
  await s.stop();
}
for (const e of s.errors) console.log('console:', e);
console.log(failed ? `${failed} failed` : 'all the same');
process.exit(failed ? 1 : 0);
