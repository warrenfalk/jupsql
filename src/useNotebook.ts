import { useEffect, useRef, useState } from 'react';
import * as bridge from './bridge';
import { acceptResults, affectedIds } from './model';
import type {
  Analysis,
  Bootstrap,
  Cell,
  ConnectionDefaults,
  ConnectionInput,
  DisplayResult,
  Notebook,
  Progress,
  RunResult,
} from './model';

const message = (error: unknown) => (error instanceof Error ? error.message : String(error));

export function useNotebook(initial: Notebook, defaults: ConnectionDefaults, notebookId: string) {
  const [notebook, setNotebook] = useState(initial);
  const [analyses, setAnalyses] = useState<Analysis[]>([]);
  const [analyzing, setAnalyzing] = useState(bridge.desktop);
  const [results, setResults] = useState<Record<string, DisplayResult>>({});
  const [busy, setBusy] = useState(false);
  const [switching, setSwitching] = useState(false);
  const switchingLock = useRef(false);
  const [progress, setProgress] = useState<Progress | null>(null);
  const [notice, setNotice] = useState<string | null>(
    defaults.error ? `Automatic connection failed: ${defaults.error}` : null,
  );
  const [saveState, setSaveState] = useState('Saved locally');
  const [connection, setConnection] = useState<string | null>(null);
  const [connectionInput, setConnectionInput] = useState(defaults.input);
  const [autoConnecting, setAutoConnecting] = useState(defaults.autoConnect && !defaults.error);
  const [loggingError, setLoggingError] = useState<string | null>(null);
  const changedDuringRun = useRef(new Set<string>());
  const saveQueue = useRef(Promise.resolve());
  const runLock = useRef(false);
  const latestNotebook = useRef(initial);
  const closing = useRef(false);
  const activeExecution = useRef<Promise<RunResult> | null>(null);
  const automaticConnection = useRef<Promise<string> | null>(null);

  useEffect(() => {
    if (!bridge.desktop || !defaults.autoConnect || defaults.error) return;
    let disposed = false;
    automaticConnection.current ??= bridge.connectEnvironment(notebookId);
    void automaticConnection.current
      .then((label) => {
        if (!disposed) setConnection(label);
      })
      .catch((error: unknown) => {
        if (!disposed) setNotice(`Automatic connection failed: ${message(error)}`);
      })
      .finally(() => {
        if (!disposed) setAutoConnecting(false);
      });
    return () => {
      disposed = true;
    };
  }, [defaults]);

  function log(event: string, data: Record<string, unknown>) {
    void bridge.record(notebookId, event, data).catch((error: unknown) => setLoggingError(message(error)));
  }

  useEffect(() => {
    if (!bridge.desktop) return;
    let disposed = false;
    const subscription = bridge.subscribeProgress((next) => {
      if (!disposed) setProgress(next);
    });
    void subscription.catch((error: unknown) => setNotice(message(error)));
    return () => {
      disposed = true;
      void subscription.then((unlisten) => unlisten()).catch(() => {});
    };
  }, []);

  useEffect(() => {
    if (!bridge.desktop) return;
    return bridge.subscribeClose(
      async () => {
        closing.current = true;
        if (activeExecution.current) {
          await bridge.cancel(notebookId);
          await activeExecution.current.catch(() => {});
        }
        await saveQueue.current.catch(() => {});
        await bridge.save(notebookId, latestNotebook.current);
        await bridge.record(notebookId, 'window_close_requested', {});
      },
      (error) => {
        closing.current = false;
        setLoggingError(message(error));
      },
    );
  }, []);

  useEffect(() => {
    if (!bridge.desktop) return;
    let disposed = false;
    const timer = setTimeout(() => {
      void bridge
        .analyze(notebook)
        .then((next) => {
          if (!disposed) {
            setAnalyses(next);
            setAnalyzing(false);
          }
        })
        .catch((error: unknown) => {
          if (!disposed) {
            setNotice(message(error));
            setAnalyzing(false);
          }
        });
    }, 180);
    return () => {
      disposed = true;
      clearTimeout(timer);
    };
  }, [notebook]);

  useEffect(() => {
    if (!bridge.desktop) return;
    let disposed = false;
    const timer = setTimeout(() => {
      if (closing.current || switchingLock.current) return;
      saveQueue.current = saveQueue.current.catch(() => {}).then(() => bridge.save(notebookId, notebook));
      void saveQueue.current
        .then(() => {
          if (!disposed) setSaveState('Saved locally');
        })
        .catch((error: unknown) => {
          if (!disposed) {
            setSaveState('Save failed');
            setLoggingError(message(error));
          }
        });
    }, 450);
    return () => {
      disposed = true;
      clearTimeout(timer);
    };
  }, [notebook]);

  function change(next: Notebook, changed: Set<string>) {
    if (switchingLock.current || closing.current) return;
    latestNotebook.current = next;
    setNotebook(next);
    setAnalyzing(bridge.desktop);
    setSaveState('Saving…');
    for (const id of changed) changedDuringRun.current.add(id);
    setResults((previous) =>
      Object.fromEntries(
        Object.entries(previous).map(([id, result]) => [id, changed.has(id) ? { ...result, stale: true } : result]),
      ),
    );
  }

  function editCell(cell: Cell) {
    const previous = notebook.cells.find((item) => item.id === cell.id);
    const executionChanged = previous?.sql !== cell.sql || previous?.kind !== cell.kind;
    const changed =
      executionChanged || previous?.name !== cell.name ? affectedIds(notebook, analyses, cell.id) : new Set<string>();
    // Renaming changes references to this cell, not its own SQL or an existing stale state.
    // The result view also checks compiled SQL, so changed/invalid expansions still become stale.
    if (!executionChanged) changed.delete(cell.id);
    change({ ...notebook, cells: notebook.cells.map((item) => (item.id === cell.id ? cell : item)) }, changed);
  }

  function invalidateAll() {
    setResults((previous) =>
      Object.fromEntries(Object.entries(previous).map(([id, result]) => [id, { ...result, stale: true }])),
    );
  }

  async function execute(cellId: string | null, bulk: boolean) {
    if (runLock.current || switchingLock.current || !connection || analyzing || !bridge.desktop) return;
    runLock.current = true;
    setBusy(true);
    setNotice(null);
    setProgress(null);
    changedDuringRun.current = new Set();
    try {
      activeExecution.current = bridge.run(notebookId, notebook, cellId, bulk);
      const result = await activeExecution.current;
      if (result.loggingError) setLoggingError(result.loggingError);
      const mutation = !bulk && notebook.cells.find((cell) => cell.id === cellId)?.kind === 'mutation';
      if (mutation && !result.cancelled && result.results.some((cell) => !cell.error)) invalidateAll();
      setResults((previous) => acceptResults(previous, result, changedDuringRun.current));
      const failures = result.results.filter((cell) => cell.error).length;
      setNotice(
        result.cancelled
          ? 'Run cancelled. Unfinished cells retain their previous results.'
          : `${result.results.length} cell${result.results.length === 1 ? '' : 's'} finished${failures ? ` · ${failures} failed` : ''}${result.skipped.length ? ` · ${result.skipped.length} mutation${result.skipped.length === 1 ? '' : 's'} skipped` : ''}.`,
      );
    } catch (error) {
      setNotice(message(error));
    } finally {
      setBusy(false);
      setProgress(null);
      runLock.current = false;
      activeExecution.current = null;
    }
  }

  async function openNotebook(id: string): Promise<Bootstrap | null> {
    if (id === notebookId || switchingLock.current || runLock.current || autoConnecting || closing.current) return null;
    switchingLock.current = true;
    setSwitching(true);
    try {
      await saveQueue.current.catch(() => {});
      await bridge.save(notebookId, latestNotebook.current);
      setSaveState('Saved locally');
      return await bridge.openNotebook(notebookId, id);
    } catch (error) {
      switchingLock.current = false;
      setSwitching(false);
      setNotice(message(error));
      return null;
    }
  }

  return {
    notebook,
    analyses,
    analyzing,
    results,
    busy,
    switching,
    openNotebook,
    progress,
    notice,
    saveState,
    connection,
    connectionInput,
    autoConnecting,
    loggingError,
    editCell,
    change,
    execute,
    log,
    setNotice,
    dismissNotice: () => setNotice(null),
    connected: (label: string, input: ConnectionInput) => {
      setConnection(label);
      setConnectionInput(input);
      setNotice(null);
      invalidateAll();
    },
    cancel: () => {
      void bridge.cancel(notebookId).catch((error: unknown) => setNotice(message(error)));
    },
    fail: (error: unknown) => setNotice(message(error)),
  };
}
