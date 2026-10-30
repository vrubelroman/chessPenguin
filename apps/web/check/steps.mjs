// Runs a list of steps in the web version in headless Firefox and keeps screenshots —
// for finding the places of buttons, and for the scenario checks built on it.
//
//   node apps/web/check/steps.mjs <app-dir> <out-dir> <steps.json>
//
// A step: {"url": "?lang=en"} | {"wait": ms} | {"click": [x, y], "double": true}
// | {"key": "l", "mods": ["ctrl"]} | {"shot": "name.png"} | {"eval": "js expression"}
// | {"until": "js expression", "ms": 60000}
import { spawn } from 'node:child_process';
import { mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { launch } from './firefox.mjs';
import { sleep, tab } from './bidi.mjs';

export const KEYS = { ctrl: '', shift: '', alt: '', esc: '', right: '', left: '', enter: '' };

export async function session(appDir, port = Number(process.env.CP_CHECK_PORT || 18767)) {
  const root = fileURLToPath(new URL('../../..', import.meta.url));
  const server = spawn('python3', ['-I', join(root, 'apps/web/check/serve.py'), resolve(appDir), join(root, 'website/static'), String(port)], { stdio: 'ignore' });
  const ff = await launch(port + 457);
  const errors = [];
  const t = await tab(ff.browser, 1400, 860, (line) => {
    if (/panicked|RuntimeError|\[error\]/.test(line)) errors.push(line);
    if (process.env.CONSOLE) console.log(line);
  });
  const base = `http://127.0.0.1:${port}/app/`;
  const run = async (step, outDir) => {
    if (step.url !== undefined) {
      await t.navigate(base + step.url);
      await t.waitFor(`document.getElementById('cp-status').hidden || !document.getElementById('cp-device').hidden`, 30000);
    }
    if (step.wait) await sleep(step.wait);
    if (step.click) {
      const [x, y] = step.click;
      const acts = [{ type: 'pointerMove', x, y }];
      for (let i = 0; i < (step.double ? 2 : 1); i++) acts.push({ type: 'pointerDown', button: step.right ? 2 : 0 }, { type: 'pointerUp', button: step.right ? 2 : 0 });
      await t.actions([{ type: 'pointer', id: 'mouse', parameters: { pointerType: 'mouse' }, actions: acts }]);
      await sleep(300);
    }
    if (step.key) {
      const mods = (step.mods || []).map((m) => KEYS[m]);
      const v = KEYS[step.key] || step.key;
      const acts = [...mods.map((m) => ({ type: 'keyDown', value: m })), { type: 'keyDown', value: v }, { type: 'keyUp', value: v }, ...mods.reverse().map((m) => ({ type: 'keyUp', value: m }))];
      for (let i = 0; i < (step.times || 1); i++) await t.actions([{ type: 'key', id: 'kb', actions: acts }]);
      await sleep(300);
    }
    if (step.eval) console.log('eval:', JSON.stringify(await t.evaluate(step.eval)).slice(0, 2000));
    if (step.until) await t.waitFor(step.until, step.ms || 60000);
    if (step.shot) writeFileSync(join(outDir, step.shot), await t.screenshot());
  };
  return {
    t, ff, errors, run,
    stop: async () => { await ff.stop(); server.kill(); },
  };
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  const [appDir, outDir, stepsFile] = process.argv.slice(2);
  mkdirSync(outDir, { recursive: true });
  const steps = JSON.parse(readFileSync(stepsFile, 'utf8'));
  const s = await session(appDir);
  try {
    for (const step of steps) await s.run(step, outDir);
  } finally {
    await s.stop();
  }
  for (const e of s.errors) console.log('console:', e);
}
