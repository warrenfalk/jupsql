import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { mkdir, readFile, writeFile } from 'node:fs/promises';
import { resolve } from 'node:path';

const port = Number(process.env.JUPSQL_DRIVER_PORT ?? 4447);
const base = `http://127.0.0.1:${port}`;
await mkdir(`${process.env.JUPSQL_DATA_DIR}/runtime`, { recursive: true, mode: 0o700 });
const driver = spawn('WebKitWebDriver', [`--port=${port}`, '--host=127.0.0.1'], {
  env: {
    ...process.env,
    XDG_CONFIG_HOME: `${process.env.JUPSQL_DATA_DIR}/config`,
    XDG_CACHE_HOME: `${process.env.JUPSQL_DATA_DIR}/cache`,
    XDG_DATA_HOME: `${process.env.JUPSQL_DATA_DIR}/share`,
    XDG_RUNTIME_DIR: `${process.env.JUPSQL_DATA_DIR}/runtime`,
    GIO_USE_VFS: 'local',
    NO_AT_BRIDGE: '1',
  },
  stdio: ['ignore', 'inherit', 'inherit'],
});
let session;

async function request(path, method = 'GET', body) {
  const response = await fetch(`${base}${path}`, {
    method,
    headers: { 'Content-Type': 'application/json' },
    body: body === undefined ? undefined : JSON.stringify(body),
    signal: AbortSignal.timeout(35_000),
  });
  const { value } = await response.json();
  if (value?.error) throw new Error(`${value.error}: ${value.message}`);
  return value;
}

const evaluate = (script, args = []) => request(`/session/${session}/execute/sync`, 'POST', { script, args });
const sleep = (ms) => new Promise((done) => setTimeout(done, ms));
async function until(script) {
  for (let i = 0; i < 100; i += 1) {
    if (await evaluate(script)) return;
    await sleep(100);
  }
  throw new Error(`Timed out: ${script}\n${await evaluate('return document.body.innerText')}`);
}
const click = (text) =>
  evaluate(
    `const scope = document.querySelector('[role=dialog]') || document; const button = [...scope.querySelectorAll('button')].find(b => b.textContent.trim() === arguments[0]); if (!button || button.disabled) throw new Error('Button unavailable: ' + arguments[0]); button.click();`,
    [text],
  );
const invoke = (command, args) =>
  request(`/session/${session}/execute/async`, 'POST', {
    script: `const done = arguments[arguments.length - 1]; window.__TAURI_INTERNALS__.invoke(arguments[0], arguments[1]).then(value => done({value}), error => done({error:String(error)}));`,
    args: [command, args],
  });

