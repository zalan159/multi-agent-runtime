import { randomUUID } from 'node:crypto';

import {
  query as createClaudeQuery,
  type AgentDefinition,
  type Options as ClaudeOptions,
  type Query as ClaudeQuery,
  type SDKMessage,
  type SDKTaskNotificationMessage,
  type SDKTaskProgressMessage,
  type SDKTaskStartedMessage,
  type SDKToolProgressMessage,
  type SDKUserMessage,
} from '@anthropic-ai/claude-agent-sdk';

import type {
  ActivityPublishedEvent,
  DispatchClaimedEvent,
  DispatchCompletedEvent,
  DispatchResultEvent,
  DispatchProgressEvent,
  DispatchStartedEvent,
  MemberRegisteredEvent,
  MemberStateChangedEvent,
  WorkspaceInitializedEvent,
  WorkspaceMessageEvent,
  WorkspaceStateChangedEvent,
} from '../../core/events.js';
import { WorkspaceRuntime } from '../../core/runtime.js';
import type {
  ClaimStatus,
  RoleSpec,
  RoleTaskRequest,
  TaskDispatch,
  WorkspaceActivity,
  WorkspaceActivityKind,
  WorkspaceMember,
  WorkspaceSpec,
  WorkspaceState,
  WorkspaceTurnRequest,
  WorkspaceTurnResult,
  WorkspaceVisibility,
} from '../../core/types.js';
import {
  buildWorkspaceTurnPrompt,
  planWorkspaceTurnHeuristically,
  parseWorkspaceTurnPlan,
} from '../../core/workspaceTurn.js';
import { AsyncMessageQueue } from './asyncMessageQueue.js';
import { extractMessageText, normalizeAgentNames } from './messageUtils.js';

export interface ClaudeAgentWorkspaceOptions {
  spec: WorkspaceSpec;
  sessionId?: string;
  debug?: boolean;
  debugFile?: string;
  env?: Record<string, string | undefined>;
}

export class ClaudeAgentWorkspace extends WorkspaceRuntime {
  private readonly spec: WorkspaceSpec;
  private readonly requestedSessionId: string | undefined;
  private readonly debug: boolean;
  private readonly debugFile: string | undefined;
  private readonly env: Record<string, string | undefined> | undefined;
  private readonly inputQueue = new AsyncMessageQueue<SDKUserMessage>();
  private readonly pendingDispatchQueue: string[] = [];
  private readonly pendingResultDispatchQueue: string[] = [];
  private readonly pendingAssistantVisibilities: WorkspaceVisibility[] = [];
  private readonly taskToDispatch = new Map<string, string>();
  private readonly toolUseToDispatch = new Map<string, string>();
  private readonly state: WorkspaceState;

  private query?: ClaudeQuery;
  private consumeLoop?: Promise<void>;
  private active = false;
  private initialized = false;
  private initializedHadSession = false;
  private availableCommands: string[] = [];

  constructor(options: ClaudeAgentWorkspaceOptions) {
    super();
    this.spec = options.spec;
    this.requestedSessionId = options.sessionId;
    this.debug = options.debug ?? false;
    this.debugFile = options.debugFile;
    this.env = options.env;

    this.state = {
      workspaceId: this.spec.id,
      status: 'idle',
      provider: 'claude-agent-sdk',
      roles: Object.fromEntries(this.spec.roles.map(role => [role.id, role])),
      dispatches: {},
      members: Object.fromEntries(
        this.spec.roles.map(role => [
          role.id,
          {
            memberId: role.id,
            workspaceId: this.spec.id,
            roleId: role.id,
            roleName: role.name,
            ...(role.direct !== undefined ? { direct: role.direct } : {}),
            status: 'idle',
          } satisfies WorkspaceMember,
        ]),
      ),
      activities: [],
    };
  }

  getSnapshot(): WorkspaceState {
    return {
      ...this.state,
      roles: { ...this.state.roles },
      dispatches: { ...this.state.dispatches },
      members: { ...this.state.members },
      activities: [...this.state.activities],
    };
  }

