// One way to drive a headless browser for the web checks: Firefox over WebDriver BiDi
// (firefox.mjs, bidi.mjs) or Chromium / Chrome for Testing over the DevTools protocol.
// The browser: CP_BROWSER=firefox (default) | chromium, the Chromium binary in
// CP_CHROME. No window is ever shown; the browser is stopped by its PID.
//
//   const b = await open(port); const t = await b.tab(1400, 860, log);
//   t.navigate(url) · t.evaluate(js) · t.waitFor(js, ms) · t.screenshot()
//   t.click(x, y, {double, right}) · t.key(key, mods, times) · t.setFiles(inputId, paths)
//   t.choose(paths, open): the user chooses the files in the dialog open() brings up
//   t.beforeLoad(js): js runs in every page loaded later, before its scripts
import { spawn } from 'node:child_process';
import { mkdirSync, mkdtempSync, readFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { basename, join } from 'node:path';
import { launch as launchFirefox } from './firefox.mjs';
import { sleep, tab as bidiTab } from './bidi.mjs';

// WebDriver key values (BiDi) and their DevTools descriptions.
const KEYS = {
  ctrl: { bidi: '', key: 'Control', code: 'ControlLeft', vk: 17, mod: 2 },
  shift: { bidi: '', key: 'Shift', code: 'ShiftLeft', vk: 16, mod: 8 },
  alt: { bidi: '', key: 'Alt', code: 'AltLeft', vk: 18, mod: 1 },
  esc: { bidi: '', key: 'Escape', code: 'Escape', vk: 27 },
  enter: { bidi: '', key: 'Enter', code: 'Enter', vk: 13, text: '\r' },
  right: { bidi: '', key: 'ArrowRight', code: 'ArrowRight', vk: 39 },
  left: { bidi: '', key: 'ArrowLeft', code: 'ArrowLeft', vk: 37 },
};

function keyOf(k) {
  if (KEYS[k]) return KEYS[k];
  if (k === ',') return { bidi: ',', key: ',', code: 'Comma', vk: 188, text: ',' };
  if (/^[0-9]$/.test(k)) return { bidi: k, key: k, code: `Digit${k}`, vk: 48 + Number(k), text: k };
  return { bidi: k, key: k, code: `Key${k.toUpperCase()}`, vk: k.toUpperCase().charCodeAt(0), text: k };
}

const waitFor = (evaluate) => async (expression, ms = 10000) => {
  const end = Date.now() + ms;
  for (;;) {
    const v = await evaluate(expression).catch(() => undefined);
    if (v) return v;
    if (Date.now() > end) throw new Error(`timed out waiting for ${expression}`);
    await sleep(100);
  }
};

// ----- Firefox --------------------------------------------------------------------------

async function firefox(port) {
  const ff = await launchFirefox(port);
  return {
    name: 'firefox',
    profile: ff.profile,
    downloads: ff.downloads,
    stop: ff.stop,
    tab: async (width, height, log) => {
      const t = await bidiTab(ff.browser, width, height, log);
      const evaluate = async (expression) => {
        const r = await ff.browser.send('script.evaluate', {
          expression: `Promise.resolve(${expression}).then((v) => JSON.stringify(v === undefined ? null : v))`,
          target: { context: t.context },
          awaitPromise: true,
        });
        if (r.type === 'exception') throw new Error(r.exceptionDetails?.text || 'script failed');
        return JSON.parse(r.result.value);
      };
      return {
        evaluate,
        waitFor: waitFor(evaluate),
        navigate: t.navigate,
        screenshot: t.screenshot,
        // `js` runs in every page loaded from now on, before the page's own scripts.
        beforeLoad: (js) => ff.browser.send('script.addPreloadScript', { functionDeclaration: `() => { ${js} }`, contexts: [t.context] }),
        click: async (x, y, { double = false, right = false } = {}) => {
          const acts = [{ type: 'pointerMove', x, y }];
          for (let i = 0; i < (double ? 2 : 1); i++) {
            acts.push({ type: 'pointerDown', button: right ? 2 : 0 }, { type: 'pointerUp', button: right ? 2 : 0 });
          }
          await t.actions([{ type: 'pointer', id: 'mouse', parameters: { pointerType: 'mouse' }, actions: acts }]);
        },
        key: async (key, mods = [], times = 1) => {
          const m = mods.map((x) => KEYS[x].bidi);
          const v = keyOf(key).bidi;
          const acts = [...m.map((value) => ({ type: 'keyDown', value })), { type: 'keyDown', value: v }, { type: 'keyUp', value: v }, ...[...m].reverse().map((value) => ({ type: 'keyUp', value }))];
          for (let i = 0; i < times; i++) await t.actions([{ type: 'key', id: 'kb', actions: acts }]);
        },
        setFiles: async (id, files) => {
          const node = await ff.browser.send('script.evaluate', {
            expression: `document.getElementById(${JSON.stringify(id)})`, target: { context: t.context }, awaitPromise: false,
          });
          await ff.browser.send('input.setFiles', { context: t.context, element: { sharedId: node.result.sharedId }, files });
        },
        // The user chooses `files` in the dialog `open()` brings up (the page's file input).
        async choose(files, open) {
          await open();
          await this.waitFor(`!!document.getElementById('cp-open-files')`, 5000);
          await this.setFiles('cp-open-files', files);
        },
      };
    },
  };
}

// ----- Chromium (DevTools protocol) -----------------------------------------------------------

async function chromium(port) {
  const binary = process.env.CP_CHROME;
  if (!binary) throw new Error('CP_CHROME: the Chromium / Chrome for Testing binary');
  const profile = mkdtempSync(join(process.env.TMPDIR || tmpdir(), 'cp-chrome-'));
  const downloads = join(profile, 'downloads');
  mkdirSync(downloads, { recursive: true });
  const proc = spawn(binary, [
    '--headless=new', `--remote-debugging-port=${port}`, `--user-data-dir=${profile}`,
    '--no-first-run', '--no-default-browser-check', '--disable-gpu-sandbox', '--use-angle=swiftshader',
    '--enable-unsafe-swiftshader', 'about:blank',
  ], { stdio: process.env.CP_CHROME_LOG ? 'inherit' : 'ignore' });
  let ws;
  for (let i = 0; i < 300 && !ws; i++) {
    try {
      const v = await (await fetch(`http://127.0.0.1:${port}/json/version`)).json();
      ws = v.webSocketDebuggerUrl;
    } catch {
      await sleep(100);
    }
  }
  if (!ws) throw new Error('Chromium did not start');
  const sock = new WebSocket(ws);
  await new Promise((res) => (sock.onopen = res));
  let id = 0;
  const pending = new Map();
  const listeners = [];
  sock.onmessage = (m) => {
    const d = JSON.parse(m.data);
    if (d.id && pending.has(d.id)) {
      const p = pending.get(d.id);
      pending.delete(d.id);
      d.error ? p.rej(new Error(d.error.message)) : p.res(d.result);
    } else if (d.method) for (const l of listeners) l(d);
  };
  const send = (method, params = {}, sessionId) =>
    new Promise((res, rej) => {
      const i = ++id;
      pending.set(i, { res, rej });
      sock.send(JSON.stringify({ id: i, method, params, sessionId }));
    });
  await send('Browser.setDownloadBehavior', { behavior: 'allow', downloadPath: downloads });
  return {
    name: 'chromium',
    profile,
    downloads,
    send,
    on: (f) => listeners.push(f),
    stop: async () => {
      try { sock.close(); } catch {}
      try { process.kill(proc.pid); } catch {}
      await sleep(500);
      rmSync(profile, { recursive: true, force: true });
    },
    tab: async (width, height, log = console.log) => {
      const { targetId } = await send('Target.createTarget', { url: 'about:blank' });
      const { sessionId } = await send('Target.attachToTarget', { targetId, flatten: true });
      const s = (method, params) => send(method, params, sessionId);
      listeners.push((d) => {
        if (d.sessionId !== sessionId) return;
        if (d.method === 'Runtime.consoleAPICalled') {
          log(`[${d.params.type}] ${d.params.args.map((a) => a.value ?? a.description ?? '').join(' ')}`);
        } else if (d.method === 'Runtime.exceptionThrown') {
          log(`[error] ${d.params.exceptionDetails.exception?.description || d.params.exceptionDetails.text}`);
        } else if (d.method === 'Log.entryAdded' && d.params.entry.level === 'error') {
          log(`[error] ${d.params.entry.text}`);
        }
      });
      await s('Page.enable');
      await s('Runtime.enable');
      await s('Log.enable');
      await s('Emulation.setDeviceMetricsOverride', { width, height, deviceScaleFactor: 1, mobile: false });
      const evaluate = async (expression) => {
        const r = await s('Runtime.evaluate', { expression: `Promise.resolve(${expression}).then((v) => JSON.stringify(v === undefined ? null : v))`, awaitPromise: true, returnByValue: true });
        if (r.exceptionDetails) throw new Error(r.exceptionDetails.exception?.description || r.exceptionDetails.text);
        return JSON.parse(r.result.value);
      };
      let modifiers = 0;
      return {
        sessionId,
        send: s,
        evaluate,
        waitFor: waitFor(evaluate),
        beforeLoad: (js) => s('Page.addScriptToEvaluateOnNewDocument', { source: js }),
        navigate: async (url) => {
          await s('Page.navigate', { url });
          await waitFor(evaluate)(`document.readyState === 'complete'`, 30000);
        },
        screenshot: async () => Buffer.from((await s('Page.captureScreenshot', { format: 'png' })).data, 'base64'),
        click: async (x, y, { double = false, right = false } = {}) => {
          const button = right ? 'right' : 'left';
          await s('Input.dispatchMouseEvent', { type: 'mouseMoved', x, y });
          for (let i = 1; i <= (double ? 2 : 1); i++) {
            await s('Input.dispatchMouseEvent', { type: 'mousePressed', x, y, button, clickCount: i });
            await s('Input.dispatchMouseEvent', { type: 'mouseReleased', x, y, button, clickCount: i });
          }
        },
        key: async (key, mods = [], times = 1) => {
          for (let n = 0; n < times; n++) {
            for (const m of mods) {
              modifiers |= KEYS[m].mod;
              await s('Input.dispatchKeyEvent', { type: 'rawKeyDown', key: KEYS[m].key, code: KEYS[m].code, windowsVirtualKeyCode: KEYS[m].vk, modifiers });
            }
            const k = keyOf(key);
            const text = modifiers & 3 ? undefined : k.text;
            await s('Input.dispatchKeyEvent', { type: text ? 'keyDown' : 'rawKeyDown', key: k.key, code: k.code, windowsVirtualKeyCode: k.vk, modifiers, text });
            await s('Input.dispatchKeyEvent', { type: 'keyUp', key: k.key, code: k.code, windowsVirtualKeyCode: k.vk, modifiers });
            for (const m of [...mods].reverse()) {
              modifiers &= ~KEYS[m].mod;
              await s('Input.dispatchKeyEvent', { type: 'keyUp', key: KEYS[m].key, code: KEYS[m].code, windowsVirtualKeyCode: KEYS[m].vk, modifiers });
            }
          }
        },
        setFiles: async (id, files) => {
          const { result } = await s('Runtime.evaluate', { expression: `document.getElementById(${JSON.stringify(id)})` });
          await s('DOM.setFileInputFiles', { objectId: result.objectId, files });
        },
        // The user chooses `files` in the dialog `open()` brings up. Chrome's own dialog
        // (showOpenFilePicker, the files remembered) cannot be driven headless: the files
        // are put into the site's private file system and its handles are what the
        // dialog gives — the same kind of handle, kept and read again the same way.
        async choose(files, open) {
          const picker = await this.evaluate(`typeof window.showOpenFilePicker === 'function'`);
          if (!picker) {
            await open();
            await this.waitFor(`!!document.getElementById('cp-open-files')`, 5000);
            return this.setFiles('cp-open-files', files);
          }
          const given = files.map((f) => ({ name: basename(f), b64: readFileSync(f).toString('base64') }));
          await this.evaluate(`(() => {
            const given = ${JSON.stringify(given)};
            window.showOpenFilePicker = async () => {
              const root = await navigator.storage.getDirectory();
              const handles = [];
              for (const g of given) {
                const h = await root.getFileHandle(g.name, { create: true });
                const w = await h.createWritable();
                await w.write(Uint8Array.from(atob(g.b64), (c) => c.charCodeAt(0)));
                await w.close();
                handles.push(h);
              }
              return handles;
            };
            return true;
          })()`);
          await open();
        },
      };
    },
  };
}

export async function open(port, kind = process.env.CP_BROWSER || 'firefox') {
  return kind === 'chromium' ? chromium(port) : firefox(port);
}
