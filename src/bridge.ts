import { invoke, isTauri } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { getCurrentWindow } from '@tauri-apps/api/window';
import { initialConnection } from './connection';
import type { Analysis, Bootstrap, ConnectionDefaults, ConnectionInput, Notebook, Progress, RunResult } from './model';

export const desktop = isTauri();

export async function bootstrap(): Promise<Bootstrap> {
  const input = initialConnection();
  if (!desktop) {
    return {
      notebook: null,
      sessionId: 'browser-preview',
      logPath: '',
      dataDirectory: '',
      connectionDefaults: { input, autoConnect: false, error: null },
    };
  }
  const [initial, connectionDefaults] = await Promise.all([
    invoke<Omit<Bootstrap, 'connectionDefaults'>>('bootstrap'),
    invoke<ConnectionDefaults>('connection_defaults'),
  ]);
  return { ...initial, connectionDefaults };
}

export const analyze = (notebook: Notebook) => invoke<Analysis[]>('analyze_notebook', { notebook });
export const save = (notebook: Notebook) => invoke<void>('save_notebook', { notebook });
export const connect = (input: ConnectionInput) => invoke<string>('connect_database', { input });
export const connectEnvironment = () => invoke<string>('connect_environment');
export const run = (notebook: Notebook, cellId: string | null, bulk: boolean) =>
  invoke<RunResult>('run_cells', { request: { notebook, cellId, bulk } });
export const cancel = () => invoke<void>('cancel_run');
export const sessionLog = () => invoke<string>('read_session_log');
export const exportFile = (name: string, contents: string) => invoke<boolean>('export_file', { name, contents });
export const record = (event: string, data: Record<string, unknown>) =>
  desktop ? invoke<void>('record_event', { event, data }) : Promise.resolve();
export const subscribeProgress = (callback: (progress: Progress) => void) =>
  listen<Progress>('run-progress', ({ payload }) => callback(payload));

export const subscribeClose = (flush: () => Promise<void>, fail: (error: unknown) => void) =>
  getCurrentWindow().onCloseRequested((event) => {
    event.preventDefault();
    void flush()
      .then(() => getCurrentWindow().destroy())
      .catch(fail);
  });
