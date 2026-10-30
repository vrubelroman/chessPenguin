// The tests of uci's worker transport (crates/uci/tests/worker.rs) in headless
// Firefox. wasm-bindgen-test-runner serves them (NO_HEADLESS: it does not look for a
// WebDriver) and this script opens its page and reads the outcome.
//
//   node apps/web/check/uci-worker.mjs
//
// Needs wasm-bindgen-test-runner of the wasm-bindgen version in Cargo.lock in PATH
// or in WASM_BINDGEN_TEST_RUNNER.
import { spawn } from 'node:child_process';
import { launch } from './firefox.mjs';
import { sleep, tab } from './bidi.mjs';

const runner = process.env.WASM_BINDGEN_TEST_RUNNER || 'wasm-bindgen-test-runner';
const address = '127.0.0.1:18731';
const cargo = spawn(
  'cargo',
  ['test', '-p', 'uci', '--target', 'wasm32-unknown-unknown', '--test', 'worker'],
  {
    env: {
      ...process.env,
      CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_RUNNER: runner,
      NO_HEADLESS: '1',
      WASM_BINDGEN_TEST_ADDRESS: address,
    },
    stdio: ['ignore', 'pipe', 'inherit'],
    // Its own process group: the runner it starts is stopped with it.
    detached: true,
  },
);
const stopCargo = () => {
  try {
    process.kill(-cargo.pid);
  } catch {}
};
let served = false;
cargo.stdout.on('data', (d) => {
  if (String(d).includes(address)) served = true;
});
for (let i = 0; i < 1200 && !served; i++) await sleep(100);
if (!served) {
  console.error('the test runner did not start');
  stopCargo();
  process.exit(1);
}
const ff = await launch();
let failed = true;
try {
  const t = await tab(ff.browser, 1000, 800, () => {});
  await t.navigate(`http://${address}/`);
  const text = await t.waitFor(
    `(() => { const t = document.body.innerText; return /test result: /.test(t) && t; })()`,
    60000,
  );
  console.log(text.trim());
  failed = !/test result: ok\./.test(text);
} finally {
  await ff.stop();
  stopCargo();
}
process.exit(failed ? 1 : 0);
