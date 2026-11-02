import { invoke, isTauri } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { getCurrentWindow } from '@tauri-apps/api/window';
import { initialConnection } from './connection';
import type { Analysis, Bootstrap, ConnectionInput, Notebook, NotebookSummary, Progress, RunResult } from './model';

export const desktop = isTauri();

export async function bootstrap(): Promise<Bootstrap> {
  const input = initialConnection();
  if (!desktop) {
    return {
      notebook: null,
      notebookId: 'browser-preview',
      sessionId: 'browser-preview',
      logPath: '',
      dataDirectory: '',
      connectionDefaults: { input, autoConnect: false, error: null },
    };
  }
  return invoke<Bootstrap>('bootstrap');
}

export const analyze = (notebook: Notebook) => invoke<Analysis[]>('analyze_notebook', { notebook });
export const save = (notebookId: string, notebook: Notebook) => invoke<void>('save_notebook', { notebookId, notebook });
export const connect = (notebookId: string, input: ConnectionInput) =>
  invoke<string>('connect_database', { notebookId, input });
export const connectEnvironment = (notebookId: string) => invoke<string>('connect_environment', { notebookId });
export const run = (notebookId: string, notebook: Notebook, cellId: string | null, bulk: boolean) =>
  invoke<RunResult>('run_cells', { notebookId, request: { notebook, cellId, bulk } });
export const cancel = (notebookId: string) => invoke<void>('cancel_run', { notebookId });
export const sessionLog = (notebookId: string) => invoke<string>('read_session_log', { notebookId });
export const listNotebooks = () => invoke<NotebookSummary[]>('list_notebooks');
export const openNotebook = (notebookId: string, id: string) => invoke<Bootstrap>('open_notebook', { notebookId, id });
export const createNotebook = (notebookId: string) => invoke<Bootstrap>('create_notebook', { notebookId });
export const exportFile = (name: string, contents: string) => invoke<boolean>('export_file', { name, contents });
export const record = (notebookId: string, event: string, data: Record<string, unknown>) =>
  desktop ? invoke<void>('record_event', { notebookId, event, data }) : Promise.resolve();
export const subscribeProgress = (callback: (progress: Progress) => void) =>
  listen<Progress>('run-progress', ({ payload }) => callback(payload));

export function subscribeClose(flush: () => Promise<void>, fail: (error: unknown) => void) {
  let active = true;
  const subscription = getCurrentWindow().onCloseRequested((event) => {
    if (!active) return;
    event.preventDefault();
    void flush()
      .then(() => {
        if (active) return getCurrentWindow().destroy();
      })
      .catch((error: unknown) => {
        if (active) fail(error);
      });
  });
  void subscription.catch((error: unknown) => {
    if (active) fail(error);
  });
  return () => {
    active = false;
    void subscription.then((unlisten) => unlisten()).catch(() => {});
  };
}
