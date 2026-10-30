// A screenshot of a page of the web version in headless Firefox, with its console:
//
//   node apps/web/check/shot.mjs <url> <out.png> [wait-ms] [width] [height]
import { writeFileSync } from 'node:fs';
import { launch } from './firefox.mjs';
import { sleep, tab } from './bidi.mjs';

const [url, out, wait = '6000', width = '1400', height = '860'] = process.argv.slice(2);
const ff = await launch();
try {
  const t = await tab(ff.browser, Number(width), Number(height));
  await t.navigate(url);
  await sleep(Number(wait));
  writeFileSync(out, await t.screenshot());
  console.log('saved', out);
} finally {
  await ff.stop();
}
