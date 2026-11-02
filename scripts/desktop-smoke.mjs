import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { once } from 'node:events';
import { mkdir, readFile, writeFile } from 'node:fs/promises';
import { resolve } from 'node:path';

const port = Number(process.env.JUPSQL_DRIVER_PORT ?? 4447);
const base = `http://127.0.0.1:${port}`;
const connectionMode = process.env.JUPSQL_TEST_CONNECTION_MODE ?? 'tcp';
const connectionEnvironment = {
  ...process.env,
  DATABASE_URL: undefined,
  PGHOST: undefined,
  PGPORT: undefined,
  PGDATABASE: undefined,
  PGUSER: undefined,
  PGPASSWORD: undefined,
};
if (connectionMode === 'defaults') {
  connectionEnvironment.PGPORT = process.env.JUPSQL_TEST_PORT;
} else {
  Object.assign(connectionEnvironment, {
    PGHOST: '127.0.0.1',
    PGPORT: process.env.JUPSQL_TEST_PORT,
    PGDATABASE: 'jupsql_demo',
    PGUSER: 'jupsql',
    PGPASSWORD: 'environment-password-never-save-or-log',
  });
  if (connectionMode.startsWith('socket')) {
    connectionEnvironment.PGHOST = process.env.JUPSQL_TEST_HOST;
    connectionEnvironment.PGPORT = 'ignored-for-sockets';
    if (connectionMode === 'socket-file') {
      connectionEnvironment.PGHOST += `/.s.PGSQL.${process.env.JUPSQL_TEST_PORT}`;
    }
  } else if (connectionMode === 'invalid-port') {
    connectionEnvironment.PGPORT = 'invalid';
  } else if (connectionMode === 'rejected') {
    connectionEnvironment.PGDATABASE = 'nonexistent_database';
  } else if (connectionMode === 'url') {
    connectionEnvironment.DATABASE_URL = `postgresql://jupsql:url%40password-never-save-or-log@127.0.0.1:${process.env.JUPSQL_TEST_PORT}/jupsql_demo?sslmode=disable&application_name=url-client`;
    connectionEnvironment.PGHOST = 'ignored.example';
    connectionEnvironment.PGPORT = 'invalid';
    connectionEnvironment.PGUSER = 'ignored';
    connectionEnvironment.PGDATABASE = 'ignored';
  } else if (connectionMode === 'url-invalid') {
    connectionEnvironment.DATABASE_URL = 'postgres://jupsql:url-password-never-save-or-log@localhost:invalid/database';
  }
}
await mkdir(`${process.env.JUPSQL_DATA_DIR}/runtime`, { recursive: true, mode: 0o700 });
const driverEnvironment = {
  ...connectionEnvironment,
  XDG_CONFIG_HOME: `${process.env.JUPSQL_DATA_DIR}/config`,
  XDG_CACHE_HOME: `${process.env.JUPSQL_DATA_DIR}/cache`,
  XDG_DATA_HOME: `${process.env.JUPSQL_DATA_DIR}/share`,
  XDG_RUNTIME_DIR: `${process.env.JUPSQL_DATA_DIR}/runtime`,
  GIO_USE_VFS: 'local',
  NO_AT_BRIDGE: '1',
};
const driver = spawn('WebKitWebDriver', [`--port=${port}`, '--host=127.0.0.1'], {
  env: driverEnvironment,
  stdio: ['ignore', 'inherit', 'inherit'],
});
let session;
let notebookId;

function driverRequests(endpoint) {
  return async (path, method = 'GET', body) => {
    const response = await fetch(`${endpoint}${path}`, {
      method,
      headers: { 'Content-Type': 'application/json' },
      body: body === undefined ? undefined : JSON.stringify(body),
      signal: AbortSignal.timeout(35_000),
    });
    const { value } = await response.json();
    if (!response.ok) throw new Error(`${value?.error}: ${value?.message}`);
    return value;
  };
}
const request = driverRequests(base);

