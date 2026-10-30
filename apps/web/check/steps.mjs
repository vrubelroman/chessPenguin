// A session of the web version in a headless browser (browser.mjs: Firefox, or
// Chromium with CP_BROWSER=chromium) over the site's layout (serve.py), and a runner of
// step lists — for finding the places of buttons, and for the scenario checks.
//
//   node apps/web/check/steps.mjs <app-dir> <out-dir> <steps.json>
//
// A step: {"url": "?lang=en"} | {"wait": ms} | {"click": [x, y], "double": true}
// | {"key": "l", "mods": ["ctrl"], "times": 3} | {"shot": "name.png"} | {"eval": "js"}
// | {"until": "js expression", "ms": 60000}
import { spawn } from 'node:child_process';
import { mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { open } from './browser.mjs';
import { sleep } from './bidi.mjs';

export async function session(appDir, port = Number(process.env.CP_CHECK_PORT || 18767)) {
  const root = fileURLToPath(new URL('../../..', import.meta.url));
  const server = spawn('python3', ['-I', join(root, 'apps/web/check/serve.py'), resolve(appDir), join(root, 'website/static'), String(port)], { stdio: 'ignore' });
  const b = await open(port + 457);
  const errors = [];
  const t = await b.tab(1400, 860, (line) => {
    if (/panicked|RuntimeError|\[error\]/.test(line) && !/favicon|404/.test(line)) errors.push(line);
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
      await t.click(step.click[0], step.click[1], step);
      await sleep(300);
    }
    if (step.key) {
      await t.key(step.key, step.mods || [], step.times || 1);
      await sleep(300);
    }
    if (step.eval) console.log('eval:', JSON.stringify(await t.evaluate(step.eval)).slice(0, 2000));
    if (step.until) await t.waitFor(step.until, step.ms || 60000);
    if (step.shot) writeFileSync(join(outDir, step.shot), await t.screenshot());
  };
  return {
    t, b, base, errors, run,
    stop: async () => { await b.stop(); server.kill(); },
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
