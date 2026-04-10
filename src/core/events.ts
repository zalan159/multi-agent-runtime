import type { TaskDispatch, WorkspaceSpec } from './types.js';

export interface BaseWorkspaceEvent {
  timestamp: string;
  workspaceId: string;
}

export interface WorkspaceStartedEvent extends BaseWorkspaceEvent {
  type: 'workspace.started';
  spec: WorkspaceSpec;
}

export interface WorkspaceInitializedEvent extends BaseWorkspaceEvent {
  type: 'workspace.initialized';
  sessionId?: string;
  availableAgents: string[];
  availableTools: string[];
  availableCommands?: string[];
}

export interface WorkspaceStateChangedEvent extends BaseWorkspaceEvent {
  type: 'workspace.state.changed';
  state: 'idle' | 'running' | 'requires_action' | 'closed';
}

export interface WorkspaceMessageEvent extends BaseWorkspaceEvent {
  type: 'message';
  role: 'user' | 'assistant' | 'system';
  text: string;
  sessionId?: string;
  parentToolUseId?: string | null;
  raw: unknown;
}

export interface DispatchQueuedEvent extends BaseWorkspaceEvent {
  type: 'dispatch.queued';
  dispatch: TaskDispatch;
}

export interface DispatchStartedEvent extends BaseWorkspaceEvent {
  type: 'dispatch.started';
  dispatch: TaskDispatch;
  taskId: string;
  description: string;
}

export interface DispatchProgressEvent extends BaseWorkspaceEvent {
  type: 'dispatch.progress';
  dispatch: TaskDispatch;
  taskId: string;
  description: string;
  summary?: string;
  lastToolName?: string;
}

export interface DispatchCompletedEvent extends BaseWorkspaceEvent {
  type: 'dispatch.completed' | 'dispatch.failed' | 'dispatch.stopped';
  dispatch: TaskDispatch;
  taskId: string;
  outputFile: string;
  summary: string;
}

export interface DispatchResultEvent extends BaseWorkspaceEvent {
  type: 'dispatch.result';
  dispatch: TaskDispatch;
  taskId: string;
  resultText: string;
}

export interface ToolProgressEvent extends BaseWorkspaceEvent {
  type: 'tool.progress';
  taskId?: string;
  toolName: string;
  elapsedTimeSeconds: number;
}

export interface ResultEvent extends BaseWorkspaceEvent {
  type: 'result';
  subtype: string;
  result?: string;
  isError: boolean;
  sessionId: string;
  raw: unknown;
}

export interface ErrorEvent extends BaseWorkspaceEvent {
  type: 'error';
  error: Error;
}

export type WorkspaceEvent =
  | WorkspaceStartedEvent
  | WorkspaceInitializedEvent
  | WorkspaceStateChangedEvent
  | WorkspaceMessageEvent
  | DispatchQueuedEvent
  | DispatchStartedEvent
  | DispatchProgressEvent
  | DispatchCompletedEvent
  | DispatchResultEvent
  | ToolProgressEvent
  | ResultEvent
  | ErrorEvent;