async function createSession(send) {
  const created = await send('/session', 'POST', {
    capabilities: {
      alwaysMatch: {
        'webkitgtk:browserOptions': {
          binary: resolve(process.env.JUPSQL_TEST_BINARY ?? 'src-tauri/target/debug/jupsql'),
        },
      },
    },
  });
  return created.sessionId;
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
const invoke = async (command, args) => {
  const result = await request(`/session/${session}/execute/async`, 'POST', {
    script: `const done = arguments[arguments.length - 1]; window.__TAURI_INTERNALS__.invoke(arguments[0], arguments[1]).then(value => done({value}), error => done({error:String(error)}));`,
    args: [command, { notebookId, ...args }],
  });
  if (command === 'bootstrap' && result.value) notebookId = result.value.notebookId;
  return result;
};
const setInput = (selector, value) =>
  evaluate(
    `const input = document.querySelector(arguments[0]); Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set.call(input, arguments[1]); input.dispatchEvent(new Event('input', {bubbles:true}));`,
    [selector, value],
  );

try {
  for (let i = 0; i < 50; i += 1) {
    try {
      await request('/status');
      break;
    } catch {
      await sleep(100);
    }
  }
  session = await createSession(request);
  await until(`return !!document.querySelector('[aria-label="Notebook title"]')`);
  assert.equal(await evaluate('return document.querySelectorAll(".cell-card").length'), 0);
  assert.equal(
    await evaluate(`return document.querySelector('[aria-label="Notebook title"]').value`),
    'Untitled notebook',
  );
  await click('Add query');
  await until('return document.querySelectorAll(".cell-card").length === 1');
  await until(`return document.activeElement?.getAttribute('aria-label') === 'query_1 SQL'`);
  assert.equal(await evaluate(`return document.querySelector('[aria-label="query_1 SQL"]').innerText.trim()`), '');
  assert.equal(await evaluate(`return document.querySelector('[aria-label="query_1 notes"]').value`), '');
  assert.equal(await evaluate('return document.body.innerText.includes("Notebook guide")'), false);
  await click('Add a query cell');
  await until('return document.querySelectorAll(".cell-card").length === 2');
  await until(`return document.activeElement?.getAttribute('aria-label') === 'query_2 SQL'`);
  assert.equal(await evaluate(`return document.querySelector('[aria-label="query_2 SQL"]').innerText.trim()`), '');
  assert.equal(await evaluate(`return document.querySelector('[aria-label="query_2 notes"]').value`), '');
  await request(`/session/${session}/actions`, 'POST', {
    actions: [
      {
        type: 'key',
        id: 'keyboard',
        actions: [...'SELECT 1;'].flatMap((value) => [
          { type: 'keyDown', value },
          { type: 'keyUp', value },
        ]),
      },
    ],
  });
  await until(`return document.querySelector('[aria-label="query_2 SQL"]').innerText.trim() === 'SELECT 1;'`);
  assert.equal(await evaluate(`return document.querySelector('[aria-label="query_1 SQL"]').innerText.trim()`), '');
  const fixture = JSON.parse(await readFile(new URL('./fixtures/notebook.json', import.meta.url), 'utf8'));
  await evaluate(
    `const input = document.querySelector('[aria-label="Notebook file"]');
     const transfer = new DataTransfer();
     transfer.items.add(new File([JSON.stringify(arguments[0])], 'test.jupsql.json', {type: 'application/json'}));
     input.files = transfer.files;
     input.dispatchEvent(new Event('change', {bubbles: true}));`,
    [fixture],
  );
  await until('return document.querySelectorAll(".cell-card").length === 3');
  assert.ok(await evaluate('return innerWidth > 800 && innerHeight > 400'));
  if (['invalid-port', 'rejected', 'url-invalid'].includes(connectionMode)) {
    await until('return document.body.innerText.includes("Automatic connection failed:")');
    await click('Connect a database');
    await until('return !!document.querySelector("[role=dialog]")');
    if (process.env.JUPSQL_TEST_PORT) {
      await setInput('input[type=number]', process.env.JUPSQL_TEST_PORT);
    }
    // Correct the detected settings through the ordinary manual connection form.
    for (const [label, value] of [
      ['Host', '127.0.0.1'],
      ['Database', 'jupsql_demo'],
      ['Username', 'jupsql'],
    ]) {
      await evaluate(
        `const input = [...document.querySelectorAll('[role=dialog] label')].find(label => label.textContent.trim().startsWith(arguments[0])).querySelector('input'); Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set.call(input, arguments[1]); input.dispatchEvent(new Event('input', {bubbles:true}));`,
        [label, value],
      );
    }
    await click('Connect');
  } else {
    // A successful startup must connect without opening or submitting the dialog.
    await until('return !!document.querySelector(".connection-dot.connected")');
    assert.equal(await evaluate('return !!document.querySelector("[role=dialog]")'), false);
    await click('Change connection');
    await until('return !!document.querySelector("[role=dialog]")');
    assert.equal(
      await evaluate('return document.querySelector("[role=dialog] input").value'),
      connectionMode === 'url' ? '127.0.0.1' : (connectionEnvironment.PGHOST ?? 'localhost'),
    );
    assert.equal(
      await evaluate('return document.querySelector("input[type=number]").disabled'),
      connectionMode.startsWith('socket'),
    );
    assert.equal(await evaluate('return document.querySelector("[role=dialog] input").disabled'), false);
    if (connectionMode === 'defaults') {
      assert.equal(
        await evaluate('return document.querySelector(".connection-card p").textContent'),
        'postgres · postgres',
      );
    }
    await evaluate(`document.querySelector('[aria-label="Close connection dialog"]').click()`);
  }
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
  // Ctrl+Enter must run the focused editor's cell without inserting a blank line.
  const sqlBeforeShortcut = await evaluate(
    `const editor = document.querySelector('[aria-label="by_customer SQL"]'); editor.focus(); return editor.innerText;`,
  );
  await request(`/session/${session}/actions`, 'POST', {
    actions: [
      {
        type: 'key',
        id: 'keyboard',
        actions: [
          { type: 'keyDown', value: '\uE009' },
          { type: 'keyDown', value: '\uE007' },
          { type: 'keyUp', value: '\uE007' },
          { type: 'keyUp', value: '\uE009' },
        ],
      },
    ],
  });
  assert.equal(
    await evaluate(`return document.querySelector('[aria-label="by_customer SQL"]').innerText`),
    sqlBeforeShortcut,
  );
  await until(
    'return !document.querySelector(".run-progress") && document.querySelectorAll(".results tbody")[2].textContent.includes("999.00")',
  );
  assert.equal(await evaluate('return document.querySelectorAll(".state-badge.stale").length'), 2);

  // Renaming must not clear an existing stale state.
  await evaluate(
    `const input = document.querySelector('[aria-label="Cell 1 name"]'); Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set.call(input, 'renamed'); input.dispatchEvent(new Event('input', {bubbles:true}));`,
  );
  await until('return document.querySelectorAll(".validation-error").length === 2');
  assert.equal(
    await evaluate('return !!document.querySelector(".cell-card").querySelector(".state-badge.stale")'),
    true,
  );
  await evaluate(
    `const input = document.querySelector('[aria-label="Cell 1 name"]'); Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set.call(input, 'orders'); input.dispatchEvent(new Event('input', {bubbles:true}));`,
  );
  await until(
    'return document.querySelectorAll(".validation-error").length === 0 && !document.querySelectorAll(".run-button")[0].disabled',
  );
  // Ctrl+Shift+Enter refreshes the focused query and its dependents in one snapshot.
  const sqlBeforeRefresh = await evaluate(
    `const editor = document.querySelector('[aria-label="orders SQL"]'); editor.focus(); return editor.innerText;`,
  );
  await request(`/session/${session}/actions`, 'POST', {
    actions: [
      {
        type: 'key',
        id: 'keyboard',
        actions: [
          { type: 'keyDown', value: '\uE009' },
          { type: 'keyDown', value: '\uE008' },
          { type: 'keyDown', value: '\uE007' },
          { type: 'keyUp', value: '\uE007' },
          { type: 'keyUp', value: '\uE008' },
          { type: 'keyUp', value: '\uE009' },
        ],
      },
    ],
  });
  assert.equal(
    await evaluate(`return document.querySelector('[aria-label="orders SQL"]').innerText`),
    sqlBeforeRefresh,
  );
  await until(
    'return !document.querySelector(".run-progress") && document.querySelectorAll(".state-badge.stale").length === 0',
  );
  const refreshedSnapshots = await evaluate(
    'return [...document.querySelectorAll(".result-bar time")].map(t => t.title.split("Snapshot ")[1])',
  );
  assert.equal(new Set(refreshedSnapshots).size, 1);
  assert.ok(await evaluate('return [...document.querySelectorAll(".results tbody")].every(t => t.rows.length === 1)'));

  // A fresh result survives a rename, while references to the old name become stale/invalid.
  const firstResult = await evaluate('return document.querySelector(".results tbody").innerText');
  await setInput('[aria-label="Cell 1 name"]', 'renamed');
  await until('return document.querySelectorAll(".validation-error").length === 2');
  assert.equal(
    await evaluate('return !!document.querySelector(".cell-card").querySelector(".state-badge.stale")'),
    false,
  );
  assert.equal(await evaluate('return document.querySelectorAll(".state-badge.stale").length'), 2);
  assert.equal(await evaluate('return document.querySelector(".results tbody").innerText'), firstResult);
  await setInput('[aria-label="Cell 1 name"]', 'orders');
  await until(
    'return document.querySelectorAll(".validation-error").length === 0 && !document.querySelectorAll(".run-button")[0].disabled',
  );
  assert.equal(
    await evaluate('return !!document.querySelector(".cell-card").querySelector(".state-badge.stale")'),
    false,
  );
  assert.equal(await evaluate('return document.querySelectorAll(".state-badge.stale").length'), 2);
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
  assert.ok(!log.value.includes('environment-password-never-save-or-log'));
  assert.ok(!log.value.includes('url@password-never-save-or-log'));
  assert.ok(!log.value.includes('url-password-never-save-or-log'));
  if (connectionEnvironment.DATABASE_URL) {
    assert.ok(!log.value.includes(connectionEnvironment.DATABASE_URL));
  }
  const catalog = await readFile(`${process.env.JUPSQL_DATA_DIR}/catalog.json`, 'utf8');
  for (const credential of [
    'environment-password-never-save-or-log',
    'url@password-never-save-or-log',
    'url-password-never-save-or-log',
  ]) {
    assert.ok(!catalog.includes(credential));
  }
  if (connectionEnvironment.DATABASE_URL) assert.ok(!catalog.includes(connectionEnvironment.DATABASE_URL));
  const profile = await evaluate('return localStorage.getItem("jupsql.connection")');
  assert.ok(!profile?.includes('environment-password-never-save-or-log'));
  assert.equal(events.filter((event) => event.event === 'connection_environment').length, 1);
  if (['defaults', 'tcp', 'url'].includes(connectionMode) || connectionMode.startsWith('socket')) {
    assert.equal(events.filter((event) => event.event === 'connection_attempt').length, 1);
  }
  assert.ok(events.some((event) => event.event === 'cell_started'));
  assert.ok(events.some((event) => event.event === 'ui.cell_focused'));
  assert.ok(events.some((event) => event.event === 'notebook_saved'));
  assert.ok(events.some((event) => event.event === 'ui.notebook_imported'));

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
    const saved = JSON.parse(await readFile(`${bootstrap.value.dataDirectory}/notebook.json`, 'utf8'));
    if (saved.title === 'Close flush check') break;
    await sleep(100);
  }
  assert.equal(
    JSON.parse(await readFile(`${bootstrap.value.dataDirectory}/notebook.json`, 'utf8')).title,
    'Close flush check',
  );
  if (connectionMode === 'tcp') {
    const peerPort = port + 1;
    const peerRequest = driverRequests(`http://127.0.0.1:${peerPort}`);
    let peerDriver;
    let peerSession;
    let peerNotebookId;
    const peerEvaluate = (script) => peerRequest(`/session/${peerSession}/execute/sync`, 'POST', { script, args: [] });
    const peerInvoke = async (command, args) => {
      const result = await peerRequest(`/session/${peerSession}/execute/async`, 'POST', {
        script:
          'const done = arguments[arguments.length - 1]; window.__TAURI_INTERNALS__.invoke(arguments[0], arguments[1]).then(value => done({value}), error => done({error:String(error)}));',
        args: [command, { notebookId: peerNotebookId, ...args }],
      });
      if (command === 'bootstrap' && result.value) peerNotebookId = result.value.notebookId;
      return result;
    };
    async function openPeer(environment) {
      peerDriver = spawn('WebKitWebDriver', [`--port=${peerPort}`, '--host=127.0.0.1'], {
        env: { ...driverEnvironment, ...environment },
        stdio: ['ignore', 'inherit', 'inherit'],
      });
      for (let i = 0; i < 50; i += 1) {
        try {
          await peerRequest('/status');
          break;
        } catch {
          await sleep(100);
        }
      }
      peerSession = await createSession(peerRequest);
      for (let i = 0; i < 100; i += 1) {
        if (await peerEvaluate(`return !!document.querySelector('[aria-label="Notebook title"]')`)) break;
        await sleep(100);
      }
      return (await peerInvoke('bootstrap', {})).value;
    }
    async function closePeer() {
      try {
        if (peerSession) {
          const closed = await peerInvoke('plugin:window|close', { label: 'main' });
          assert.equal(closed.error, undefined);
          await peerRequest(`/session/${peerSession}`, 'DELETE').catch(() => {});
        }
      } finally {
        peerSession = null;
        if (peerDriver && peerDriver.exitCode === null && peerDriver.signalCode === null) {
          const exited = once(peerDriver, 'exit');
          peerDriver.kill('SIGTERM');
          await exited;
        }
      }
    }
    async function reopenPrimary() {
      await request(`/session/${session}`, 'DELETE').catch(() => {});
      session = await createSession(request);
      await until(`return !!document.querySelector('[aria-label="Notebook title"]')`);
      return (await invoke('bootstrap', {})).value;
    }
    try {
      const resumed = await reopenPrimary();
      assert.equal(resumed.dataDirectory, bootstrap.value.dataDirectory);
      assert.equal(resumed.notebook.title, 'Close flush check');
      assert.equal(resumed.notebook.cells.length, 3);

      // An already-open most recent notebook creates a new empty notebook in a second process.
      const parallel = await openPeer({});
      assert.notEqual(parallel.dataDirectory, resumed.dataDirectory);
      assert.equal(parallel.notebook.title, 'Untitled notebook');
      assert.equal(parallel.notebook.cells.length, 0);
      assert.equal(await evaluate('return document.querySelectorAll(".cell-card").length'), 3);
      await closePeer();
      await invoke('plugin:window|close', { label: 'main' });
      const latest = await reopenPrimary();
      assert.equal(latest.dataDirectory, parallel.dataDirectory);
      assert.equal(latest.notebook.cells.length, 0);

      // A different database has separate history, even when it was opened more recently.
      const other = await openPeer({ PGDATABASE: 'postgres' });
      assert.notEqual(other.dataDirectory, latest.dataDirectory);
      assert.notEqual(other.dataDirectory, resumed.dataDirectory);
      assert.equal(other.notebook.cells.length, 0);
      await peerEvaluate(
        `const input = document.querySelector('[aria-label="Notebook title"]'); Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set.call(input, 'Other database notebook'); input.dispatchEvent(new Event('input', {bubbles:true}));`,
      );
      await closePeer();
      assert.equal(
        JSON.parse(await readFile(`${other.dataDirectory}/notebook.json`, 'utf8')).title,
        'Other database notebook',
      );
      await invoke('plugin:window|close', { label: 'main' });
      const matching = await reopenPrimary();
      assert.equal(matching.dataDirectory, parallel.dataDirectory);
      assert.equal(matching.notebook.title, 'Untitled notebook');
      await until(`return !!document.querySelector('.connection-dot.connected')`);
      await until(`return document.querySelectorAll('[aria-label="Workspace notebooks"] button').length === 3`);

      async function selectNotebook(id, title) {
        await until(
          `const button = document.querySelector('[data-notebook-id="${id}"]'); return !!button && !button.disabled;`,
        );
        await evaluate(`document.querySelector('[data-notebook-id="${id}"]').click()`);
        await until(
          `return document.querySelector('[aria-label="Notebook title"]').value === ${JSON.stringify(title)}`,
        );
        const selected = await invoke('bootstrap', {});
        assert.equal(selected.value.notebookId, id);
        assert.equal(await evaluate(`return document.querySelector('[aria-current="page"]').dataset.notebookId`), id);
        return selected.value;
      }

      const original = await selectNotebook(resumed.notebookId, 'Close flush check');
      assert.equal(original.notebook.cells.length, 3);
      assert.equal(await evaluate('return document.querySelectorAll(".results").length'), 0);
      await setInput('[aria-label="Notebook title"]', 'Unsaved switch draft');
      const different = await selectNotebook(other.notebookId, 'Other database notebook');
      assert.equal(
        JSON.parse(await readFile(`${original.dataDirectory}/notebook.json`, 'utf8')).title,
        'Unsaved switch draft',
      );
      assert.equal(different.connectionDefaults.autoConnect, false);
      assert.equal(different.connectionDefaults.input.database, 'postgres');
      assert.equal(different.connectionDefaults.input.password, '');
      assert.equal(await evaluate(`return !!document.querySelector('.connection-dot.connected')`), false);
      const stale = await invoke('save_notebook', {
        notebookId: original.notebookId,
        notebook: { ...original.notebook, title: 'Stale overwrite' },
      });
      assert.ok(stale.error.includes('notebook changed'));
      assert.equal(
        JSON.parse(await readFile(`${different.dataDirectory}/notebook.json`, 'utf8')).title,
        'Other database notebook',
      );

      await click('Connect a database');
      await until(`return !!document.querySelector('[role=dialog]')`);
      assert.equal(await evaluate(`return document.querySelector('input[type=password]').value`), '');
      await click('Connect');
      await until(
        `return !document.querySelector('[role=dialog]') && !!document.querySelector('.connection-dot.connected')`,
      );
      assert.equal(
        await evaluate(`return document.querySelector('.connection-card p').textContent`),
        'postgres · jupsql',
      );

      const live = await openPeer({});
      assert.equal(live.notebookId, original.notebookId);
      await until(
        `const button = document.querySelector('[data-notebook-id="${live.notebookId}"]'); return button?.disabled && !!button.querySelector('[aria-label="Open in another window"]');`,
      );
      await until(
        `return document.querySelector('[aria-label="Workspace notebooks"] button').dataset.notebookId === '${live.notebookId}'`,
      );
      const busy = await invoke('open_notebook', { id: live.notebookId });
      assert.ok(busy.error.includes('already open'));
      assert.equal((await invoke('bootstrap', {})).value.notebookId, different.notebookId);
      await peerEvaluate(
        `const input = document.querySelector('[aria-label="Notebook title"]'); Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set.call(input, 'Live peer rename'); input.dispatchEvent(new Event('input', {bubbles:true}));`,
      );
      await until(
        `return document.querySelector('[data-notebook-id="${live.notebookId}"]').textContent.includes('Live peer rename')`,
      );
      await writeFile(
        '.cache/validation/native-workspace.png',
        Buffer.from(await request(`/session/${session}/screenshot`), 'base64'),
      );
      await closePeer();
      await selectNotebook(live.notebookId, 'Live peer rename');
      await until(`return !!document.querySelector('.connection-dot.connected')`);
      assert.equal(
        await evaluate(`return document.querySelector('.connection-card p').textContent`),
        'jupsql_demo · jupsql',
      );
      const activityBefore = (await invoke('list_notebooks', {})).value.find(
        (entry) => entry.id === live.notebookId,
      ).lastActivity;
      await sleep(650);
      const activityAfter = (await invoke('list_notebooks', {})).value.find(
        (entry) => entry.id === live.notebookId,
      ).lastActivity;
      assert.equal(activityAfter, activityBefore, 'an unchanged autosave must not count as activity');
      const cached = await selectNotebook(different.notebookId, 'Other database notebook');
      assert.equal(cached.connectionDefaults.autoConnect, true);
      await until(`return !!document.querySelector('.connection-dot.connected')`);
      assert.equal(
        await evaluate(`return document.querySelector('.connection-card p').textContent`),
        'postgres · jupsql',
      );
      console.log(
        'Native workspace browser passed: listing, activity order, live updates, draft flush, stale-save rejection, busy notebooks, and safe connection switching.',
      );
      console.log(
        'Native notebook history passed: resume, concurrent empty notebook, latest-opened selection, and separate database history.',
      );
    } finally {
      await closePeer().catch(() => {});
    }
  }
  console.log(
    `Native Tauri smoke passed (${connectionMode} connection): blank startup and new cells, import, connect, run, shared snapshot, edit/stale propagation, Ctrl+Enter, Ctrl+Shift+Enter, current SQL composition, rename validation, refresh, persistence, logs, and save on close.`,
  );
} finally {
  if (session) await request(`/session/${session}`, 'DELETE').catch(() => {});
  driver.kill('SIGTERM');
}
