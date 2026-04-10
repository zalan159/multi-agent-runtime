import { randomUUID } from 'node:crypto';

import {
  Codex,
  type Thread,
  type ThreadEvent,
  type ThreadItem,
  type ApprovalMode as CodexApprovalMode,
  type ModelReasoningEffort,
  type SandboxMode as CodexSandboxMode,
  type WebSearchMode,
} from '@openai/codex-sdk';

import type {
  DispatchCompletedEvent,
  DispatchProgressEvent,
  DispatchResultEvent,
  ToolProgressEvent,
  WorkspaceInitializedEvent,
  WorkspaceMessageEvent,
  WorkspaceStateChangedEvent,
} from '../../core/events.js';
import { WorkspaceRuntime } from '../../core/runtime.js';
import type {
  RoleSpec,
  RoleTaskRequest,
  TaskDispatch,
  WorkspaceSpec,
  WorkspaceState,
} from '../../core/types.js';

export interface CodexSdkWorkspaceOptions {
  spec: WorkspaceSpec;
  codexPathOverride?: string;
  baseUrl?: string;
  apiKey?: string;
  env?: Record<string, string>;
  config?: NonNullable<ConstructorParameters<typeof Codex>[0]>['config'];
  sandboxMode?: CodexSandboxMode;
  approvalPolicy?: CodexApprovalMode;
  workingDirectory?: string;
  skipGitRepoCheck?: boolean;
  modelReasoningEffort?: ModelReasoningEffort;
  networkAccessEnabled?: boolean;
  webSearchMode?: WebSearchMode;
  additionalDirectories?: string[];
  debug?: boolean;
}

export class CodexSdkWorkspace extends WorkspaceRuntime {
  private readonly spec: WorkspaceSpec;
  private readonly sandboxMode: CodexSandboxMode | undefined;
  private readonly approvalPolicy: CodexApprovalMode | undefined;
  private readonly workingDirectory: string | undefined;
  private readonly skipGitRepoCheck: boolean;
  private readonly modelReasoningEffort: ModelReasoningEffort | undefined;
  private readonly networkAccessEnabled: boolean | undefined;
  private readonly webSearchMode: WebSearchMode | undefined;
  private readonly additionalDirectories: string[] | undefined;
  private readonly debug: boolean;
  private readonly client: Codex;

  private readonly state: WorkspaceState;
  private readonly roleThreads = new Map<string, Thread>();
  private readonly roleChains = new Map<string, Promise<void>>();
  private readonly activeRuns = new Map<
    string,
    {
      controller: AbortController;
      threadId?: string;
      roleId: string;
    }
  >();

  private active = false;
  private initialized = false;
  private initializedWithThread = false;

  constructor(options: CodexSdkWorkspaceOptions) {
    super();
    this.spec = options.spec;
    this.sandboxMode = options.sandboxMode;
    this.approvalPolicy = options.approvalPolicy;
    this.workingDirectory = options.workingDirectory ?? this.spec.cwd;
    this.skipGitRepoCheck = options.skipGitRepoCheck ?? true;
    this.modelReasoningEffort = options.modelReasoningEffort;
    this.networkAccessEnabled = options.networkAccessEnabled;
    this.webSearchMode = options.webSearchMode;
    this.additionalDirectories = options.additionalDirectories;
    this.debug = options.debug ?? false;
    this.client = new Codex({
      ...(options.codexPathOverride
        ? { codexPathOverride: options.codexPathOverride }
        : {}),
      ...(options.baseUrl ? { baseUrl: options.baseUrl } : {}),
      ...(options.apiKey ? { apiKey: options.apiKey } : {}),
      ...(options.env ? { env: options.env } : {}),
      ...(options.config ? { config: options.config } : {}),
    });

    this.state = {
      workspaceId: this.spec.id,
      status: 'idle',
      provider: 'codex-sdk',
      roles: Object.fromEntries(this.spec.roles.map(role => [role.id, role])),
      dispatches: {},
    };
  }

  getSnapshot(): WorkspaceState {
    return {
      ...this.state,
      roles: { ...this.state.roles },
      dispatches: { ...this.state.dispatches },
    };
  }

