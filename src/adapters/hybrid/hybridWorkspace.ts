import { randomUUID } from 'node:crypto';

import type { WorkspaceEvent, WorkspaceInitializedEvent, WorkspaceMessageEvent } from '../../core/events.js';
import { WorkspaceRuntime } from '../../core/runtime.js';
import type {
  ClaimStatus,
  CoordinatorWorkflowDecision,
  MultiAgentProvider,
  RoleSpec,
  RoleTaskRequest,
  TaskDispatch,
  WorkspaceActivity,
  WorkspaceActivityKind,
  WorkspaceClaimResponse,
  WorkspaceClaimWindow,
  WorkspaceMember,
  WorkspaceSpec,
  WorkspaceState,
  WorkspaceTurnRequest,
  WorkspaceTurnResult,
  WorkspaceVisibility,
  WorkspaceWorkflowVoteResponse,
  WorkspaceWorkflowVoteWindow,
} from '../../core/types.js';
import {
  buildCoordinatorDecisionPrompt,
  buildPlanFromClaimResponses,
  buildWorkflowEntryPlan,
  buildWorkflowVotePrompt,
  buildWorkspaceClaimPrompt,
  parseCoordinatorDecision,
  parseWorkflowVoteResponse,
  parseWorkspaceClaimResponse,
  planWorkspaceTurnHeuristically,
  resolveClaimCandidateRoleIds,
  resolveWorkflowVoteCandidateRoleIds,
  shouldApproveWorkflowVote,
} from '../../core/workspaceTurn.js';
import { ClaudeAgentWorkspace, type ClaudeAgentWorkspaceOptions } from '../claude/claudeAgentWorkspace.js';
import { CodexSdkWorkspace, type CodexSdkWorkspaceOptions } from '../codex/codexSdkWorkspace.js';

type ChildWorkspace = ClaudeAgentWorkspace | CodexSdkWorkspace;

export interface HybridWorkspaceOptions {
  spec: WorkspaceSpec;
  defaultModels?: Partial<Record<MultiAgentProvider, string>>;
  claude?: Omit<ClaudeAgentWorkspaceOptions, 'spec' | 'sessionId'>;
  codex?: Omit<CodexSdkWorkspaceOptions, 'spec'>;
}

export class HybridWorkspace extends WorkspaceRuntime {
  private readonly spec: WorkspaceSpec;
  private readonly state: WorkspaceState;
  private readonly defaultModels: Partial<Record<MultiAgentProvider, string>>;
  private readonly childWorkspaces = new Map<MultiAgentProvider, ChildWorkspace>();
  private readonly childWorkspaceIds = new Map<MultiAgentProvider, string>();
  private readonly childUnsubscribers: Array<() => void> = [];
  private readonly childSessionIds = new Map<MultiAgentProvider, string>();
  private active = false;
  private initialized = false;

