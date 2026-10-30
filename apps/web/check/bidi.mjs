// A minimal WebDriver BiDi client for headless Firefox (Node 22+: global WebSocket).
// Used by the web checks (docs/dev/web.md, «Проверки»); no windows are opened.

export async function connect(port) {
  const ws = new WebSocket(`ws://127.0.0.1:${port}/session`);
  let id = 0;
  const pending = new Map();
  const listeners = [];
  ws.onmessage = (m) => {
    const d = JSON.parse(m.data);
    if (d.id && pending.has(d.id)) {
      const p = pending.get(d.id);
      pending.delete(d.id);
      if (d.type === 'error') p.rej(new Error(`${d.error}: ${d.message}`));
      else p.res(d.result);
    } else if (d.method) {
      for (const l of listeners) l(d.method, d.params);
    }
  };
  await new Promise((res, rej) => {
    ws.onopen = res;
    ws.onerror = () => rej(new Error('cannot connect to Firefox'));
  });
  const send = (method, params) =>
    new Promise((res, rej) => {
      const i = ++id;
      pending.set(i, { res, rej });
      ws.send(JSON.stringify({ id: i, method, params }));
    });
  await send('session.new', { capabilities: {} });
  return {
    send,
    on: (f) => listeners.push(f),
    close: async () => {
      await send('session.end', {}).catch(() => {});
      ws.close();
    },
  };
}

export const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

// A tab of `width`×`height` CSS pixels with the console forwarded to `log`.
export async function tab(b, width, height, log = console.log) {
  await b.send('session.subscribe', { events: ['log.entryAdded'] });
  b.on((method, p) => {
    if (method === 'log.entryAdded') log(`[${p.level}] ${p.text}`);
  });
  const { context } = await b.send('browsingContext.create', { type: 'tab' });
  await b.send('browsingContext.setViewport', { context, viewport: { width, height } });
  const evaluate = async (expression) => {
    const r = await b.send('script.evaluate', {
      expression,
      target: { context },
      awaitPromise: true,
    });
    if (r.type === 'exception') throw new Error(r.exceptionDetails?.text || 'script failed');
    return r.result?.value;
  };
  return {
    context,
    evaluate,
    navigate: (url) => b.send('browsingContext.navigate', { context, url, wait: 'complete' }),
    screenshot: async () =>
      Buffer.from((await b.send('browsingContext.captureScreenshot', { context })).data, 'base64'),
    actions: (actions) => b.send('input.performActions', { context, actions }),
    // Waits until `expression` is truthy (polled), or throws after `ms`.
    waitFor: async (expression, ms = 10000) => {
      const end = Date.now() + ms;
      for (;;) {
        const v = await evaluate(expression).catch(() => undefined);
        if (v) return v;
        if (Date.now() > end) throw new Error(`timed out waiting for ${expression}`);
        await sleep(100);
      }
    },
  };
}
