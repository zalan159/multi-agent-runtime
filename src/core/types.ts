export type MultiAgentProvider = 'claude-agent-sdk' | 'codex-sdk';
export type WorkspaceProvider = MultiAgentProvider | 'hybrid';

export type WorkspaceVisibility = 'public' | 'private' | 'coordinator';
export type ClaimMode = 'direct' | 'claim' | 'coordinator_only';
export type ClaimStatus = 'pending' | 'claimed' | 'supporting' | 'released' | 'declined';
export type ClaimDecision = 'claim' | 'support' | 'decline';
export type WorkflowVoteDecision = 'approve' | 'reject' | 'abstain';
export type CoordinatorDecisionKind = 'respond' | 'delegate' | 'propose_workflow';
export type MemberStatus = 'idle' | 'active' | 'blocked' | 'waiting' | 'offline';
export type WorkspaceMode = 'group_chat' | 'workflow_vote' | 'workflow_running';
export type WorkflowMode = 'freeform_team' | 'pipeline' | 'loop' | 'review_loop';
export type WorkflowNodeType =
  | 'announce'
  | 'assign'
  | 'claim'
  | 'shell'
  | 'evaluate'
  | 'review'
  | 'branch'
  | 'loop'
  | 'artifact'
  | 'commit'
  | 'revert'
  | 'merge'
  | 'complete';
export type WorkflowEdgeCondition =
  | 'always'
  | 'success'
  | 'failure'
  | 'timeout'
  | 'pass'
  | 'fail'
  | 'approved'
  | 'rejected'
  | 'improved'
  | 'equal_or_worse'
  | 'crash'
  | 'retry'
  | 'exhausted';
export type CompletionStatus = 'done' | 'stuck' | 'discarded' | 'crash';
export type WorkspaceActivityKind =
  | 'user_message'
  | 'coordinator_message'
  | 'claim_window_opened'
  | 'claim_window_closed'
  | 'member_claimed'
  | 'member_supporting'
  | 'member_declined'
  | 'member_progress'
  | 'member_blocked'
  | 'member_delivered'
  | 'member_summary'
  | 'workflow_vote_opened'
  | 'workflow_vote_approved'
  | 'workflow_vote_rejected'
  | 'workflow_started'
  | 'workflow_stage_started'
  | 'workflow_stage_completed'
  | 'workflow_completed'
  | 'dispatch_started'
  | 'dispatch_progress'
  | 'dispatch_completed'
  | 'system_notice';

export type PermissionMode =
  | 'default'
  | 'acceptEdits'
  | 'plan'
  | 'dontAsk'
  | 'bypassPermissions';

export interface RoleAgentSpec {
  description: string;
  prompt: string;
  provider?: MultiAgentProvider;
  tools?: string[];
  disallowedTools?: string[];
  model?: string;
  skills?: string[];
  mcpServers?: string[];
  initialPrompt?: string;
  maxTurns?: number;
  background?: boolean;
  effort?: 'low' | 'medium' | 'high' | 'max' | number;
  permissionMode?: PermissionMode;
}

export interface RoleSpec {
  id: string;
  name: string;
  description?: string;
  direct?: boolean;
  outputRoot?: string;
  agent: RoleAgentSpec;
}

export interface ClaimPolicy {
  mode: ClaimMode;
  claimTimeoutMs?: number;
  maxAssignees?: number;
  allowSupportingClaims?: boolean;
  fallbackRoleId?: string;
}

export interface ActivityPolicy {
  publishUserMessages?: boolean;
  publishCoordinatorMessages?: boolean;
  publishDispatchLifecycle?: boolean;
  publishMemberMessages?: boolean;
  defaultVisibility?: WorkspaceVisibility;
}

export interface WorkflowVotePolicy {
  timeoutMs?: number;
  minimumApprovals?: number;
  requiredApprovalRatio?: number;
  candidateRoleIds?: string[];
}

export interface WorkflowRetryPolicy {
  maxAttempts?: number;
}

export interface WorkflowArtifactSpec {
  id: string;
  kind: 'doc' | 'code' | 'report' | 'metric' | 'evidence' | 'result' | 'task_order';
  path: string;
  ownerRoleId?: string;
  required?: boolean;
  description?: string;
}

export interface WorkflowStageSpec {
  id: string;
  name: string;
  description?: string;
  entryNodeId?: string;
  exitNodeIds?: string[];
}

export interface WorkflowNodeSpec {
  id: string;
  type: WorkflowNodeType;
  title?: string;
  roleId?: string;
  reviewerRoleId?: string;
  candidateRoleIds?: string[];
  command?: string;
  evaluator?: string;
  prompt?: string;
  timeoutMs?: number;
  retry?: WorkflowRetryPolicy;
  requiresArtifacts?: string[];
  producesArtifacts?: string[];
  visibility?: WorkspaceVisibility;
  stageId?: string;
}

export interface WorkflowEdgeSpec {
  from: string;
  to: string;
  when: WorkflowEdgeCondition;
}

export interface CompletionPolicy {
  successNodeIds?: string[];
  failureNodeIds?: string[];
  maxIterations?: number;
  defaultStatus?: CompletionStatus;
}