  async start(): Promise<void> {
    if (this.active) {
      return;
    }

    this.active = true;
    this.state.status = 'running';
    this.state.startedAt = new Date().toISOString();

    this.emitEvent({
      type: 'workspace.started',
      timestamp: new Date().toISOString(),
      workspaceId: this.spec.id,
      spec: this.spec,
    });

    this.emitInitialized({});
    this.emitStateChanged('running');
  }

  async assignRoleTask(request: RoleTaskRequest): Promise<TaskDispatch> {
    this.ensureStarted();

    const role = this.state.roles[request.roleId];
    if (!role) {
      throw new Error(`Unknown role: ${request.roleId}`);
    }

    const dispatch: TaskDispatch = {
      dispatchId: randomUUID(),
      workspaceId: this.spec.id,
      roleId: role.id,
      instruction: request.instruction,
      status: 'queued',
      createdAt: new Date().toISOString(),
      ...(request.summary ? { summary: request.summary } : {}),
    };

    this.state.dispatches[dispatch.dispatchId] = dispatch;

    this.emitEvent({
      type: 'dispatch.queued',
      timestamp: new Date().toISOString(),
      workspaceId: this.spec.id,
      dispatch: { ...dispatch },
    });

    const previous = this.roleChains.get(role.id) ?? Promise.resolve();
    const current = previous
      .catch(() => undefined)
      .then(async () => this.executeDispatch(role, dispatch));
    this.roleChains.set(role.id, current);
    void current.finally(() => {
      if (this.roleChains.get(role.id) === current) {
        this.roleChains.delete(role.id);
      }
    });

    return { ...dispatch };
  }

  async runRoleTask(
    request: RoleTaskRequest,
    options: { timeoutMs?: number; resultTimeoutMs?: number } = {},
  ): Promise<TaskDispatch> {
    return this.runDispatch(this.assignRoleTask(request), options);
  }

  async stopTask(taskId: string): Promise<void> {
    const activeRun = [...this.activeRuns.entries()].find(([dispatchId, run]) => {
      return run.threadId === taskId || dispatchId === taskId;
    });
    activeRun?.[1].controller.abort();
  }

  async close(): Promise<void> {
    for (const run of this.activeRuns.values()) {
      run.controller.abort();
    }
    this.activeRuns.clear();
    this.roleChains.clear();
    this.active = false;
    this.state.status = 'closed';
    this.emitStateChanged('closed');
  }

