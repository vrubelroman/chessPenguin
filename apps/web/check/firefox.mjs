// Starts a headless Firefox with a temporary profile and WebDriver BiDi; stops it by
// its PID. No window is ever shown (MOZ_HEADLESS).
import { spawn } from 'node:child_process';
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { connect, sleep } from './bidi.mjs';

export async function launch(port = 19222) {
  const profile = mkdtempSync(join(process.env.TMPDIR || tmpdir(), 'cp-firefox-'));
  writeFileSync(
    join(profile, 'user.js'),
    [
      'user_pref("remote.active-protocols", 1);',
      'user_pref("browser.shell.checkDefaultBrowser", false);',
      'user_pref("datareporting.policy.dataSubmissionEnabled", false);',
      'user_pref("toolkit.telemetry.reportingpolicy.firstRun", false);',
      // Downloads go to the profile without asking.
      'user_pref("browser.download.folderList", 2);',
      `user_pref("browser.download.dir", "${join(profile, 'downloads')}");`,
      'user_pref("browser.download.useDownloadDir", true);',
      'user_pref("browser.download.always_ask_before_handling_new_types", false);',
      '',
    ].join('\n'),
  );
  const log = join(profile, 'firefox.log');
  const out = (await import('node:fs')).openSync(log, 'a');
  const ff = spawn(
    'firefox',
    ['--headless', '--no-remote', '--profile', profile, '--remote-debugging-port', String(port), 'about:blank'],
    { env: { ...process.env, MOZ_HEADLESS: '1' }, stdio: ['ignore', out, out] },
  );
  for (let i = 0; i < 100; i++) {
    if (readFileSync(log, 'utf8').includes('WebDriver BiDi listening')) break;
    await sleep(100);
  }
  const browser = await connect(port);
  return {
    browser,
    profile,
    downloads: join(profile, 'downloads'),
    stop: async () => {
      await browser.close().catch(() => {});
      try {
        process.kill(ff.pid);
      } catch {}
      await sleep(300);
      rmSync(profile, { recursive: true, force: true });
    },
  };
}
