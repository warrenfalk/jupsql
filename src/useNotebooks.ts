import { useEffect, useState } from 'react';
import * as bridge from './bridge';
import type { NotebookSummary } from './model';

export function useNotebooks(notebookId: string) {
  const [notebooks, setNotebooks] = useState<NotebookSummary[]>([]);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!bridge.desktop) return;
    let active = true;
    let refreshing = false;
    async function refresh() {
      if (refreshing) return;
      refreshing = true;
      try {
        const next = await bridge.listNotebooks();
        if (active) {
          setNotebooks(next);
          setError(null);
        }
      } catch (error) {
        if (active) setError(error instanceof Error ? error.message : String(error));
      } finally {
        refreshing = false;
      }
    }
    void refresh();
    const timer = setInterval(() => void refresh(), 2000);
    window.addEventListener('focus', refresh);
    return () => {
      active = false;
      clearInterval(timer);
      window.removeEventListener('focus', refresh);
    };
  }, [notebookId]);

  return { notebooks, error };
}