  private async executeDispatch(role: RoleSpec, dispatch: TaskDispatch): Promise<void> {
    const thread = this.ensureRoleThread(role);
    const controller = new AbortController();
    this.activeRuns.set(dispatch.dispatchId, {
      controller,
      ...(thread.id ? { threadId: thread.id } : {}),
      roleId: role.id,
    });

    let completed = false;
    let resultText: string | undefined;
    const commandStartedAt = new Map<string, number>();

    try {
      const { events } = await thread.runStreamed(
        this.buildDispatchPrompt(role, dispatch),
        {
          signal: controller.signal,
        },
      );

      for await (const event of events) {
        if (event.type === 'thread.started') {
          this.roleThreads.set(role.id, thread);
          this.activeRuns.get(dispatch.dispatchId)!.threadId = event.thread_id;
          this.state.sessionId = event.thread_id;

          const nextDispatch = this.mustGetDispatch(dispatch.dispatchId);
          nextDispatch.status = 'started';
          nextDispatch.providerTaskId = event.thread_id;
          nextDispatch.toolUseId = `codex-thread:${event.thread_id}`;
          nextDispatch.startedAt = new Date().toISOString();
          nextDispatch.lastSummary = dispatch.summary ?? dispatch.instruction;

          if (!this.initializedWithThread) {
            this.initializedWithThread = true;
            this.emitInitialized({ sessionId: event.thread_id });
          }

          this.emitEvent({
            type: 'dispatch.started',
            timestamp: new Date().toISOString(),
            workspaceId: this.spec.id,
            dispatch: this.cloneDispatch(nextDispatch),
            taskId: event.thread_id,
            description: dispatch.summary ?? dispatch.instruction,
          });
          continue;
        }

        if (event.type === 'item.started') {
          this.handleItemStarted(dispatch.dispatchId, event.item, commandStartedAt);
          continue;
        }

        if (event.type === 'item.updated') {
          this.handleItemUpdated(dispatch.dispatchId, event.item);
          continue;
        }

        if (event.type === 'item.completed') {
          const itemResult = this.handleItemCompleted(
            dispatch.dispatchId,
            event.item,
            commandStartedAt,
          );
          if (itemResult?.resultText) {
            resultText = itemResult.resultText;
          }
          continue;
        }

        if (event.type === 'turn.completed') {
          completed = true;
          const threadId = thread.id ?? dispatch.dispatchId;
          this.finishDispatch(
            dispatch.dispatchId,
            threadId,
            buildFinishResult(
              'completed',
              resultText ? 'Codex completed the task.' : 'Codex finished the turn.',
              resultText,
            ),
          );
          continue;
        }

        if (event.type === 'turn.failed') {
          completed = true;
          const threadId = thread.id ?? dispatch.dispatchId;
          this.finishDispatch(
            dispatch.dispatchId,
            threadId,
            buildFinishResult('failed', event.error.message, resultText),
          );
          continue;
        }

        if (event.type === 'error') {
          this.emitEvent({
            type: 'error',
            timestamp: new Date().toISOString(),
            workspaceId: this.spec.id,
            error: new Error(event.message),
          });
        }
      }
    } catch (error) {
      if (!completed) {
        const threadId = thread.id ?? dispatch.dispatchId;
        this.finishDispatch(
          dispatch.dispatchId,
          threadId,
          buildFinishResult(
            controller.signal.aborted ? 'stopped' : 'failed',
            error instanceof Error ? error.message : String(error),
            resultText,
          ),
        );
        this.emitEvent({
          type: 'error',
          timestamp: new Date().toISOString(),
          workspaceId: this.spec.id,
          error: error instanceof Error ? error : new Error(String(error)),
        });
      }
    } finally {
      this.activeRuns.delete(dispatch.dispatchId);
    }
  }

  private ensureRoleThread(role: RoleSpec): Thread {
    const existing = this.roleThreads.get(role.id);
    if (existing) {
      return existing;
    }

    const thread = this.client.startThread({
      model: role.agent.model ?? this.spec.model,
      ...(this.sandboxMode ? { sandboxMode: this.sandboxMode } : {}),
      ...(this.workingDirectory
        ? { workingDirectory: this.workingDirectory }
        : {}),
      skipGitRepoCheck: this.skipGitRepoCheck,
      ...(this.modelReasoningEffort
        ? { modelReasoningEffort: this.modelReasoningEffort }
        : {}),
      ...(this.networkAccessEnabled !== undefined
        ? { networkAccessEnabled: this.networkAccessEnabled }
        : {}),
      ...(this.webSearchMode ? { webSearchMode: this.webSearchMode } : {}),
      ...(this.approvalPolicy ? { approvalPolicy: this.approvalPolicy } : {}),
      ...(this.additionalDirectories
        ? { additionalDirectories: this.additionalDirectories }
        : {}),
    });
    this.roleThreads.set(role.id, thread);
    return thread;
  }

  private handleItemStarted(
    dispatchId: string,
    item: ThreadItem,
    commandStartedAt: Map<string, number>,
  ): void {
    if (item.type === 'command_execution') {
      commandStartedAt.set(item.id, Date.now());
      this.emitDispatchProgress(dispatchId, {
        description: item.command,
        summary: 'Codex is executing a shell command.',
        lastToolName: 'Bash',
      });
      return;
    }

    if (item.type === 'web_search') {
      this.emitDispatchProgress(dispatchId, {
        description: item.query,
        summary: 'Codex started a web search.',
        lastToolName: 'WebSearch',
      });
      return;
    }

    if (item.type === 'mcp_tool_call') {
      this.emitDispatchProgress(dispatchId, {
        description: `${item.server}.${item.tool}`,
        summary: 'Codex started an MCP tool call.',
        lastToolName: item.tool,
      });
    }
  }

