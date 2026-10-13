import { useEffect, useRef, useState } from 'react';
import * as bridge from './bridge';
import { acceptResults, affectedIds } from './model';
import type {
  Analysis,
  Cell,
  ConnectionDefaults,
  ConnectionInput,
  DisplayResult,
  Notebook,
  Progress,
  RunResult,
} from './model';

const message = (error: unknown) => (error instanceof Error ? error.message : String(error));

export function useNotebook(initial: Notebook, defaults: ConnectionDefaults) {
  const [notebook, setNotebook] = useState(initial);
  const [analyses, setAnalyses] = useState<Analysis[]>([]);
  const [analyzing, setAnalyzing] = useState(bridge.desktop);
  const [results, setResults] = useState<Record<string, DisplayResult>>({});
  const [busy, setBusy] = useState(false);
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
    automaticConnection.current ??= bridge.connectEnvironment();
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
    void bridge.record(event, data).catch((error: unknown) => setLoggingError(message(error)));
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
    const subscription = bridge.subscribeClose(
      async () => {
        closing.current = true;
        if (activeExecution.current) {
          await bridge.cancel();
          await activeExecution.current.catch(() => {});
        }
        await saveQueue.current.catch(() => {});
        await bridge.save(latestNotebook.current);
        await bridge.record('window_close_requested', {});
      },
      (error) => {
        closing.current = false;
        setLoggingError(message(error));
      },
    );
    void subscription.catch((error: unknown) => setNotice(message(error)));
    return () => {
      void subscription.then((unlisten) => unlisten()).catch(() => {});
    };
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
      if (closing.current) return;
      saveQueue.current = saveQueue.current.catch(() => {}).then(() => bridge.save(notebook));
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
    const sqlChanged = previous?.sql !== cell.sql || previous.name !== cell.name || previous.kind !== cell.kind;
    change(
      { ...notebook, cells: notebook.cells.map((item) => (item.id === cell.id ? cell : item)) },
      sqlChanged ? affectedIds(notebook, analyses, cell.id) : new Set(),
    );
  }

  function invalidateAll() {
    setResults((previous) =>
      Object.fromEntries(Object.entries(previous).map(([id, result]) => [id, { ...result, stale: true }])),
    );
  }

  async function execute(cellId: string | null, bulk: boolean) {
    if (runLock.current || !connection || analyzing || !bridge.desktop) return;
    runLock.current = true;
    setBusy(true);
    setNotice(null);
    setProgress(null);
    changedDuringRun.current = new Set();
    try {
      activeExecution.current = bridge.run(notebook, cellId, bulk);
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

  return {
    notebook,
    analyses,
    analyzing,
    results,
    busy,
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
      void bridge.cancel().catch((error: unknown) => setNotice(message(error)));
    },
    fail: (error: unknown) => setNotice(message(error)),
  };
}