  async start(): Promise<void> {
    if (this.active) {
      return;
    }

    this.emitEvent({
      type: 'workspace.started',
      timestamp: new Date().toISOString(),
      workspaceId: this.spec.id,
      spec: this.spec,
    });
    for (const member of Object.values(this.state.members)) {
      const event: MemberRegisteredEvent = {
        type: 'member.registered',
        timestamp: new Date().toISOString(),
        workspaceId: this.spec.id,
        member: { ...member },
      };
      this.emitEvent(event);
    }

    this.query = createClaudeQuery({
      prompt: this.inputQueue,
      options: this.buildClaudeOptions(),
    });
    this.consumeLoop = this.consumeMessages();

    const init = await this.query.initializationResult();
    this.availableCommands = Array.isArray(init.commands)
      ? init.commands.map(command => command.name)
      : [];
    this.active = true;
    this.state.startedAt = new Date().toISOString();
    const knownSessionId = this.state.sessionId ?? this.requestedSessionId;
    this.emitInitialized({
      availableAgents: normalizeAgentNames(init.agents),
      availableTools: [],
      ...(knownSessionId ? { sessionId: knownSessionId } : {}),
    });
  }

  async send(message: string): Promise<void> {
    this.ensureStarted();
    this.pushUserMessage(message, 'public', true);
  }

  async assignRoleTask(request: RoleTaskRequest): Promise<TaskDispatch> {
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
      ...(request.visibility ? { visibility: request.visibility } : {}),
      ...(request.sourceRoleId ? { sourceRoleId: request.sourceRoleId } : {}),
      ...(this.spec.claimPolicy?.mode !== 'claim'
        ? {
            claimStatus: 'claimed' satisfies ClaimStatus,
            claimedByMemberIds: [role.id],
          }
        : {
            claimStatus: 'pending' satisfies ClaimStatus,
          }),
    };

    this.state.dispatches[dispatch.dispatchId] = dispatch;
    this.pendingDispatchQueue.push(dispatch.dispatchId);

    this.emitEvent({
      type: 'dispatch.queued',
      timestamp: new Date().toISOString(),
      workspaceId: this.spec.id,
      dispatch: { ...dispatch },
    });
    if (dispatch.claimStatus === 'claimed') {
      const event: DispatchClaimedEvent = {
        type: 'dispatch.claimed',
        timestamp: new Date().toISOString(),
        workspaceId: this.spec.id,
        dispatch: { ...dispatch },
        member: this.state.members[role.id]!,
        claimStatus: 'claimed',
        note: 'Assigned by policy',
      };
      this.emitEvent(event);
    }