  private handleItemUpdated(dispatchId: string, item: ThreadItem): void {
    if (item.type === 'todo_list') {
      const incomplete = item.items.filter(todo => !todo.completed).length;
      this.emitDispatchProgress(dispatchId, {
        description: 'todo_list',
        summary: `Codex is tracking ${incomplete} remaining todo item(s).`,
        lastToolName: 'TodoList',
      });
      return;
    }

    if (item.type === 'reasoning') {
      this.emitDispatchProgress(dispatchId, {
        description: 'reasoning',
        summary: item.text,
        lastToolName: 'Reasoning',
      });
    }
  }

  private handleItemCompleted(
    dispatchId: string,
    item: ThreadItem,
    commandStartedAt: Map<string, number>,
  ): { resultText?: string } | undefined {
    if (item.type === 'command_execution') {
      const startedAt = commandStartedAt.get(item.id) ?? Date.now();
      const event: ToolProgressEvent = {
        type: 'tool.progress',
        timestamp: new Date().toISOString(),
        workspaceId: this.spec.id,
        taskId: this.mustGetDispatch(dispatchId).providerTaskId ?? dispatchId,
        toolName: 'Bash',
        elapsedTimeSeconds: Math.max(
          0,
          Math.round((Date.now() - startedAt) / 1000),
        ),
      };
      this.emitEvent(event);
      this.emitDispatchProgress(dispatchId, {
        description: item.command,
        summary:
          item.exit_code === 0
            ? 'Codex completed a shell command.'
            : `Codex command exited with code ${item.exit_code}.`,
        lastToolName: 'Bash',
      });
      return undefined;
    }

    if (item.type === 'agent_message') {
      const dispatch = this.mustGetDispatch(dispatchId);
      if (!dispatch.toolUseId) {
        dispatch.toolUseId = item.id;
      }

      const messageEvent: WorkspaceMessageEvent = {
        type: 'message',
        timestamp: new Date().toISOString(),
        workspaceId: this.spec.id,
        role: 'assistant',
        text: item.text,
        raw: item,
        ...(dispatch.providerTaskId ? { sessionId: dispatch.providerTaskId } : {}),
        ...(dispatch.toolUseId ? { parentToolUseId: dispatch.toolUseId } : {}),
      };
      this.emitEvent(messageEvent);
      return { resultText: item.text };
    }

    if (item.type === 'file_change') {
      const changedPaths = item.changes.map(change => change.path).join(', ');
      this.emitDispatchProgress(dispatchId, {
        description: changedPaths || 'file changes',
        summary: 'Codex applied file changes.',
        lastToolName: 'ApplyPatch',
      });
      return undefined;
    }

    if (item.type === 'mcp_tool_call') {
      this.emitDispatchProgress(dispatchId, {
        description: `${item.server}.${item.tool}`,
        summary:
          item.status === 'failed'
            ? item.error?.message ?? 'Codex MCP call failed.'
            : 'Codex completed an MCP tool call.',
        lastToolName: item.tool,
      });
      return undefined;
    }

    if (item.type === 'web_search') {
      this.emitDispatchProgress(dispatchId, {
        description: item.query,
        summary: 'Codex completed a web search.',
        lastToolName: 'WebSearch',
      });
      return undefined;
    }

    if (item.type === 'error') {
      this.emitEvent({
        type: 'error',
        timestamp: new Date().toISOString(),
        workspaceId: this.spec.id,
        error: new Error(item.message),
      });
    }

    return undefined;
  }

  private emitDispatchProgress(
    dispatchId: string,
    progress: {
      description: string;
      summary?: string;
      lastToolName?: string;
    },
  ): void {
    const dispatch = this.mustGetDispatch(dispatchId);
    dispatch.status = 'running';
    if (progress.summary) {
      dispatch.lastSummary = progress.summary;
    }

    const event: DispatchProgressEvent = {
      type: 'dispatch.progress',
      timestamp: new Date().toISOString(),
      workspaceId: this.spec.id,
      dispatch: this.cloneDispatch(dispatch),
      taskId: dispatch.providerTaskId ?? dispatch.dispatchId,
      description: progress.description,
      ...(progress.summary ? { summary: progress.summary } : {}),
      ...(progress.lastToolName ? { lastToolName: progress.lastToolName } : {}),
    };
    this.emitEvent(event);
  }

