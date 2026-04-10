import { EventEmitter } from 'node:events';

import type { WorkspaceEvent } from './events.js';
import type { TaskDispatch } from './types.js';

export abstract class WorkspaceRuntime extends EventEmitter {
  protected emitEvent(event: WorkspaceEvent): void {
    this.emit('event', event);
  }

  onEvent(listener: (event: WorkspaceEvent) => void): () => void {
    this.on('event', listener);
    return () => this.off('event', listener);
  }

  waitForEvent<TEvent extends WorkspaceEvent = WorkspaceEvent>(
    predicate: (event: WorkspaceEvent) => event is TEvent,
    options: { timeoutMs?: number } = {},
  ): Promise<TEvent> {
    const timeoutMs = options.timeoutMs ?? 120_000;

    return new Promise<TEvent>((resolve, reject) => {
      let timeout: NodeJS.Timeout | undefined;

      const cleanup = () => {
        this.off('event', onEvent);
        if (timeout) {
          clearTimeout(timeout);
        }
      };

      const onEvent = (event: WorkspaceEvent) => {
        if (!predicate(event)) {
          return;
        }

        cleanup();
        resolve(event);
      };

      this.on('event', onEvent);

      timeout = setTimeout(() => {
        cleanup();
        reject(new Error(`Timed out after ${timeoutMs}ms waiting for workspace event.`));
      }, timeoutMs);
    });
  }

  waitForDispatchTerminal(
    dispatchId: string,
    options: { timeoutMs?: number } = {},
  ): Promise<
    Extract<WorkspaceEvent, { type: 'dispatch.completed' | 'dispatch.failed' | 'dispatch.stopped' }>
  > {
    return this.waitForEvent(
      (
        event,
      ): event is Extract<
        WorkspaceEvent,
        { type: 'dispatch.completed' | 'dispatch.failed' | 'dispatch.stopped' }
      > =>
        (event.type === 'dispatch.completed' ||
          event.type === 'dispatch.failed' ||
          event.type === 'dispatch.stopped') &&
        event.dispatch.dispatchId === dispatchId,
      options,
    );
  }

  waitForDispatchResult(
    dispatchId: string,
    options: { timeoutMs?: number } = {},
  ): Promise<Extract<WorkspaceEvent, { type: 'dispatch.result' }>> {
    return this.waitForEvent(
      (event): event is Extract<WorkspaceEvent, { type: 'dispatch.result' }> =>
        event.type === 'dispatch.result' && event.dispatch.dispatchId === dispatchId,
      options,
    );
  }

  async runDispatch<TDispatch extends TaskDispatch>(
    dispatchPromise: Promise<TDispatch>,
    options: { timeoutMs?: number; resultTimeoutMs?: number } = {},
  ): Promise<TDispatch> {
    const dispatch = await dispatchPromise;
    const terminal = await this.waitForDispatchTerminal(
      dispatch.dispatchId,
      options.timeoutMs !== undefined ? { timeoutMs: options.timeoutMs } : {},
    );

    try {
      const result = await this.waitForDispatchResult(dispatch.dispatchId, {
        timeoutMs: options.resultTimeoutMs ?? 10_000,
      });
      return { ...result.dispatch } as TDispatch;
    } catch {
      return { ...terminal.dispatch } as TDispatch;
    }
  }
}