  constructor(options: HybridWorkspaceOptions) {
    super();
    this.spec = options.spec;
    this.defaultModels = options.defaultModels ?? {};
    this.assertHybridSpec();

    this.state = {
      workspaceId: this.spec.id,
      status: 'idle',
      provider: 'hybrid',
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
            provider: this.resolveRoleProvider(role),
            ...(role.direct !== undefined ? { direct: role.direct } : {}),
            status: 'idle',
          } satisfies WorkspaceMember,
        ]),
      ),
      activities: [],
      workflowRuntime: {
        mode: 'group_chat',
      },
    };

    const rolesByProvider = this.groupRolesByProvider();
    for (const [provider, roles] of rolesByProvider.entries()) {
      const childSpec = this.buildChildSpec(provider, roles);
      this.childWorkspaceIds.set(provider, childSpec.id);
      const workspace =
        provider === 'claude-agent-sdk'
          ? new ClaudeAgentWorkspace({
              ...(options.claude ?? {}),
              spec: childSpec,
            })
          : new CodexSdkWorkspace({
              ...(options.codex ?? {}),
              spec: childSpec,
            });
      this.childWorkspaces.set(provider, workspace);
      this.childUnsubscribers.push(
        workspace.onEvent(event => {
          this.handleChildEvent(provider, event);
        }),
      );
    }
  }

  getSnapshot(): WorkspaceState {
    return {
      ...this.state,
      roles: { ...this.state.roles },
      dispatches: { ...this.state.dispatches },
      members: { ...this.state.members },
      activities: [...this.state.activities],
      workflowRuntime: { ...this.state.workflowRuntime },
    };
  }

  async start(): Promise<void> {
    if (this.active) {
      return;
    }

    this.active = true;
    this.state.startedAt = new Date().toISOString();
    this.emitEvent({
      type: 'workspace.started',
      timestamp: new Date().toISOString(),
      workspaceId: this.spec.id,
      spec: this.spec,
    });

    await Promise.all(
      [...this.childWorkspaces.values()].map(workspace => workspace.start()),
    );

    this.initialized = true;
    this.state.status = 'running';
    this.emitEvent(this.buildInitializedEvent());
    this.emitStateChanged('running');
  }

  async send(message: string, visibility: WorkspaceVisibility = 'public'): Promise<void> {
    this.ensureStarted();

    const event: WorkspaceMessageEvent = {
      type: 'message',
      timestamp: new Date().toISOString(),
      workspaceId: this.spec.id,
      role: 'user',
      text: message,
      visibility,
      raw: {
        type: 'workspace_user_message',
      },
    };
    this.emitLocalEvent(event);
    this.publishActivity('user_message', message, {
      visibility,
    });
  }

  async assignRoleTask(request: RoleTaskRequest): Promise<TaskDispatch> {
    this.ensureStarted();
    const role = this.state.roles[request.roleId];
    if (!role) {
      throw new Error(`Unknown role: ${request.roleId}`);
    }

    const provider = this.resolveRoleProvider(role);
    const child = this.getChildWorkspace(provider);
    const dispatch = await child.assignRoleTask(request);
    return this.toTopLevelDispatch(dispatch, provider);
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
    this.ensureStarted();
    await this.send(request.message, request.visibility ?? 'public');

    const coordinatorRole = this.resolveCoordinatorRole();
    const coordinatorDispatch = await this.runRoleTask(
      {
        roleId: coordinatorRole.id,
        summary: 'Route workspace turn',
        visibility: 'coordinator',
        instruction: buildCoordinatorDecisionPrompt(this.spec, request),
      },
      options,
    );
    const coordinatorDecision = parseCoordinatorDecision(
      coordinatorDispatch.resultText ?? coordinatorDispatch.lastSummary ?? '',
      this.spec,
      request,
    );
    this.emitCoordinatorSummary(coordinatorDecision.responseText, coordinatorRole.id);

    if (coordinatorDecision.kind === 'respond') {
      return {
        request,
        coordinatorDispatch,
        plan: {
          coordinatorRoleId: coordinatorRole.id,
          responseText: coordinatorDecision.responseText,
          assignments: [],
          ...(coordinatorDecision.rationale ? { rationale: coordinatorDecision.rationale } : {}),
        },
        dispatches: [],
      };
    }

    let workflowVoteWindow: WorkspaceWorkflowVoteWindow | undefined;
    let workflowVoteResponses: WorkspaceWorkflowVoteResponse[] | undefined;
    let shouldRunWorkflow = false;
    if (coordinatorDecision.kind === 'propose_workflow') {
      workflowVoteWindow = this.openWorkflowVoteWindow(
        request,
        coordinatorDecision,
        resolveWorkflowVoteCandidateRoleIds(this.spec, request, coordinatorDecision),
      );
      workflowVoteResponses = await this.collectWorkflowVoteResponses(
        workflowVoteWindow,
        request,
        coordinatorDecision,
        options.timeoutMs,
      );
      shouldRunWorkflow = shouldApproveWorkflowVote(this.spec, workflowVoteResponses);
      this.closeWorkflowVoteWindow(
        workflowVoteWindow,
        coordinatorDecision,
        workflowVoteResponses,
        shouldRunWorkflow,
      );
      if (!shouldRunWorkflow) {
        return {
          request,
          coordinatorDispatch,
          workflowVoteWindow,
          workflowVoteResponses,
          plan: {
            coordinatorRoleId: coordinatorRole.id,
            responseText: coordinatorDecision.responseText,
            assignments: [],
            rationale: 'Workflow vote rejected; staying in group chat mode.',
          },
          dispatches: [],
        };
      }
      this.emitWorkflowStarted(coordinatorDecision, workflowVoteWindow);
    }

    const effectiveRequest =
      coordinatorDecision.kind === 'delegate' && coordinatorDecision.targetRoleId
        ? { ...request, preferRoleId: coordinatorDecision.targetRoleId }
        : request;

    const claimCandidateRoleIds =
      !shouldRunWorkflow && this.spec.claimPolicy?.mode === 'claim'
        ? resolveClaimCandidateRoleIds(this.spec, effectiveRequest)
        : undefined;
    const claimWindow =
      !shouldRunWorkflow && this.spec.claimPolicy?.mode === 'claim'
        ? this.openClaimWindow(
            effectiveRequest,
            claimCandidateRoleIds ?? this.spec.roles.map(role => role.id),
          )
        : undefined;

    const claimResponses = claimWindow
      ? await this.collectClaimResponses(claimWindow, effectiveRequest, options.timeoutMs)
      : undefined;

    const plan = claimResponses
      ? buildPlanFromClaimResponses(this.spec, effectiveRequest, claimResponses)
      : shouldRunWorkflow
        ? buildWorkflowEntryPlan(this.spec, effectiveRequest)
        : {
            coordinatorRoleId: coordinatorRole.id,
            responseText: coordinatorDecision.responseText,
            assignments: planWorkspaceTurnHeuristically(this.spec, effectiveRequest).assignments,
            ...(coordinatorDecision.rationale ? { rationale: coordinatorDecision.rationale } : {}),
          };

    if (claimWindow) {
      this.closeClaimWindow(
        claimWindow,
        claimResponses ?? [],
        plan.assignments.map(assignment => assignment.roleId),
      );
    }

    const dispatches: TaskDispatch[] = [];
    for (const assignment of plan.assignments) {
      const dispatch = await this.assignRoleTask({
        roleId: assignment.roleId,
        instruction: assignment.instruction,
        ...(assignment.summary ? { summary: assignment.summary } : {}),
        visibility: assignment.visibility ?? request.visibility ?? 'public',
        sourceRoleId: coordinatorRole.id,
      });
      const claimResponse = claimResponses?.find(response => response.roleId === assignment.roleId);
      this.claimDispatch(
        dispatch.dispatchId,
        assignment.roleId,
        claimResponse?.publicResponse ?? claimResponse?.rationale ?? 'Claimed by runtime routing',
        claimResponse?.decision === 'support' ? 'supporting' : 'claimed',
      );
      dispatches.push(
        await this.runDispatch(Promise.resolve(dispatch), options),
      );
    }

    return {
      request,
      coordinatorDispatch,
      ...(claimWindow ? { claimWindow } : {}),
      ...(claimResponses ? { claimResponses } : {}),
      ...(workflowVoteWindow ? { workflowVoteWindow } : {}),
      ...(workflowVoteResponses ? { workflowVoteResponses } : {}),
      plan,
      dispatches,
    };
  }

  async deleteWorkspace(): Promise<void> {
    await Promise.all(
      [...this.childWorkspaces.values()].map(workspace => workspace.deleteWorkspace()),
    );
  }

  async close(): Promise<void> {
    if (!this.active) {
      return;
    }

    await Promise.all(
      [...this.childWorkspaces.values()].map(workspace => workspace.close()),
    );
    this.active = false;
    this.initialized = false;
    this.state.status = 'closed';
    this.emitStateChanged('closed');
  }

  private assertHybridSpec(): void {
    if (this.spec.provider !== 'hybrid') {
      throw new Error('HybridWorkspace requires spec.provider = "hybrid".');
    }

    if (this.spec.roles.length === 0) {
      throw new Error('HybridWorkspace requires at least one role.');
    }

    for (const role of this.spec.roles) {
      if (!role.agent.provider) {
        throw new Error(
          `Role "${role.id}" is missing agent.provider. Hybrid workspaces require provider per role.`,
        );
      }
    }
  }

  private groupRolesByProvider(): Map<MultiAgentProvider, RoleSpec[]> {
    const map = new Map<MultiAgentProvider, RoleSpec[]>();
    for (const role of this.spec.roles) {
      const provider = this.resolveRoleProvider(role);
      const bucket = map.get(provider) ?? [];
      bucket.push(role);
      map.set(provider, bucket);
    }
    return map;
  }

  private buildChildSpec(provider: MultiAgentProvider, roles: RoleSpec[]): WorkspaceSpec {
    const defaultModel = this.resolveDefaultModel(provider, roles);
    const roleIds = new Set(roles.map(role => role.id));
    const defaultRoleId =
      this.spec.defaultRoleId && roleIds.has(this.spec.defaultRoleId)
        ? this.spec.defaultRoleId
        : roles[0]?.id;
    const coordinatorRoleId =
      this.spec.coordinatorRoleId && roleIds.has(this.spec.coordinatorRoleId)
        ? this.spec.coordinatorRoleId
        : defaultRoleId;

    return {
      ...this.spec,
      id: `${this.spec.id}--${provider === 'claude-agent-sdk' ? 'claude' : 'codex'}`,
      provider,
      model: defaultModel,
      roles,
      ...(defaultRoleId ? { defaultRoleId } : {}),
      ...(coordinatorRoleId ? { coordinatorRoleId } : {}),
      ...(this.spec.claimPolicy
        ? {
            claimPolicy: {
              ...this.spec.claimPolicy,
              ...(this.spec.claimPolicy.fallbackRoleId &&
              roleIds.has(this.spec.claimPolicy.fallbackRoleId)
                ? { fallbackRoleId: this.spec.claimPolicy.fallbackRoleId }
                : defaultRoleId
                  ? { fallbackRoleId: defaultRoleId }
                  : {}),
            },
          }
        : {}),
    };
  }

  private resolveDefaultModel(provider: MultiAgentProvider, roles: RoleSpec[]): string {
    const explicitRoleModel = roles.find(role => role.agent.model)?.agent.model;
    const defaultModel = this.defaultModels[provider] ?? explicitRoleModel;
    if (!defaultModel) {
      throw new Error(
        `No model configured for ${provider}. Set role.agent.model or HybridWorkspace defaultModels.`,
      );
    }
    return defaultModel;
  }

  private resolveRoleProvider(role: RoleSpec): MultiAgentProvider {
    if (role.agent.provider) {
      return role.agent.provider;
    }

    throw new Error(`Role "${role.id}" does not declare a provider.`);
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

  private getChildWorkspace(provider: MultiAgentProvider): ChildWorkspace {
    const workspace = this.childWorkspaces.get(provider);
    if (!workspace) {
      throw new Error(`No child workspace registered for provider ${provider}.`);
    }
    return workspace;
  }

  private handleChildEvent(provider: MultiAgentProvider, event: WorkspaceEvent): void {
    if (event.type === 'workspace.started') {
      return;
    }

    if (event.type === 'workspace.initialized') {
      if (event.sessionId) {
        this.childSessionIds.set(provider, event.sessionId);
      }
      return;
    }

    if (event.type === 'workspace.state.changed') {
      return;
    }

    const rewritten = this.rewriteEvent(provider, event);
    this.applyEventToState(rewritten);
    this.emitEvent(rewritten);
  }

  private rewriteEvent(provider: MultiAgentProvider, event: WorkspaceEvent): WorkspaceEvent {
    switch (event.type) {
      case 'member.registered':
      case 'member.state.changed':
        return {
          ...event,
          workspaceId: this.spec.id,
          member: {
            ...event.member,
            workspaceId: this.spec.id,
            provider,
          },
        };
      case 'dispatch.queued':
      case 'dispatch.started':
      case 'dispatch.progress':
      case 'dispatch.completed':
      case 'dispatch.failed':
      case 'dispatch.stopped':
      case 'dispatch.result':
      case 'dispatch.claimed':
        return {
          ...event,
          workspaceId: this.spec.id,
          dispatch: {
            ...event.dispatch,
            workspaceId: this.spec.id,
            provider,
          },
          ...('member' in event && event.member
            ? {
                member: {
                  ...event.member,
                  workspaceId: this.spec.id,
                  provider,
                },
              }
            : {}),
        } as WorkspaceEvent;
      case 'message':
        return {
          ...event,
          workspaceId: this.spec.id,
        };
      case 'activity.published':
        return {
          ...event,
          workspaceId: this.spec.id,
          activity: {
            ...event.activity,
            workspaceId: this.spec.id,
          },
        };
      case 'claim.window.opened':
      case 'claim.window.closed':
      case 'claim.response':
      case 'workflow.vote.opened':
      case 'workflow.vote.closed':
      case 'workflow.vote.response':
      case 'workflow.started':
      case 'workflow.stage.started':
      case 'workflow.stage.completed':
      case 'tool.progress':
      case 'result':
      case 'error':
        return {
          ...event,
          workspaceId: this.spec.id,
        };
      default:
        return event;
    }
  }

  private applyEventToState(event: WorkspaceEvent): void {
    switch (event.type) {
      case 'member.registered':
      case 'member.state.changed':
        this.state.members[event.member.roleId] = { ...event.member };
        return;
      case 'dispatch.queued':
      case 'dispatch.started':
      case 'dispatch.progress':
      case 'dispatch.completed':
      case 'dispatch.failed':
      case 'dispatch.stopped':
      case 'dispatch.result':
      case 'dispatch.claimed':
        this.state.dispatches[event.dispatch.dispatchId] = { ...event.dispatch };
        return;
      case 'activity.published':
        this.state.activities = [...this.state.activities, event.activity];
        return;
      default:
        return;
    }
  }

  private buildInitializedEvent(): WorkspaceInitializedEvent {
    return {
      type: 'workspace.initialized',
      timestamp: new Date().toISOString(),
      workspaceId: this.spec.id,
      availableAgents: this.spec.roles.map(role => role.id),
      availableTools: this.spec.allowedTools ?? [],
      availableCommands: ['runWorkspaceTurn', 'runRoleTask', 'assignRoleTask'],
    };
  }

  private emitStateChanged(state: WorkspaceState['status']): void {
    this.emitEvent({
      type: 'workspace.state.changed',
      timestamp: new Date().toISOString(),
      workspaceId: this.spec.id,
      state,
    });
  }

  private emitLocalEvent(event: WorkspaceEvent): void {
    this.applyEventToState(event);
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
    this.emitLocalEvent({
      type: 'activity.published',
      timestamp: new Date().toISOString(),
      workspaceId: this.spec.id,
      activity,
    });
  }

  private emitCoordinatorSummary(text: string, roleId: string): void {
    this.publishActivity('coordinator_message', text, {
      roleId,
      visibility: 'public',
    });
  }

  private openClaimWindow(
    request: WorkspaceTurnRequest,
    candidateRoleIds: string[],
  ): WorkspaceClaimWindow {
    const claimWindow: WorkspaceClaimWindow = {
      windowId: randomUUID(),
      request,
      candidateRoleIds,
      ...(this.spec.claimPolicy?.claimTimeoutMs
        ? { timeoutMs: this.spec.claimPolicy.claimTimeoutMs }
        : {}),
    };
    this.emitLocalEvent({
      type: 'claim.window.opened',
      timestamp: new Date().toISOString(),
      workspaceId: this.spec.id,
      claimWindow,
    });
    this.publishActivity('claim_window_opened', `Claim window opened for: ${request.message}`, {
      visibility: 'public',
    });
    return claimWindow;
  }

  private closeClaimWindow(
    claimWindow: WorkspaceClaimWindow,
    responses: WorkspaceClaimResponse[],
    selectedRoleIds: string[],
  ): void {
    this.emitLocalEvent({
      type: 'claim.window.closed',
      timestamp: new Date().toISOString(),
      workspaceId: this.spec.id,
      claimWindow,
      responses,
      selectedRoleIds,
    });
    this.publishActivity(
      'claim_window_closed',
      selectedRoleIds.length > 0
        ? `Claim window resolved: ${selectedRoleIds.map(roleId => `@${roleId}`).join(', ')}`
        : 'Claim window closed with no claimants.',
      { visibility: 'public' },
    );
  }

  private emitClaimResponse(
    claimWindow: WorkspaceClaimWindow,
    response: WorkspaceClaimResponse,
  ): void {
    this.emitLocalEvent({
      type: 'claim.response',
      timestamp: new Date().toISOString(),
      workspaceId: this.spec.id,
      claimWindowId: claimWindow.windowId,
      response,
    });
    this.publishActivity(
      response.decision === 'claim'
        ? 'member_claimed'
        : response.decision === 'support'
          ? 'member_supporting'
          : 'member_declined',
      response.publicResponse ?? response.rationale,
      {
        roleId: response.roleId,
        visibility: 'public',
      },
    );
  }

  private async collectClaimResponses(
    claimWindow: WorkspaceClaimWindow,
    request: WorkspaceTurnRequest,
    timeoutMs = 120_000,
  ): Promise<WorkspaceClaimResponse[]> {
    const claimProbeTimeout = Math.max(
      5_000,
      Math.min(timeoutMs, this.spec.claimPolicy?.claimTimeoutMs ?? 30_000),
    );

    return Promise.all(
      claimWindow.candidateRoleIds.map(async roleId => {
        const role = this.spec.roles.find(value => value.id === roleId);
        if (!role) {
          const response: WorkspaceClaimResponse = {
            roleId,
            decision: 'decline',
            confidence: 0,
            rationale: `@${roleId} is not available for this claim window.`,
            publicResponse: `@${roleId} passed on this request.`,
          };
          this.emitClaimResponse(claimWindow, response);
          return response;
        }
        try {
          const dispatch = await this.runRoleTask(
            {
              roleId: role.id,
              summary: 'Claim probe',
              visibility: 'coordinator',
              instruction: buildWorkspaceClaimPrompt(this.spec, role, request),
            },
            { timeoutMs: claimProbeTimeout, resultTimeoutMs: 10_000 },
          );
          const response = parseWorkspaceClaimResponse(
            dispatch.resultText ?? dispatch.lastSummary ?? '',
            role,
            request,
          );
          this.emitClaimResponse(claimWindow, response);
          return response;
        } catch {
          const response: WorkspaceClaimResponse = {
            roleId: role.id,
            decision: 'decline',
            confidence: 0,
            rationale: `@${role.id} did not return a valid claim response in time.`,
            publicResponse: `@${role.id} passed on this request.`,
          };
          this.emitClaimResponse(claimWindow, response);
          return response;
        }
      }),
    );
  }

  private openWorkflowVoteWindow(
    request: WorkspaceTurnRequest,
    coordinatorDecision: CoordinatorWorkflowDecision,
    candidateRoleIds: string[],
  ): WorkspaceWorkflowVoteWindow {
    this.state.workflowRuntime = {
      ...this.state.workflowRuntime,
      mode: 'workflow_vote',
    };
    const voteWindow: WorkspaceWorkflowVoteWindow = {
      voteId: randomUUID(),
      request,
      reason: coordinatorDecision.workflowVoteReason ?? coordinatorDecision.responseText,
      candidateRoleIds,
      ...(this.spec.workflowVotePolicy?.timeoutMs
        ? { timeoutMs: this.spec.workflowVotePolicy.timeoutMs }
        : {}),
    };
    this.state.workflowRuntime.activeVoteWindow = voteWindow;
    this.emitLocalEvent({
      type: 'workflow.vote.opened',
      timestamp: new Date().toISOString(),
      workspaceId: this.spec.id,
      coordinatorDecision,
      voteWindow,
    });
    this.publishActivity('workflow_vote_opened', voteWindow.reason, {
      roleId: this.resolveCoordinatorRole().id,
      visibility: 'public',
    });
    return voteWindow;
  }

  private emitWorkflowVoteResponse(
    voteWindow: WorkspaceWorkflowVoteWindow,
    response: WorkspaceWorkflowVoteResponse,
  ): void {
    this.emitLocalEvent({
      type: 'workflow.vote.response',
      timestamp: new Date().toISOString(),
      workspaceId: this.spec.id,
      voteId: voteWindow.voteId,
      response,
    });
    this.publishActivity(
      response.decision === 'approve'
        ? 'workflow_vote_approved'
        : response.decision === 'reject'
          ? 'workflow_vote_rejected'
          : 'member_summary',
      response.publicResponse ?? response.rationale,
      {
        roleId: response.roleId,
        visibility: 'public',
      },
    );
  }

  private async collectWorkflowVoteResponses(
    voteWindow: WorkspaceWorkflowVoteWindow,
    request: WorkspaceTurnRequest,
    coordinatorDecision: CoordinatorWorkflowDecision,
    timeoutMs = 120_000,
  ): Promise<WorkspaceWorkflowVoteResponse[]> {
    const voteTimeout = Math.max(
      5_000,
      Math.min(timeoutMs, this.spec.workflowVotePolicy?.timeoutMs ?? 30_000),
    );

    return Promise.all(
      voteWindow.candidateRoleIds.map(async roleId => {
        const role = this.spec.roles.find(value => value.id === roleId);
        if (!role) {
          const response: WorkspaceWorkflowVoteResponse = {
            roleId,
            decision: 'abstain',
            confidence: 0,
            rationale: `@${roleId} is not available for workflow voting.`,
            publicResponse: `@${roleId} abstained.`,
          };
          this.emitWorkflowVoteResponse(voteWindow, response);
          return response;
        }
        try {
          const dispatch = await this.runRoleTask(
            {
              roleId: role.id,
              summary: 'Workflow vote',
              visibility: 'coordinator',
              instruction: buildWorkflowVotePrompt(
                this.spec,
                role,
                request,
                coordinatorDecision,
              ),
            },
            { timeoutMs: voteTimeout, resultTimeoutMs: 10_000 },
          );
          const response = parseWorkflowVoteResponse(
            dispatch.resultText ?? dispatch.lastSummary ?? '',
            role,
            this.spec,
            request,
            coordinatorDecision,
          );
          this.emitWorkflowVoteResponse(voteWindow, response);
          return response;
        } catch {
          const response: WorkspaceWorkflowVoteResponse = {
            roleId: role.id,
            decision: 'abstain',
            confidence: 0,
            rationale: `@${role.id} did not return a workflow vote in time.`,
            publicResponse: `@${role.id} abstained.`,
          };
          this.emitWorkflowVoteResponse(voteWindow, response);
          return response;
        }
      }),
    );
  }

  private closeWorkflowVoteWindow(
    voteWindow: WorkspaceWorkflowVoteWindow,
    coordinatorDecision: CoordinatorWorkflowDecision,
    responses: WorkspaceWorkflowVoteResponse[],
    approved: boolean,
  ): void {
    this.state.workflowRuntime = {
      mode: approved ? 'workflow_running' : 'group_chat',
      ...(this.state.workflowRuntime.activeNodeId
        ? { activeNodeId: this.state.workflowRuntime.activeNodeId }
        : {}),
      ...(this.state.workflowRuntime.activeStageId
        ? { activeStageId: this.state.workflowRuntime.activeStageId }
        : {}),
    };
    this.emitLocalEvent({
      type: 'workflow.vote.closed',
      timestamp: new Date().toISOString(),
      workspaceId: this.spec.id,
      coordinatorDecision,
      voteWindow,
      responses,
      approved,
    });
    this.publishActivity(
      approved ? 'workflow_vote_approved' : 'workflow_vote_rejected',
      approved ? 'Workflow vote approved.' : 'Workflow vote rejected.',
      {
        roleId: this.resolveCoordinatorRole().id,
        visibility: 'public',
      },
    );
  }

  private emitWorkflowStarted(
    coordinatorDecision: CoordinatorWorkflowDecision,
    voteWindow?: WorkspaceWorkflowVoteWindow,
  ): void {
    this.emitLocalEvent({
      type: 'workflow.started',
      timestamp: new Date().toISOString(),
      workspaceId: this.spec.id,
      coordinatorDecision,
      ...(voteWindow ? { voteWindow } : {}),
    });
    this.publishActivity('workflow_started', coordinatorDecision.responseText, {
      roleId: this.resolveCoordinatorRole().id,
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

    this.emitLocalEvent({
      type: 'dispatch.claimed',
      timestamp: new Date().toISOString(),
      workspaceId: this.spec.id,
      dispatch: { ...dispatch },
      member: { ...member },
      claimStatus,
      ...(note ? { note } : {}),
    });
    this.publishActivity(
      claimStatus === 'supporting'
        ? 'member_supporting'
        : claimStatus === 'declined'
          ? 'member_declined'
          : 'member_claimed',
      note ?? `${member.roleName} claimed the task.`,
      {
        roleId,
        dispatchId,
        visibility: dispatch.visibility ?? this.spec.activityPolicy?.defaultVisibility ?? 'public',
      },
    );
  }

  private toTopLevelDispatch(dispatch: TaskDispatch, provider: MultiAgentProvider): TaskDispatch {
    return {
      ...dispatch,
      workspaceId: this.spec.id,
      provider,
    };
  }

  private ensureStarted(): void {
    if (!this.active || !this.initialized) {
      throw new Error('Workspace has not been started.');
    }
  }
}
