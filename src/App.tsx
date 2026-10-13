import {
  BookOpen,
  Braces,
  Check,
  ChevronRight,
  CircleHelp,
  Database,
  Download,
  FileInput,
  GitBranch,
  History,
  LoaderCircle,
  Plus,
  Radio,
  Square,
  Terminal,
  X,
} from 'lucide-react';
import { useRef, useState } from 'react';
import * as bridge from './bridge';
import { CellCard } from './CellCard';
import { ConnectionDialog } from './ConnectionDialog';
import { affectedIds, exampleNotebook, newCell, parseNotebook } from './model';
import type { Bootstrap, Cell } from './model';
import { download } from './Results';
import { useNotebook } from './useNotebook';

export function App({ initial }: { initial: Bootstrap }) {
  const state = useNotebook(initial.notebook ?? exampleNotebook(), initial.connectionDefaults);
  const [connecting, setConnecting] = useState(false);
  const [showSession, setShowSession] = useState(false);
  const [showHelp, setShowHelp] = useState(false);
  const [sessionText, setSessionText] = useState('');
  const [selected, setSelected] = useState<string | null>(null);
  const [deleted, setDeleted] = useState<{ cell: Cell; index: number } | null>(null);
  const fileInput = useRef<HTMLInputElement>(null);
  const names = state.notebook.cells.map((cell) => cell.name);
  const canRun = bridge.desktop && !!state.connection && !state.busy && !state.analyzing && !connecting;
  const staleCount = state.notebook.cells.filter((cell) => {
    const result = state.results[cell.id];
    return (
      result && (result.stale || result.compiledSql !== state.analyses.find((a) => a.id === result.cellId)?.compiledSql)
    );
  }).length;

  function addCell() {
    let index = state.notebook.cells.length + 1;
    while (names.includes(`query_${index}`)) index += 1;
    const cell = newCell(index);
    state.change({ ...state.notebook, cells: [...state.notebook.cells, cell] }, new Set());
    state.log('cell_added', { cellId: cell.id });
    setTimeout(
      () => document.getElementById(`cell-${cell.id}`)?.scrollIntoView({ behavior: 'smooth', block: 'center' }),
      0,
    );
  }

  function focusCell(id: string) {
    if (id === selected) return;
    setSelected(id);
    state.log('cell_focused', { cellId: id });
  }

  return (
    <div className="app-shell">
      <aside className="sidebar">
        <div className="brand">
          <span className="brand-mark">
            <Braces size={22} />
          </span>
          <span>
            JupSQL<span className="prototype-tag">PROTOTYPE</span>
          </span>
        </div>
        <div className="sidebar-section-label">WORKSPACE</div>
        <button className="notebook-nav">
          <BookOpen size={16} />
          <span className="truncate">{state.notebook.title || 'Untitled notebook'}</span>
          <ChevronRight size={14} />
        </button>
        <div className="outline-heading">
          <span className="sidebar-section-label">CELLS</span>
          <span>{state.notebook.cells.length}</span>
        </div>
        <nav className="cell-outline" aria-label="Notebook cells">
          {state.notebook.cells.map((cell, index) => {
            const result = state.results[cell.id];
            const stale =
              result &&
              (result.stale || result.compiledSql !== state.analyses.find((a) => a.id === cell.id)?.compiledSql);
            return (
              <button
                key={cell.id}
                className={selected === cell.id ? 'selected' : ''}
                onClick={() => {
                  focusCell(cell.id);
                  document.getElementById(`cell-${cell.id}`)?.scrollIntoView({ behavior: 'smooth', block: 'start' });
                }}
              >
                <span className="outline-index">{String(index + 1).padStart(2, '0')}</span>
                <span className="truncate">{cell.name || 'Unnamed cell'}</span>
                <span className={`outline-dot ${stale ? 'stale' : result?.error ? 'error' : result ? 'fresh' : ''}`} />
              </button>
            );
          })}
        </nav>
        <button className="sidebar-add" onClick={addCell} disabled={state.notebook.cells.length >= 100}>
          <Plus size={14} />
          Add query
        </button>
        <div className="sidebar-bottom">
          <div className="connection-card">
            <div className="flex items-center gap-2">
              <Database size={15} />
              <strong>PostgreSQL</strong>
              <span className={`connection-dot ${state.connection ? 'connected' : ''}`} />
            </div>
            <p>
              {state.autoConnecting ? 'Connecting from environment…' : (state.connection ?? 'No database connected')}
            </p>
            <button
              disabled={!bridge.desktop || state.busy || state.autoConnecting}
              onClick={() => setConnecting(true)}
            >
              {state.connection ? 'Change connection' : 'Connect a database'}
              <ChevronRight size={13} />
            </button>
          </div>
          <button className="sidebar-link" onClick={() => setShowHelp(true)}>
            <CircleHelp size={15} />
            Notebook guide
          </button>
          <button
            className="sidebar-link"
            disabled={!bridge.desktop}
            onClick={() => {
              setShowSession(true);
              void bridge.sessionLog().then(setSessionText).catch(state.fail);
            }}
          >
            <History size={15} />
            Session log
            <span className="live-dot" />
          </button>
          <div className="local-label">
            <Radio size={12} />
            Local to this computer
          </div>
        </div>
      </aside>
      <main className="main-panel">
        <header className="topbar">
          <div className="flex items-center gap-2">
            <BookOpen size={14} />
            <span>Notebooks</span>
            <ChevronRight size={13} />
            <span className="text-slate-700">{state.notebook.title || 'Untitled notebook'}</span>
          </div>
          <div className="flex items-center gap-4">
            <span className="save-status">
              <Check size={12} />
              {bridge.desktop ? state.saveState : 'Browser preview'}
            </span>
            <button
              className="icon-button"
              aria-label="Import notebook"
              title="Import notebook"
              disabled={state.busy}
              onClick={() => fileInput.current?.click()}
            >
              <FileInput size={16} />
            </button>
            <button
              className="icon-button"
              aria-label="Export notebook"
              title="Export notebook"
              onClick={() => {
                void download(
                  `${state.notebook.title || 'notebook'}.jupsql.json`,
                  JSON.stringify(state.notebook, null, 2),
                  'application/json',
                )
                  .then(() => state.log('notebook_export_requested', {}))
                  .catch(state.fail);
              }}
            >
              <Download size={16} />
            </button>
          </div>
        </header>
        <div className="notebook-content">
          <div className="notebook-title-row">
            <div>
              <div className="eyebrow">POSTGRESQL NOTEBOOK</div>
              <input
                aria-label="Notebook title"
                className="notebook-title"
                value={state.notebook.title}
                onChange={(e) => state.change({ ...state.notebook, title: e.target.value }, new Set())}
              />
              <p className="notebook-subtitle">A place to query, connect the dots, and keep your train of thought.</p>
            </div>
            <span className="sql-emblem">
              <Terminal size={26} />
            </span>
          </div>
          <div className="notebook-toolbar">
            <div className="flex items-center gap-4">
              <button className="button primary" disabled={!canRun} onClick={() => void state.execute(null, true)}>
                <GitBranch size={15} />
                Run all queries
              </button>
              <span className="toolbar-detail">
                {state.notebook.cells.length} cells{staleCount > 0 ? ` · ${staleCount} stale` : ''}
              </span>
            </div>
            <span className="shortcut-hint">
              <kbd>Ctrl</kbd> + <kbd>Enter</kbd> to run a cell
            </span>
          </div>
          {!bridge.desktop ? (
            <div className="info-banner">
              <Terminal size={17} />
              <div>
                <strong>Desktop preview</strong>
                <p>
                  Launch with <code>pnpm dev</code> in the Nix shell to connect to PostgreSQL and record sessions.
                </p>
              </div>
            </div>
          ) : (
            !state.connection && (
              <div className="info-banner">
                <Database size={17} />
                <div>
                  <strong>
                    {state.autoConnecting
                      ? 'Connecting to PostgreSQL…'
                      : 'Your notebook is ready. Connect a database to run it.'}
                  </strong>
                  <p>
                    The example queries work on any PostgreSQL database. For the local demo, run{' '}
                    <code>pnpm db:demo</code>.
                  </p>
                </div>
                <button className="button compact" disabled={state.autoConnecting} onClick={() => setConnecting(true)}>
                  Connect
                </button>
              </div>
            )
          )}
          {state.loggingError && (
            <div role="alert" className="error-box mb-4">
              Local saving or session logging needs attention: {state.loggingError}
            </div>
          )}
          {state.busy && (
            <div className="run-progress" role="status">
              <LoaderCircle size={16} className="animate-spin" />
              <span>
                {state.progress
                  ? `Running ${state.notebook.cells.find((c) => c.id === state.progress?.cellId)?.name ?? 'query'} · ${state.progress.index + 1} of ${state.progress.total}`
                  : 'Starting run…'}
              </span>
              <button className="button compact" onClick={state.cancel}>
                <Square size={11} fill="currentColor" />
                Cancel
              </button>
            </div>
          )}
          {state.notice && (
            <div className="notice" role="status">
              <span>{state.notice}</span>
              <button className="icon-button small" aria-label="Dismiss notification" onClick={state.dismissNotice}>
                <X size={14} />
              </button>
            </div>
          )}
          {deleted && (
            <div className="notice">
              <span>Cell removed.</span>
              <button
                className="text-button"
                onClick={() => {
                  const cells = [...state.notebook.cells];
                  cells.splice(Math.min(deleted.index, cells.length), 0, deleted.cell);
                  state.change({ ...state.notebook, cells }, new Set([deleted.cell.id]));
                  setDeleted(null);
                }}
              >
                Undo
              </button>
            </div>
          )}
          <div className="cells">
            {state.notebook.cells.map((cell, index) => (
              <CellCard
                key={cell.id}
                cell={cell}
                index={index}
                count={state.notebook.cells.length}
                names={names}
                analysis={state.analyses.find((a) => a.id === cell.id)}
                result={state.results[cell.id]}
                canRun={canRun}
                running={state.progress?.cellId === cell.id}
                onChange={state.editCell}
                onRun={() => void state.execute(cell.id, false)}
                onRefresh={() => void state.execute(cell.id, true)}
                onFocus={() => focusCell(cell.id)}
                onDelete={() => {
                  setDeleted({ cell, index });
                  state.change(
                    { ...state.notebook, cells: state.notebook.cells.filter((c) => c.id !== cell.id) },
                    affectedIds(state.notebook, state.analyses, cell.id),
                  );
                  state.log('cell_deleted', { cellId: cell.id });
                }}
                onMove={(direction) => {
                  const cells = [...state.notebook.cells];
                  const other = cells[index + direction];
                  if (!other) return;
                  cells[index] = other;
                  cells[index + direction] = cell;
                  state.change({ ...state.notebook, cells }, new Set());
                  state.log('cell_moved', { cellId: cell.id, direction });
                }}
              />
            ))}
          </div>
          <button className="add-cell" onClick={addCell} disabled={state.notebook.cells.length >= 100}>
            <Plus size={17} />
            Add a query cell
          </button>
          <footer className="notebook-footer">
            <span>
              <span className="live-dot" />
              {bridge.desktop ? 'Session recording is on' : 'Session recording starts in the desktop app'}
            </span>
            <span>References use current SQL · Refreshes share one snapshot</span>
          </footer>
        </div>
      </main>
      <input
        ref={fileInput}
        type="file"
        accept=".json"
        className="hidden"
        aria-label="Notebook file"
        onChange={(event) => {
          const file = event.target.files?.[0];
          event.target.value = '';
          if (!file) return;
          if (file.size > 2_000_000) {
            state.fail('Notebook exceeds the 2 MB prototype limit.');
            return;
          }
          void file
            .text()
            .then((text) => {
              const next = parseNotebook(text);
              setDeleted(null);
              state.change(next, new Set([...state.notebook.cells, ...next.cells].map((cell) => cell.id)));
              state.log('notebook_imported', { filename: file.name });
            })
            .catch(state.fail);
        }}
      />
      {connecting && (
        <ConnectionDialog
          initialValue={state.connectionInput}
          onClose={() => setConnecting(false)}
          onConnected={state.connected}
        />
      )}
      {showHelp && (
        <div className="modal-backdrop">
          <section className="modal guide" role="dialog" aria-modal="true" aria-labelledby="guide-title">
            <div className="flex justify-between">
              <h2 id="guide-title">A small notebook guide</h2>
              <button className="icon-button" aria-label="Close guide" onClick={() => setShowHelp(false)}>
                <X size={18} />
              </button>
            </div>
            <p>
              Name a query cell <code>orders</code>, then use it as a table in another cell:
            </p>
            <pre>SELECT * FROM notebook.orders AS o{'\n'}WHERE o.amount &gt; 100;</pre>
            <p>
              References expand the cell’s <strong>current SQL</strong>. They include its entire query, even when its
              displayed result is limited. Cell order doesn’t affect references.
            </p>
            <p>
              Editing SQL marks dependent results stale. <strong>Rerun dependents</strong> refreshes the source cell and
              downstream query cells in one read-only database snapshot. Each cell is executed independently; an error
              doesn’t discard other successful cells.
            </p>
            <p>
              <strong>Mutation cells</strong> run only when you click Run mutation. They may use query definitions, but
              cannot be referenced. Successful writes mark other results stale.
            </p>
            <p>
              Results show at most 500 rows and 2 MB. Queries time out after 30 seconds. Notes and SQL save
              automatically; results are kept only for this window.
            </p>
          </section>
        </div>
      )}
      {showSession && (
        <div className="modal-backdrop">
          <section className="modal session-modal" role="dialog" aria-modal="true" aria-labelledby="session-title">
            <div className="flex items-center justify-between">
              <h2 id="session-title">Session log</h2>
              <button className="icon-button" aria-label="Close session log" onClick={() => setShowSession(false)}>
                <X size={18} />
              </button>
            </div>
            <p className="muted">
              Local JSONL records edits, executed SQL, dependency expansion, timing, failures, and cancellation. Results
              and connection passwords are excluded. SQL literals are recorded as written.
            </p>
            <label className="mt-4 block">
              Current log
              <input readOnly value={initial.logPath} onFocus={(e) => e.target.select()} />
            </label>
            <pre className="session-log">{sessionText || 'No events loaded.'}</pre>
            <div className="flex justify-between">
              <button
                className="button"
                onClick={() => void bridge.sessionLog().then(setSessionText).catch(state.fail)}
              >
                Refresh
              </button>
              <button
                className="button primary"
                onClick={() =>
                  void download(`jupsql-session-${initial.sessionId}.jsonl`, sessionText, 'application/x-ndjson').catch(
                    state.fail,
                  )
                }
              >
                <Download size={14} />
                Export session
              </button>
            </div>
          </section>
        </div>
      )}
    </div>
  );
}
