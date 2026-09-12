// Live desktop regression check. Requires Playwright, Chromium, Python 3 and
// xprop. Uses an isolated XWayland browser on the current Linux desktop.
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const {spawn, execFileSync} = require('node:child_process');
const readline = require('node:readline');

if (process.argv.includes('--help')) {
  console.log('Usage: node scripts/check-linux-insertion.cjs [rounds=100]\n' +
    'Build first: cargo build --manifest-path src-tauri/Cargo.toml --example clipboard_probe\n' +
    'Requires a local Playwright installation and Chromium (npx playwright install chromium).\n' +
    'Optional VOICE_FLOW_CHROMIUM: Chromium executable path.\n' +
    'Opens a disposable text field and emits real paste shortcuts; never sends Enter.\n' +
    'Temporarily uses the desktop clipboard. Keep the test window active.\n' +
    'Reports metadata only; does not run ASR or access Voice Flow settings.');
  process.exit(0);
}

const {chromium} = require('playwright');
const root = path.resolve(__dirname, '..');
const executable = path.join(root, 'src-tauri/target/debug/examples/clipboard_probe');
const title = 'Voice Flow insertion verification';

function readClipboard() {
  const result = JSON.parse(execFileSync(executable, ['--clipboard-read-helper'], {
    timeout: 2000, encoding: 'utf8', maxBuffer: 16 * 1024 * 1024,
  }));
  if ('Err' in result) throw new Error(result.Err);
  return result.Ok;
}

function activeWindow() {
  return execFileSync('xprop', ['-root', '_NET_ACTIVE_WINDOW'], {encoding: 'utf8'})
    .match(/0x[0-9a-f]+/i)?.[0];
}

async function waitFor(check, message, timeout = 3000) {
  const started = Date.now();
  while (!await check()) {
    if (Date.now() - started >= timeout) throw new Error(message);
    await new Promise(resolve => setTimeout(resolve, 25));
  }
}

async function main() {
  const rounds = Number(process.argv[2] || 100);
  if (!Number.isInteger(rounds) || rounds < 1) throw new Error('rounds must be a positive integer');
  const original = readClipboard();
  const profile = fs.mkdtempSync(path.join(os.tmpdir(), 'voice-flow-insertion-check-'));
  const chrome = spawn(process.env.VOICE_FLOW_CHROMIUM || chromium.executablePath(), [
    `--user-data-dir=${profile}`, '--remote-debugging-port=0', '--ozone-platform=x11',
    '--no-first-run', '--no-default-browser-check', '--disable-background-networking', 'about:blank',
  ], {stdio: 'ignore', env: {...process.env, XDG_ACTIVATION_TOKEN: '', DESKTOP_STARTUP_ID: ''}});
  let browser;
  let probe;
  try {
    await waitFor(() => fs.existsSync(path.join(profile, 'DevToolsActivePort')), 'Chromium did not start');
    const port = fs.readFileSync(path.join(profile, 'DevToolsActivePort'), 'utf8').split('\n')[0];
    browser = await chromium.connectOverCDP(`http://127.0.0.1:${port}`, {noDefaults: true});
    const page = browser.contexts()[0].pages()[0];
    await page.setContent(`<title>${title}</title><h2>Voice Flow local insertion test</h2>
      <p>Disposable field. Nothing is sent.</p>
      <textarea id="target" autofocus style="width:90%;height:300px;font-size:22px"></textarea>`);
    await page.bringToFront();
    await page.waitForTimeout(500);
    let windowId;
    await waitFor(() => {
      const ids = execFileSync('xprop', ['-root', '_NET_CLIENT_LIST'], {encoding: 'utf8'})
        .match(/0x[0-9a-f]+/gi) || [];
      windowId = ids.find(id => {
        const properties = execFileSync('xprop', ['-id', id, '_NET_WM_NAME', '_NET_WM_PID'], {encoding: 'utf8'});
        return properties.includes(title) && Number(properties.match(/_NET_WM_PID\(CARDINAL\) = (\d+)/)?.[1]) === chrome.pid;
      });
      return windowId;
    }, 'Disposable test window not found');
    execFileSync('python3', [path.join(__dirname, 'focus-insertion-test-window.py'), windowId]);
    await waitFor(() => activeWindow() === windowId, 'Test window did not acquire desktop focus');
    await page.locator('#target').click();
    await page.evaluate(() => {
      window.insertionEvents = {keys: 0, pastes: 0};
      window.addEventListener('keydown', () => window.insertionEvents.keys++);
      window.addEventListener('paste', () => window.insertionEvents.pastes++);
    });
    probe = spawn(executable, [], {stdio: ['pipe', 'pipe', 'inherit']});
    const lines = readline.createInterface({input: probe.stdout})[Symbol.asyncIterator]();
    let failures = 0;
    let missing = 0;
    let restoreFailures = 0;
    for (let index = 0; index < rounds; index++) {
      await waitFor(() => activeWindow() === windowId, 'Test window lost desktop focus; stopping before paste');
      const cases = [
        `Voice Flow check ${index} 中文测试。`,
        `多行输入 ${index}\n第二行 mixed Rust/Tauri 2.`,
        `标点与 Unicode：你好，世界！ 🌸 ${index}`,
        `长文本 ${index} ${'streaming 语音输入 '.repeat(100)}`,
      ];
      const text = cases[index % cases.length];
      await page.locator('#target').fill('');
      await page.locator('#target').click();
      await page.evaluate(() => { window.insertionEvents = {keys: 0, pastes: 0}; });
      probe.stdin.write(`${JSON.stringify(text)}\n`);
      let timer;
      const line = await Promise.race([
        lines.next(),
        new Promise((_, reject) => { timer = setTimeout(() => reject(new Error('Probe response timed out')), 10000); }),
      ]).finally(() => clearTimeout(timer));
      if (line.done) throw new Error('Insertion probe exited unexpectedly');
      const report = JSON.parse(line.value);
      const actual = await page.locator('#target').inputValue();
      const delivered = actual === text;
      const restored = readClipboard() === original;
      if (report.error) failures++;
      if (!delivered) missing++;
      if (!restored) restoreFailures++;
      if (report.error || !delivered || !restored) console.log(JSON.stringify({index, ...report, delivered, restored, actualChars: actual.length, actualIsOriginal: actual === original, ...await page.evaluate(() => window.insertionEvents)}));
    }
    console.log(JSON.stringify({rounds, failures, missing, restoreFailures, clipboardRestored: readClipboard() === original}));
    if (failures || missing || restoreFailures) process.exitCode = 1;
  } finally {
    if (probe) {
      probe.stdin.end();
      if (probe.exitCode === null) {
        const timer = setTimeout(() => probe.kill('SIGTERM'), 5000);
        await new Promise(resolve => probe.once('exit', resolve));
        clearTimeout(timer);
      }
    }
    if (browser) await browser.close();
    if (chrome.exitCode === null) {
      chrome.kill('SIGTERM');
      await new Promise(resolve => chrome.once('exit', resolve));
    }
    fs.rmSync(profile, {recursive: true, force: true, maxRetries: 5, retryDelay: 100});
  }
}

main().catch(error => { console.error(error.message); process.exitCode = 1; });