    this.pushUserMessage(
      this.buildRoleDispatchPrompt(role, dispatch),
      dispatch.visibility ?? 'coordinator',
      false,
    );
    return { ...dispatch };
  }

  async runRoleTask(
    request: RoleTaskRequest,
    options: { timeoutMs?: number; resultTimeoutMs?: number } = {},
  ): Promise<TaskDispatch> {
    return this.runDispatch(this.assignRoleTask(request), options);
  }

  async runWorkspaceTurn(
    request: WorkspaceTurnRequest,
    options: { timeoutMs?: number; resultTimeoutMs?: number } = {},
  ): Promise<WorkspaceTurnResult> {
    const coordinatorRole = this.resolveCoordinatorRole();
    if (this.spec.claimPolicy?.mode === 'claim') {
      this.recordUserMessage(request.message, request.visibility ?? 'public', true);
    } else {
      await this.send(request.message);
    }

    const plan = this.spec.claimPolicy?.mode === 'claim'
      ? planWorkspaceTurnHeuristically(this.spec, request)
      : parseWorkspaceTurnPlan(
          await this.requestCoordinatorPlan(request, options.timeoutMs),
          this.spec,
          request,
        );

    this.emitCoordinatorSummary(plan.responseText, coordinatorRole.id);

    const dispatches: TaskDispatch[] = [];
    for (const assignment of plan.assignments) {
      const dispatch = await this.assignRoleTask({
        roleId: assignment.roleId,
        instruction: assignment.instruction,
        ...(assignment.summary ? { summary: assignment.summary } : {}),
        visibility: assignment.visibility ?? request.visibility ?? 'public',
        sourceRoleId: coordinatorRole.id,
      });
      this.claimDispatch(dispatch.dispatchId, assignment.roleId, 'Claimed by coordinator routing');
      dispatches.push(
        await this.runDispatch(Promise.resolve(dispatch), options),
      );
    }

    return {
      request,
      plan,
      dispatches,
    };
  }

  async stopTask(taskId: string): Promise<void> {
    this.ensureStarted();
    await this.query!.stopTask(taskId);
  }

  async close(): Promise<void> {
    this.inputQueue.close();

    if (this.query) {
      this.query.close();
    }

    try {
      await this.consumeLoop;
    } catch {
      // Shutdown should still succeed even if the stream loop was already broken.
    }

    this.active = false;
    this.state.status = 'closed';
    const event: WorkspaceStateChangedEvent = {
      type: 'workspace.state.changed',
      timestamp: new Date().toISOString(),
      workspaceId: this.spec.id,
      state: 'closed',
    };
    this.emitEvent(event);
  }

  private buildClaudeOptions(): ClaudeOptions {
    const allowedTools = Array.from(
      new Set(['Agent', ...(this.spec.allowedTools ?? [])]),
    );

    const options: ClaudeOptions = {
      model: this.spec.model,
      allowedTools,
      tools: {
        type: 'preset',
        preset: 'claude_code',
      },
      agents: this.buildAgents(),
      ...(this.spec.cwd ? { cwd: this.spec.cwd } : {}),
      ...(this.spec.permissionMode
        ? { permissionMode: this.spec.permissionMode }
        : {}),
      ...(this.spec.disallowedTools
        ? { disallowedTools: this.spec.disallowedTools }
        : {}),
      ...(this.spec.orchestratorPrompt
        ? { systemPrompt: this.spec.orchestratorPrompt }
        : {}),
      ...(this.spec.settingSources
        ? { settingSources: this.spec.settingSources }
        : {}),
      ...(this.debug ? { debug: true } : {}),
      ...(this.debugFile ? { debugFile: this.debugFile } : {}),
      ...(this.env ? { env: this.env } : {}),
      ...(this.requestedSessionId ? { sessionId: this.requestedSessionId } : {}),
    };

    return options;
  }

  private buildAgents(): Record<string, AgentDefinition> {
    return Object.fromEntries(
      this.spec.roles.map(role => [role.id, this.toClaudeAgentDefinition(role)]),
    );
  }

  private toClaudeAgentDefinition(role: RoleSpec): AgentDefinition {
    return {
      description: role.agent.description,
      prompt: role.agent.prompt,
      ...(role.agent.tools ? { tools: role.agent.tools } : {}),
      ...(role.agent.disallowedTools
        ? { disallowedTools: role.agent.disallowedTools }
        : {}),
      ...(role.agent.model ? { model: role.agent.model } : {}),
      ...(role.agent.skills ? { skills: role.agent.skills } : {}),
      ...(role.agent.mcpServers ? { mcpServers: role.agent.mcpServers } : {}),
      ...(role.agent.initialPrompt
        ? { initialPrompt: role.agent.initialPrompt }
        : {}),
      ...(role.agent.maxTurns ? { maxTurns: role.agent.maxTurns } : {}),
      ...(role.agent.background !== undefined
        ? { background: role.agent.background }
        : {}),
      ...(role.agent.effort !== undefined ? { effort: role.agent.effort } : {}),
      ...(role.agent.permissionMode
        ? { permissionMode: role.agent.permissionMode }
        : {}),
    };
  }

  private buildRoleDispatchPrompt(role: RoleSpec, dispatch: TaskDispatch): string {
    const lines = [
      `Delegate this task to the ${role.id} agent using the Agent tool.`,
      `Do not complete the task yourself. Do not answer directly without launching the ${role.id} agent first.`,
      `Dispatch ID: ${dispatch.dispatchId}`,
      `Role: ${role.name}`,
      `Role description: ${role.agent.description}`,
    ];

    if (dispatch.summary) {
      lines.push(`Summary: ${dispatch.summary}`);
    }

    if (role.outputRoot) {
      lines.push(`Write any files under: ${role.outputRoot}`);
    }

    lines.push(
      `After the ${role.id} agent finishes, relay its result with a concise completion summary that is easy for an orchestrator to pass along.`,
      'Task:',
      dispatch.instruction,
    );

    return lines.join('\n\n');
  }

  private resolveCoordinatorRole(): RoleSpec {
    const coordinatorRoleId =
      this.spec.coordinatorRoleId ?? this.spec.defaultRoleId ?? this.spec.roles[0]?.id;
    if (!coordinatorRoleId) {
      throw new Error('Workspace has no coordinator role.');
    }
    const coordinatorRole = this.state.roles[coordinatorRoleId];
    if (!coordinatorRole) {
      throw new Error(`Unknown coordinator role: ${coordinatorRoleId}`);
    }
    return coordinatorRole;
  }

  private createUserMessage(message: string): SDKUserMessage {
    const sessionId = this.state.sessionId ?? this.requestedSessionId;

    return {
      type: 'user',
      message: {
        role: 'user',
        content: message,
      },
      parent_tool_use_id: null,
      ...(sessionId ? { session_id: sessionId } : {}),
    };
  }

  private pushUserMessage(
    message: string,
    visibility: WorkspaceVisibility,
    publishActivity: boolean,
  ): void {
    const payload = this.createUserMessage(message);
    this.inputQueue.push(payload);
    this.pendingAssistantVisibilities.push(visibility);

    this.recordUserMessage(message, visibility, publishActivity, payload);
  }

  private recordUserMessage(
    message: string,
    visibility: WorkspaceVisibility,
    publishActivity: boolean,
    payloadOverride?: SDKUserMessage,
  ): void {
    const payload = payloadOverride ?? this.createUserMessage(message);

    const event: WorkspaceMessageEvent = {
      type: 'message',
      timestamp: new Date().toISOString(),
      workspaceId: this.spec.id,
      role: 'user',
      text: message,
      visibility,
      raw: payload,
      ...(payload.session_id ? { sessionId: payload.session_id } : {}),
      ...(payload.parent_tool_use_id !== undefined
        ? { parentToolUseId: payload.parent_tool_use_id }
        : {}),
    };
    this.emitEvent(event);
    if (publishActivity) {
      this.publishActivity('user_message', message, { visibility });
    }
  }

  private ensureStarted(): void {
    if (!this.query) {
      throw new Error('Workspace has not been started yet.');
    }
  }

  private async consumeMessages(): Promise<void> {
    if (!this.query) {
      return;
    }

    try {
      for await (const message of this.query) {
        this.handleMessage(message);
      }
    } catch (error) {
      this.emitEvent({
        type: 'error',
        timestamp: new Date().toISOString(),
        workspaceId: this.spec.id,
        error: error instanceof Error ? error : new Error(String(error)),
      });
    }
  }

  private handleMessage(message: SDKMessage): void {
    switch (message.type) {
      case 'assistant':
        this.handleAssistantMessage(message);
        return;
      case 'user':
        return;
      case 'result':
        this.handleResultMessage(message);
        this.emitEvent({
          type: 'result',
          timestamp: new Date().toISOString(),
          workspaceId: this.spec.id,
          subtype: message.subtype,
          ...(message.subtype === 'success' ? { result: message.result } : {}),
          isError: message.is_error,
          sessionId: message.session_id,
          raw: message,
        });
        return;
      case 'tool_progress':
        this.handleToolProgress(message);
        return;
      case 'system':
        this.handleSystemMessage(message as Extract<SDKMessage, { type: 'system' }>);
        return;
      default:
        return;
    }
  }

  private handleResultMessage(message: Extract<SDKMessage, { type: 'result' }>): void {
    const nextDispatchId = this.pendingResultDispatchQueue.shift();
    if (!nextDispatchId) {
      return;
    }

    const dispatch = this.state.dispatches[nextDispatchId];
    if (!dispatch || message.subtype !== 'success' || typeof message.result !== 'string') {
      return;
    }

    const resultText = message.result.trim();
    if (!resultText) {
      return;
    }

    dispatch.resultText = resultText;

    const event: DispatchResultEvent = {
      type: 'dispatch.result',
      timestamp: new Date().toISOString(),
      workspaceId: this.spec.id,
      dispatch: { ...dispatch },
      taskId: dispatch.providerTaskId ?? '',
      resultText,
    };
    this.emitEvent(event);
  }

  private handleAssistantMessage(
    message: Extract<SDKMessage, { type: 'assistant' }>,
  ): void {
    const text = extractMessageText(message);
    if (!text) {
      return;
    }
    const visibility = this.pendingAssistantVisibilities.shift() ?? 'public';

    const event: WorkspaceMessageEvent = {
      type: 'message',
      timestamp: new Date().toISOString(),
      workspaceId: this.spec.id,
      role: 'assistant',
      text,
      visibility,
      raw: message,
      ...(this.spec.coordinatorRoleId ?? this.spec.defaultRoleId
        ? { memberId: this.spec.coordinatorRoleId ?? this.spec.defaultRoleId }
        : {}),
      ...(message.session_id ? { sessionId: message.session_id } : {}),
      ...(message.parent_tool_use_id !== undefined
        ? { parentToolUseId: message.parent_tool_use_id }
        : {}),
    };
    this.emitEvent(event);
    if (visibility === 'public') {
      this.publishActivity('coordinator_message', text, {
        visibility: 'public',
        ...(this.spec.coordinatorRoleId ?? this.spec.defaultRoleId
          ? { roleId: this.spec.coordinatorRoleId ?? this.spec.defaultRoleId }
          : {}),
      });
    }
  }

  private handleToolProgress(message: SDKToolProgressMessage): void {
    this.emitEvent({
      type: 'tool.progress',
      timestamp: new Date().toISOString(),
      workspaceId: this.spec.id,
      toolName: message.tool_name,
      elapsedTimeSeconds: message.elapsed_time_seconds,
      ...(message.task_id ? { taskId: message.task_id } : {}),
    });
  }

  private handleSystemMessage(
    message: Extract<SDKMessage, { type: 'system' }>,
  ): void {
    switch (message.subtype) {
      case 'init':
        this.state.sessionId = message.session_id;
        this.emitInitialized({
          availableAgents: normalizeAgentNames(message.agents),
          availableTools: Array.isArray(message.tools) ? message.tools : [],
          sessionId: message.session_id,
        });
        return;
      case 'session_state_changed':
        this.state.status = message.state;
        this.emitEvent({
          type: 'workspace.state.changed',
          timestamp: new Date().toISOString(),
          workspaceId: this.spec.id,
          state: message.state,
        });
        return;
      case 'task_started':
        this.handleTaskStarted(message as SDKTaskStartedMessage);
        return;
      case 'task_progress':
        this.handleTaskProgress(message as SDKTaskProgressMessage);
        return;
      case 'task_notification':
        this.handleTaskNotification(message as SDKTaskNotificationMessage);
        return;
      default: {
        const text = extractMessageText(message);
        if (!text) {
          return;
        }

        const event: WorkspaceMessageEvent = {
          type: 'message',
          timestamp: new Date().toISOString(),
          workspaceId: this.spec.id,
          role: 'system',
          text,
          raw: message,
          ...(message.session_id ? { sessionId: message.session_id } : {}),
        };
        this.emitEvent(event);
      }
    }
  }

  private handleTaskStarted(message: SDKTaskStartedMessage): void {
    const dispatch = this.attachDispatchToTask(message.task_id, message.description);
    if (message.tool_use_id) {
      dispatch.toolUseId = message.tool_use_id;
      const liveDispatch = this.state.dispatches[dispatch.dispatchId];
      if (liveDispatch) {
        liveDispatch.toolUseId = message.tool_use_id;
      }
      this.toolUseToDispatch.set(message.tool_use_id, dispatch.dispatchId);
    }

    const event: DispatchStartedEvent = {
      type: 'dispatch.started',
      timestamp: new Date().toISOString(),
      workspaceId: this.spec.id,
      dispatch,
      taskId: message.task_id,
      description: message.description,
    };
    this.emitEvent(event);
    this.updateMemberState(dispatch.roleId, 'active', message.description, message.task_id);
    this.publishActivity('dispatch_started', message.description, {
      roleId: dispatch.roleId,
      dispatchId: dispatch.dispatchId,
      taskId: message.task_id,
      visibility: dispatch.visibility ?? this.spec.activityPolicy?.defaultVisibility ?? 'public',
    });
  }

  private handleTaskProgress(message: SDKTaskProgressMessage): void {
    const dispatch = this.findDispatchByTaskId(message.task_id);
    if (!dispatch) {
      return;
    }

    dispatch.status = 'running';
    if (message.summary) {
      dispatch.lastSummary = message.summary;
    }

    const event: DispatchProgressEvent = {
      type: 'dispatch.progress',
      timestamp: new Date().toISOString(),
      workspaceId: this.spec.id,
      dispatch: { ...dispatch },
      taskId: message.task_id,
      description: message.description,
      ...(message.summary ? { summary: message.summary } : {}),
      ...(message.last_tool_name ? { lastToolName: message.last_tool_name } : {}),
    };
    this.emitEvent(event);
    this.updateMemberState(
      dispatch.roleId,
      'active',
      message.summary ?? message.description,
      message.task_id,
    );
    this.publishActivity('member_progress', message.summary ?? message.description, {
      roleId: dispatch.roleId,
      dispatchId: dispatch.dispatchId,
      taskId: message.task_id,
      visibility: dispatch.visibility ?? this.spec.activityPolicy?.defaultVisibility ?? 'public',
    });
  }

  private handleTaskNotification(message: SDKTaskNotificationMessage): void {
    const dispatch = this.findDispatchByTaskId(message.task_id);
    if (!dispatch) {
      return;
    }

    dispatch.status =
      message.status === 'completed'
        ? 'completed'
        : message.status === 'failed'
          ? 'failed'
          : 'stopped';
    dispatch.completedAt = new Date().toISOString();
    if (message.output_file) {
      dispatch.outputFile = message.output_file;
    } else {
      delete dispatch.outputFile;
    }
    dispatch.lastSummary = message.summary;
    this.pendingResultDispatchQueue.push(dispatch.dispatchId);

    const event: DispatchCompletedEvent = {
      type:
        dispatch.status === 'completed'
          ? 'dispatch.completed'
          : dispatch.status === 'failed'
            ? 'dispatch.failed'
            : 'dispatch.stopped',
      timestamp: new Date().toISOString(),
      workspaceId: this.spec.id,
      dispatch: { ...dispatch },
      taskId: message.task_id,
      outputFile: message.output_file,
      summary: message.summary,
    };
    this.emitEvent(event);
    this.updateMemberState(
      dispatch.roleId,
      dispatch.status === 'completed'
        ? 'idle'
        : dispatch.status === 'failed'
          ? 'blocked'
          : 'waiting',
      message.summary,
      message.task_id,
    );
    this.publishActivity(
      dispatch.status === 'completed' ? 'member_delivered' : 'member_summary',
      message.summary,
      {
        roleId: dispatch.roleId,
        dispatchId: dispatch.dispatchId,
        taskId: message.task_id,
        visibility: dispatch.visibility ?? this.spec.activityPolicy?.defaultVisibility ?? 'public',
      },
    );
  }

  private attachDispatchToTask(taskId: string, description: string): TaskDispatch {
    const nextDispatchId = this.pendingDispatchQueue.shift();

    if (!nextDispatchId) {
      const synthetic: TaskDispatch = {
        dispatchId: `untracked-${taskId}`,
        workspaceId: this.spec.id,
        roleId: '_unknown',
        instruction: description,
        status: 'started',
        providerTaskId: taskId,
        createdAt: new Date().toISOString(),
        startedAt: new Date().toISOString(),
      };
      this.state.dispatches[synthetic.dispatchId] = synthetic;
      this.taskToDispatch.set(taskId, synthetic.dispatchId);
      return { ...synthetic };
    }

    const dispatch = this.state.dispatches[nextDispatchId];
    if (!dispatch) {
      throw new Error(`Dispatch disappeared before task start: ${nextDispatchId}`);
    }

    dispatch.status = 'started';
    dispatch.providerTaskId = taskId;
    dispatch.startedAt = new Date().toISOString();
    dispatch.lastSummary = description;
    this.taskToDispatch.set(taskId, nextDispatchId);

    return { ...dispatch };
  }

  private findDispatchByTaskId(taskId: string): TaskDispatch | undefined {
    const dispatchId = this.taskToDispatch.get(taskId);
    if (!dispatchId) {
      return undefined;
    }

    return this.state.dispatches[dispatchId];
  }

  private emitInitialized(details: {
    availableAgents: string[];
    availableTools: string[];
    sessionId?: string;
  }): void {
    const hasSession = Boolean(details.sessionId);
    if (this.initialized && (this.initializedHadSession || !hasSession)) {
      return;
    }

    this.initialized = true;
    this.initializedHadSession = hasSession;

    const initializedEvent: WorkspaceInitializedEvent = {
      type: 'workspace.initialized',
      timestamp: new Date().toISOString(),
      workspaceId: this.spec.id,
      availableAgents: details.availableAgents,
      availableTools: details.availableTools,
      ...(this.availableCommands.length ? { availableCommands: this.availableCommands } : {}),
      ...(details.sessionId ? { sessionId: details.sessionId } : {}),
    };
    this.emitEvent(initializedEvent);
  }

  private async requestCoordinatorPlan(
    request: WorkspaceTurnRequest,
    timeoutMs = 120_000,
  ): Promise<string> {
    const responsePromise = this.waitForEvent(
      (event): event is WorkspaceMessageEvent =>
        event.type === 'message' &&
        event.role === 'assistant' &&
        event.visibility === 'coordinator',
      { timeoutMs },
    );

    this.pushUserMessage(
      buildWorkspaceTurnPrompt(this.spec, request),
      'coordinator',
      false,
    );

    const response = await responsePromise;
    return response.text;
  }

  private emitCoordinatorSummary(text: string, roleId: string): void {
    const event: WorkspaceMessageEvent = {
      type: 'message',
      timestamp: new Date().toISOString(),
      workspaceId: this.spec.id,
      role: 'assistant',
      text,
      visibility: 'public',
      memberId: roleId,
      raw: {
        type: 'workspace_turn_summary',
        roleId,
      },
      ...(this.state.sessionId ? { sessionId: this.state.sessionId } : {}),
    };
    this.emitEvent(event);
    this.publishActivity('coordinator_message', text, {
      roleId,
      visibility: 'public',
    });
  }

  private claimDispatch(
    dispatchId: string,
    roleId: string,
    note?: string,
    claimStatus: ClaimStatus = 'claimed',
  ): void {
    const dispatch = this.state.dispatches[dispatchId];
    const member = this.state.members[roleId];
    if (!dispatch || !member) {
      return;
    }

    dispatch.claimStatus = claimStatus;
    dispatch.claimedByMemberIds = Array.from(
      new Set([...(dispatch.claimedByMemberIds ?? []), roleId]),
    );

    const event: DispatchClaimedEvent = {
      type: 'dispatch.claimed',
      timestamp: new Date().toISOString(),
      workspaceId: this.spec.id,
      dispatch: { ...dispatch },
      member: { ...member },
      claimStatus,
      ...(note ? { note } : {}),
    };
    this.emitEvent(event);
    this.publishActivity('member_claimed', note ?? `${member.roleName} claimed the task.`, {
      roleId,
      dispatchId,
      visibility: dispatch.visibility ?? this.spec.activityPolicy?.defaultVisibility ?? 'public',
    });
  }

  private updateMemberState(
    roleId: string,
    status: WorkspaceMember['status'],
    summary?: string,
    sessionId?: string,
  ): void {
    const member = this.state.members[roleId];
    if (!member) {
      return;
    }
    const nextMember: WorkspaceMember = {
      ...member,
      status,
      ...(summary ? { publicStateSummary: summary } : {}),
      ...(sessionId ? { sessionId } : {}),
      lastActivityAt: new Date().toISOString(),
    };
    this.state.members[roleId] = nextMember;
    const event: MemberStateChangedEvent = {
      type: 'member.state.changed',
      timestamp: new Date().toISOString(),
      workspaceId: this.spec.id,
      member: { ...nextMember },
    };
    this.emitEvent(event);
  }

  private publishActivity(
    kind: WorkspaceActivityKind,
    text: string,
    details: {
      roleId?: string;
      dispatchId?: string;
      taskId?: string;
      visibility?: WorkspaceVisibility;
    } = {},
  ): void {
    const activity: WorkspaceActivity = {
      activityId: randomUUID(),
      workspaceId: this.spec.id,
      kind,
      visibility: details.visibility ?? this.spec.activityPolicy?.defaultVisibility ?? 'public',
      text,
      createdAt: new Date().toISOString(),
      ...(details.roleId ? { roleId: details.roleId, memberId: details.roleId } : {}),
      ...(details.dispatchId ? { dispatchId: details.dispatchId } : {}),
      ...(details.taskId ? { taskId: details.taskId } : {}),
    };
    this.state.activities = [...this.state.activities, activity];
    const event: ActivityPublishedEvent = {
      type: 'activity.published',
      timestamp: new Date().toISOString(),
      workspaceId: this.spec.id,
      activity,
    };
    this.emitEvent(event);
  }
}