  private finishDispatch(
    dispatchId: string,
    taskId: string,
    result: {
      status: 'completed' | 'failed' | 'stopped';
      summary: string;
      resultText?: string;
      outputFile?: string;
    },
  ): void {
    const dispatch = this.mustGetDispatch(dispatchId);
    dispatch.status = result.status;
    dispatch.completedAt = new Date().toISOString();
    dispatch.lastSummary = result.summary;
    if (result.outputFile) {
      dispatch.outputFile = result.outputFile;
    }
    if (result.resultText) {
      dispatch.resultText = result.resultText;
    }

    const event: DispatchCompletedEvent = {
      type:
        result.status === 'completed'
          ? 'dispatch.completed'
          : result.status === 'stopped'
            ? 'dispatch.stopped'
            : 'dispatch.failed',
      timestamp: new Date().toISOString(),
      workspaceId: this.spec.id,
      dispatch: this.cloneDispatch(dispatch),
      taskId,
      outputFile: result.outputFile ?? '',
      summary: result.summary,
    };
    this.emitEvent(event);

    if (result.resultText) {
      const resultEvent: DispatchResultEvent = {
        type: 'dispatch.result',
        timestamp: new Date().toISOString(),
        workspaceId: this.spec.id,
        dispatch: this.cloneDispatch(dispatch),
        taskId,
        resultText: result.resultText,
      };
      this.emitEvent(resultEvent);
    }
  }

  private buildDispatchPrompt(role: RoleSpec, dispatch: TaskDispatch): string {
    const parts = [
      `You are the ${role.name} role in the workspace "${this.spec.name}".`,
      role.description ? `Role description: ${role.description}` : null,
      `Follow this role-specific instruction set strictly:\n${role.agent.prompt}`,
      this.spec.orchestratorPrompt
        ? `Workspace orchestration context:\n${this.spec.orchestratorPrompt}`
        : null,
      role.outputRoot
        ? `Preferred output root for this role: ${role.outputRoot}`
        : null,
      dispatch.summary ? `Task summary: ${dispatch.summary}` : null,
      `Task instruction:\n${dispatch.instruction}`,
      'Return a concise final answer after completing the task. If you create or edit files, mention the key output paths in the final answer.',
    ];

    return parts.filter(Boolean).join('\n\n');
  }

  private emitInitialized({ sessionId }: { sessionId?: string }): void {
    const event: WorkspaceInitializedEvent = {
      type: 'workspace.initialized',
      timestamp: new Date().toISOString(),
      workspaceId: this.spec.id,
      availableAgents: this.spec.roles.map(role => role.id),
      availableTools: this.spec.allowedTools ?? [],
      availableCommands: ['run', 'runStreamed', 'resumeThread'],
      ...(sessionId ? { sessionId } : {}),
    };
    this.initialized = true;
    this.emitEvent(event);
  }

  private emitStateChanged(state: WorkspaceStateChangedEvent['state']): void {
    const event: WorkspaceStateChangedEvent = {
      type: 'workspace.state.changed',
      timestamp: new Date().toISOString(),
      workspaceId: this.spec.id,
      state,
    };
    this.emitEvent(event);
  }

  private ensureStarted(): void {
    if (!this.active || !this.initialized) {
      throw new Error('Workspace has not been started.');
    }
  }

  private mustGetDispatch(dispatchId: string): TaskDispatch {
    const dispatch = this.state.dispatches[dispatchId];
    if (!dispatch) {
      throw new Error(`Unknown dispatch: ${dispatchId}`);
    }
    return dispatch;
  }

  private cloneDispatch(dispatch: TaskDispatch): TaskDispatch {
    return {
      ...dispatch,
    };
  }
}

function buildFinishResult(
  status: 'completed' | 'failed' | 'stopped',
  summary: string,
  resultText?: string,
): {
  status: 'completed' | 'failed' | 'stopped';
  summary: string;
  resultText?: string;
} {
  return resultText === undefined
    ? { status, summary }
    : { status, summary, resultText };
}
