// Which devices get the program and which the page about the apps (page/device.js):
//
//   node apps/web/check/device.test.mjs
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import vm from 'node:vm';

const source = readFileSync(fileURLToPath(new URL('../page/device.js', import.meta.url)), 'utf8');

// device.js in a window that looks like `d`.
function classOf(d) {
  const media = { '(pointer: coarse)': d.coarse, '(any-pointer: fine)': d.fine };
  const window = {
    navigator: { userAgent: d.ua, platform: d.platform || '', maxTouchPoints: d.touch || 0, userAgentData: d.uad },
    screen: { width: d.screen[0], height: d.screen[1] },
    matchMedia: (q) => ({ matches: !!media[q] }),
  };
  window.window = window;
  vm.runInNewContext(source, window);
  return window.CPDevice.deviceClass();
}

const MAC = 'Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.0 Safari/605.1.15';
const cases = [
  ['Windows laptop', { ua: 'Mozilla/5.0 (Windows NT 10.0; Win64; x64) Firefox/159.0', screen: [1920, 1080], fine: true }, 'computer'],
  ['Mac', { ua: MAC, platform: 'MacIntel', screen: [1512, 982], fine: true }, 'computer'],
  ['laptop with a touch screen', { ua: 'Mozilla/5.0 (X11; Linux x86_64) Chrome/140.0', screen: [1366, 768], touch: 10, coarse: false, fine: true }, 'computer'],
  ['Chromebook', { ua: 'Mozilla/5.0 (X11; CrOS x86_64 15000.0.0) Chrome/140.0', screen: [1366, 768], touch: 5, coarse: true }, 'computer'],
  ['iPhone', { ua: 'Mozilla/5.0 (iPhone; CPU iPhone OS 18_0 like Mac OS X) Mobile/15E148', screen: [390, 844], touch: 5, coarse: true }, 'iphone'],
  ['iPhone asking for the desktop site', { ua: MAC, platform: 'MacIntel', screen: [390, 844], touch: 5, coarse: true }, 'iphone'],
  ['iPad (says it is a Mac)', { ua: MAC, platform: 'MacIntel', screen: [820, 1180], touch: 5, coarse: true }, 'ipad'],
  ['iPad mini', { ua: MAC, platform: 'MacIntel', screen: [744, 1133], touch: 5, coarse: true }, 'ipad'],
  ['Android phone', { ua: 'Mozilla/5.0 (Linux; Android 14; Pixel 8) Chrome/140.0 Mobile', screen: [412, 915], touch: 5, coarse: true }, 'android-phone'],
  ['Android tablet', { ua: 'Mozilla/5.0 (Linux; Android 14; SM-X710) Chrome/140.0', screen: [800, 1280], touch: 10, coarse: true }, 'android-tablet'],
  ['Galaxy Fold folded', { ua: 'Mozilla/5.0 (Linux; Android 14; SM-F946B) Chrome/140.0 Mobile', screen: [344, 882], touch: 5, coarse: true }, 'android-phone'],
  ['tablet asking for the desktop site', { ua: 'Mozilla/5.0 (X11; Linux x86_64) Chrome/140.0', screen: [800, 1280], touch: 10, coarse: true, fine: false }, 'tablet'],
  ['phone asking for the desktop site', { ua: 'Mozilla/5.0 (X11; Linux x86_64) Chrome/140.0', screen: [412, 915], touch: 5, coarse: true, fine: false }, 'phone'],
  ['a mobile Chromium by its client hints', { ua: 'Mozilla/5.0 (X11; Linux x86_64) Chrome/140.0', uad: { mobile: true, platform: 'Linux' }, screen: [412, 915], touch: 5, coarse: true }, 'phone'],
];

let failed = 0;
for (const [name, device, want] of cases) {
  const got = classOf(device);
  const ok = got === want;
  if (!ok) failed++;
  console.log(`${ok ? 'ok  ' : 'FAIL'} ${name}: ${got}${ok ? '' : ` (want ${want})`}`);
}
console.log(failed ? `${failed} of ${cases.length} failed` : `all ${cases.length} ok`);
process.exit(failed ? 1 : 0);