try {
  for (let i = 0; i < 50; i += 1) {
    try {
      await request('/status');
      break;
    } catch {
      await sleep(100);
    }
  }
  const created = await request('/session', 'POST', {
    capabilities: { alwaysMatch: { 'webkitgtk:browserOptions': { binary: resolve('src-tauri/target/debug/jupsql') } } },
  });
  session = created.sessionId;
  await until('return document.querySelectorAll(".cell-card").length === 3');
  assert.ok(await evaluate('return innerWidth > 800 && innerHeight > 400'));
  await click('Connect a database');
  await until('return !!document.querySelector("[role=dialog]")');
  if (process.env.JUPSQL_TEST_PORT) {
    await evaluate(
      `const input = document.querySelector('input[type=number]'); Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set.call(input, arguments[0]); input.dispatchEvent(new Event('input', {bubbles:true}));`,
      [process.env.JUPSQL_TEST_PORT],
    );
  }
  await click('Connect');
  await until(
    'return !document.querySelector("[role=dialog]") && ![...document.querySelectorAll("button")].find(b => b.textContent.trim() === "Run all queries").disabled',
  );
  await click('Run all queries');
  await until('return document.querySelectorAll(".results").length === 3 && !document.querySelector(".run-progress")');
  const counts = await evaluate('return [...document.querySelectorAll(".results tbody")].map(t => t.rows.length)');
  assert.deepEqual(counts, [5, 3, 2]);
  assert.equal(
    await evaluate('return document.querySelectorAll(".results tbody")[2].rows[0].cells[3].textContent'),
    '360.00',
  );
  const snapshots = await evaluate(
    'return [...document.querySelectorAll(".result-bar time")].map(t => t.title.split("Snapshot ")[1])',
  );
  assert.equal(new Set(snapshots).size, 1);

  // Edit the upstream cell without running it, then run only the final cell.
  await evaluate(`const editor = document.querySelector('[aria-label="orders SQL"]'); editor.focus();`);
  const element = await request(`/session/${session}/element`, 'POST', {
    using: 'css selector',
    value: '[aria-label="orders SQL"]',
  });
  const id = element['element-6066-11e4-a52e-4f735466cecf'];
  await request(`/session/${session}/actions`, 'POST', {
    actions: [
      {
        type: 'key',
        id: 'keyboard',
        actions: [
          { type: 'keyDown', value: '\uE009' },
          { type: 'keyDown', value: 'a' },
          { type: 'keyUp', value: 'a' },
          { type: 'keyUp', value: '\uE009' },
        ],
      },
    ],
  });
  await request(`/session/${session}/element/${id}/value`, 'POST', {
    text: "SELECT * FROM (VALUES (101, 'Maya', 'paid', 999.00)) AS orders(id, customer, status, amount);",
  });
  await until('return document.querySelectorAll(".state-badge.stale").length === 3');
  await until('return !document.querySelectorAll(".run-button")[2].disabled');
  await evaluate('document.querySelectorAll(".run-button")[2].click()');
  await until(
    'return !document.querySelector(".run-progress") && document.querySelectorAll(".results tbody")[2].textContent.includes("999.00")',
  );
  assert.equal(await evaluate('return document.querySelectorAll(".state-badge.stale").length'), 2);

  // Renaming a referenced definition should expose an error, not query a real table.
  await evaluate(
    `const input = document.querySelector('[aria-label="Cell 1 name"]'); Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set.call(input, 'renamed'); input.dispatchEvent(new Event('input', {bubbles:true}));`,
  );
  await until('return document.querySelectorAll(".validation-error").length === 2');
  await evaluate(
    `const input = document.querySelector('[aria-label="Cell 1 name"]'); Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set.call(input, 'orders'); input.dispatchEvent(new Event('input', {bubbles:true}));`,
  );
  await until(
    'return document.querySelectorAll(".validation-error").length === 0 && !document.querySelectorAll(".run-button")[0].disabled',
  );
  await click('Rerun dependents');
  await until(
    'return !document.querySelector(".run-progress") && document.querySelectorAll(".state-badge.stale").length === 0',
  );

  // Exercise native IPC session logging and persistence, not a browser mock.
  await until('return document.querySelector(".save-status").textContent.includes("Saved locally")');
  const bootstrap = await invoke('bootstrap', {});
  assert.ok(bootstrap.value.notebook.cells[0].sql.includes('999.00'));
  const log = await invoke('read_session_log', {});
  const events = log.value.trim().split('\n').map(JSON.parse);
  assert.ok(events.some((event) => event.event === 'cell_started'));
  assert.ok(events.some((event) => event.event === 'ui.cell_focused'));
  assert.ok(events.some((event) => event.event === 'notebook_saved'));

  await mkdir('.cache/validation', { recursive: true });
  const screenshot = await request(`/session/${session}/screenshot`);
  await writeFile('.cache/validation/native-notebook.png', Buffer.from(screenshot, 'base64'));
  await writeFile('.cache/validation/native-session.jsonl', log.value);
  // Closing immediately after an edit must flush the draft before destroying the window.
  await evaluate(
    `const input = document.querySelector('[aria-label="Notebook title"]'); Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set.call(input, 'Close flush check'); input.dispatchEvent(new Event('input', {bubbles:true}));`,
  );
  await invoke('plugin:window|close', { label: 'main' });
  for (let i = 0; i < 50; i += 1) {
    const saved = JSON.parse(await readFile(`${process.env.JUPSQL_DATA_DIR}/notebook.json`, 'utf8'));
    if (saved.title === 'Close flush check') break;
    await sleep(100);
  }
  assert.equal(
    JSON.parse(await readFile(`${process.env.JUPSQL_DATA_DIR}/notebook.json`, 'utf8')).title,
    'Close flush check',
  );
  console.log(
    'Native Tauri smoke passed: connect, run, shared snapshot, edit/stale propagation, current SQL composition, rename validation, refresh, persistence, logs, and save on close.',
  );
} finally {
  if (session) await request(`/session/${session}`, 'DELETE').catch(() => {});
  driver.kill('SIGTERM');
}