export interface WorkflowSpec {
  mode: WorkflowMode;
  entryNodeId: string;
  stages?: WorkflowStageSpec[];
  nodes: WorkflowNodeSpec[];
  edges: WorkflowEdgeSpec[];
}

export interface WorkspaceSpec {
  id: string;
  name: string;
  provider: WorkspaceProvider;
  model: string;
  cwd?: string;
  orchestratorPrompt?: string;
  allowedTools?: string[];
  disallowedTools?: string[];
  permissionMode?: PermissionMode;
  settingSources?: Array<'user' | 'project' | 'local'>;
  roles: RoleSpec[];
  defaultRoleId?: string;
  coordinatorRoleId?: string;
  claimPolicy?: ClaimPolicy;
  activityPolicy?: ActivityPolicy;
  workflowVotePolicy?: WorkflowVotePolicy;
  workflow?: WorkflowSpec;
  artifacts?: WorkflowArtifactSpec[];
  completionPolicy?: CompletionPolicy;
}

export interface RoleTaskRequest {
  roleId: string;
  instruction: string;
  summary?: string;
  visibility?: WorkspaceVisibility;
  sourceRoleId?: string;
}

export interface WorkspaceTurnRequest {
  message: string;
  visibility?: WorkspaceVisibility;
  maxAssignments?: number;
  preferRoleId?: string;
}

export interface WorkspaceClaimResponse {
  roleId: string;
  decision: ClaimDecision;
  confidence: number;
  rationale: string;
  publicResponse?: string;
  proposedInstruction?: string;
}

export interface WorkspaceClaimWindow {
  windowId: string;
  request: WorkspaceTurnRequest;
  candidateRoleIds: string[];
  timeoutMs?: number;
}

export interface WorkspaceWorkflowVoteWindow {
  voteId: string;
  request: WorkspaceTurnRequest;
  reason: string;
  candidateRoleIds: string[];
  timeoutMs?: number;
}

export interface WorkspaceWorkflowVoteResponse {
  roleId: string;
  decision: WorkflowVoteDecision;
  confidence: number;
  rationale: string;
  publicResponse?: string;
}

export interface CoordinatorWorkflowDecision {
  kind: CoordinatorDecisionKind;
  responseText: string;
  targetRoleId?: string;
  workflowVoteReason?: string;
  rationale?: string;
}

export interface WorkspaceTurnAssignment {
  roleId: string;
  instruction: string;
  summary?: string;
  visibility?: WorkspaceVisibility;
  workflowNodeId?: string;
  stageId?: string;
}

export interface WorkspaceTurnPlan {
  coordinatorRoleId: string;
  responseText: string;
  assignments: WorkspaceTurnAssignment[];
  rationale?: string;
}

export interface WorkspaceTurnResult {
  request: WorkspaceTurnRequest;
  coordinatorDispatch?: TaskDispatch;
  claimWindow?: WorkspaceClaimWindow;
  claimResponses?: WorkspaceClaimResponse[];
  workflowVoteWindow?: WorkspaceWorkflowVoteWindow;
  workflowVoteResponses?: WorkspaceWorkflowVoteResponse[];
  plan: WorkspaceTurnPlan;
  dispatches: TaskDispatch[];
}

export interface TaskDispatch {
  dispatchId: string;
  workspaceId: string;
  roleId: string;
  provider?: MultiAgentProvider;
  instruction: string;
  summary?: string;
  visibility?: WorkspaceVisibility;
  sourceRoleId?: string;
  status: 'queued' | 'started' | 'running' | 'completed' | 'failed' | 'stopped';
  providerTaskId?: string;
  toolUseId?: string;
  createdAt: string;
  startedAt?: string;
  completedAt?: string;
  outputFile?: string;
  lastSummary?: string;
  resultText?: string;
  claimedByMemberIds?: string[];
  claimStatus?: ClaimStatus;
}

export interface WorkspaceMember {
  memberId: string;
  workspaceId: string;
  roleId: string;
  roleName: string;
  provider?: MultiAgentProvider;
  direct?: boolean;
  sessionId?: string;
  status: MemberStatus;
  publicStateSummary?: string;
  lastActivityAt?: string;
}

export interface WorkspaceActivity {
  activityId: string;
  workspaceId: string;
  kind: WorkspaceActivityKind;
  visibility: WorkspaceVisibility;
  text: string;
  createdAt: string;
  roleId?: string;
  memberId?: string;
  dispatchId?: string;
  taskId?: string;
}

export interface WorkspaceWorkflowRuntimeState {
  mode: WorkspaceMode;
  activeVoteWindow?: WorkspaceWorkflowVoteWindow;
  activeNodeId?: string;
  activeStageId?: string;
}

export interface WorkspaceState {
  workspaceId: string;
  status: 'idle' | 'running' | 'requires_action' | 'closed';
  provider: WorkspaceProvider;
  sessionId?: string;
  startedAt?: string;
  roles: Record<string, RoleSpec>;
  dispatches: Record<string, TaskDispatch>;
  members: Record<string, WorkspaceMember>;
  activities: WorkspaceActivity[];
  workflowRuntime: WorkspaceWorkflowRuntimeState;
}
