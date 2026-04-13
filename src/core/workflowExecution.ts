import { mkdir, readFile, writeFile } from 'node:fs/promises';
import path from 'node:path';

import { resolveWorkflowNodeModel, resolveWorkflowNodeProvider } from './providerResolution.js';
import type {
  CompletionStatus,
  RoleSpec,
  TaskDispatch,
  WorkflowEdgeCondition,
  WorkflowNodeSpec,
  WorkflowSpec,
  WorkflowTaskListArtifact,
  WorkflowWorkItem,
  WorkflowWorkItemStatus,
  WorkflowWorklistMode,
  WorkflowWorklistRuntimeState,
  WorkspaceSpec,
  WorkspaceTurnAssignment,
  WorkspaceTurnRequest,
} from './types.js';
import { buildWorkflowDispatchAssignment, getWorkflowEntryNode } from './workspaceTurn.js';

export interface WorkflowExecutionResult {
  dispatches: TaskDispatch[];
  visitedNodeIds: string[];
  completionStatus: CompletionStatus;
  finalNodeId?: string;
}

export interface WorkflowExecutionHooks {
  onNodeStarted?: (node: WorkflowNodeSpec) => void | Promise<void>;
  onNodeCompleted?: (
    node: WorkflowNodeSpec,
    dispatch: TaskDispatch | undefined,
    outcome: WorkflowEdgeCondition,
  ) => void | Promise<void>;
  onStageStarted?: (stageId: string, node: WorkflowNodeSpec) => void | Promise<void>;
  onStageCompleted?: (stageId: string, node: WorkflowNodeSpec) => void | Promise<void>;
  onWorklistUpdated?: (
    node: WorkflowNodeSpec,
    worklist: WorkflowWorklistRuntimeState,
  ) => void | Promise<void>;
  onCompleted?: (
    result: WorkflowExecutionResult,
    lastNode: WorkflowNodeSpec | undefined,
  ) => void | Promise<void>;
}

interface WorkflowNodeExecutionContext {
  spec: WorkspaceSpec;
  request: WorkspaceTurnRequest;
  runAssignment: (
    assignment: WorkspaceTurnAssignment,
    node: WorkflowNodeSpec,
  ) => Promise<TaskDispatch>;
  hooks: WorkflowExecutionHooks;
}

interface WorkflowNodeExecutionResult {
  dispatches: TaskDispatch[];
  outcome: WorkflowEdgeCondition;
  completionStatus?: CompletionStatus;
}

type WorkflowNodeExecutor = (
  context: WorkflowNodeExecutionContext,
  node: WorkflowNodeSpec,
) => Promise<WorkflowNodeExecutionResult>;

interface LoadedWorklistDocument {
  document: WorkflowTaskListArtifact;
  sourceKey: 'items' | 'tasks';
}

const WORKFLOW_NODE_EXECUTORS: Partial<Record<string, WorkflowNodeExecutor>> = {
  complete: executeCompleteNode,
  worklist: executeWorklistNode,
};

export async function executeWorkflow(
  spec: WorkspaceSpec,
  request: WorkspaceTurnRequest,
  runAssignment: (assignment: WorkspaceTurnAssignment, node: WorkflowNodeSpec) => Promise<TaskDispatch>,
  hooks: WorkflowExecutionHooks = {},
): Promise<WorkflowExecutionResult> {
  const workflow = spec.workflow;
  if (!workflow) {
    return {
      dispatches: [],
      visitedNodeIds: [],
      completionStatus: spec.completionPolicy?.defaultStatus ?? 'stuck',
    };
  }

  const nodeById = new Map(workflow.nodes.map(node => [node.id, node]));
  let currentNode = getWorkflowEntryNode(spec);
  if (!currentNode) {
    return {
      dispatches: [],
      visitedNodeIds: [],
      completionStatus: spec.completionPolicy?.defaultStatus ?? 'stuck',
    };
  }

  const maxIterations = Math.max(1, spec.completionPolicy?.maxIterations ?? 8);
  const maxSteps = Math.max(workflow.nodes.length, 1) * maxIterations;
  const dispatches: TaskDispatch[] = [];
  const visitedNodeIds: string[] = [];
  let activeStageId: string | undefined;
  let lastNode: WorkflowNodeSpec | undefined;

  for (let iteration = 0; iteration < maxSteps && currentNode; iteration += 1) {
    const node = currentNode;
    lastNode = node;
    visitedNodeIds.push(node.id);

    if (node.stageId && node.stageId !== activeStageId) {
      if (activeStageId && hooks.onStageCompleted) {
        await hooks.onStageCompleted(activeStageId, node);
      }
      activeStageId = node.stageId;
      if (hooks.onStageStarted) {
        await hooks.onStageStarted(activeStageId, node);
      }
    }

    if (hooks.onNodeStarted) {
      await hooks.onNodeStarted(node);
    }

    const execution = await executeWorkflowNode(
      {
        spec,
        request,
        runAssignment,
        hooks,
      },
      node,
    );
    dispatches.push(...execution.dispatches);

    const completionStatus =
      execution.completionStatus ?? resolveCompletionStatus(spec, node.id, execution.outcome);
    const terminalDispatch = execution.dispatches.at(-1);

    if (hooks.onNodeCompleted) {
      await hooks.onNodeCompleted(node, terminalDispatch, execution.outcome);
    }

    if (completionStatus) {
      const result: WorkflowExecutionResult = {
        dispatches,
        visitedNodeIds,
        completionStatus,
        finalNodeId: node.id,
      };
      if (activeStageId && hooks.onStageCompleted) {
        await hooks.onStageCompleted(activeStageId, node);
      }
      if (hooks.onCompleted) {
        await hooks.onCompleted(result, node);
      }
      return result;
    }

    const nextNodeId = resolveNextWorkflowNodeId(workflow, node.id, execution.outcome);
    if (!nextNodeId) {
      const result: WorkflowExecutionResult = {
        dispatches,
        visitedNodeIds,
        completionStatus:
          terminalDispatch?.status === 'failed'
            ? 'crash'
            : terminalDispatch?.status === 'stopped'
              ? 'stuck'
              : spec.completionPolicy?.defaultStatus ?? 'stuck',
        finalNodeId: node.id,
      };
      if (activeStageId && hooks.onStageCompleted) {
        await hooks.onStageCompleted(activeStageId, node);
      }
      if (hooks.onCompleted) {
        await hooks.onCompleted(result, node);
      }
      return result;
    }

    const nextNode = nodeById.get(nextNodeId);
    if (!nextNode) {
      const result: WorkflowExecutionResult = {
        dispatches,
        visitedNodeIds,
        completionStatus: 'crash',
        finalNodeId: node.id,
      };
      if (activeStageId && hooks.onStageCompleted) {
        await hooks.onStageCompleted(activeStageId, node);
      }
      if (hooks.onCompleted) {
        await hooks.onCompleted(result, node);
      }
      return result;
    }

    if (
      activeStageId &&
      node.stageId &&
      nextNode.stageId !== node.stageId &&
      hooks.onStageCompleted
    ) {
      await hooks.onStageCompleted(activeStageId, node);
      activeStageId = undefined;
    }

    currentNode = nextNode;
  }

  const result: WorkflowExecutionResult = {
    dispatches,
    visitedNodeIds,
    completionStatus: spec.completionPolicy?.defaultStatus ?? 'stuck',
    ...(lastNode ? { finalNodeId: lastNode.id } : {}),
  };
  if (lastNode && activeStageId && hooks.onStageCompleted) {
    await hooks.onStageCompleted(activeStageId, lastNode);
  }
  if (hooks.onCompleted) {
    await hooks.onCompleted(result, lastNode);
  }
  return result;
}

async function executeWorkflowNode(
  context: WorkflowNodeExecutionContext,
  node: WorkflowNodeSpec,
): Promise<WorkflowNodeExecutionResult> {
  const executor = WORKFLOW_NODE_EXECUTORS[node.type] ?? executeAssignmentNode;
  return executor(context, node);
}

async function executeCompleteNode(): Promise<WorkflowNodeExecutionResult> {
  return {
    dispatches: [],
    outcome: 'success',
    completionStatus: 'done',
  };
}

async function executeAssignmentNode(
  context: WorkflowNodeExecutionContext,
  node: WorkflowNodeSpec,
): Promise<WorkflowNodeExecutionResult> {
  const assignment = buildWorkflowDispatchAssignment(context.spec, context.request, node);
  if (!assignment) {
    return {
      dispatches: [],
      outcome: 'success',
    };
  }

  const dispatch = await context.runAssignment(assignment, node);
  const outgoingEdges = context.spec.workflow?.edges.filter(edge => edge.from === node.id) ?? [];
  return {
    dispatches: [dispatch],
    outcome: inferWorkflowOutcome(node, dispatch, outgoingEdges.map(edge => edge.when)),
  };
}

async function executeWorklistNode(
  context: WorkflowNodeExecutionContext,
  node: WorkflowNodeSpec,
): Promise<WorkflowNodeExecutionResult> {
  const artifactId = node.worklistArtifactId ?? node.producesArtifacts?.[0];
  const workerRoleId = node.workerRoleId ?? node.roleId;
  if (!artifactId || !workerRoleId) {
    return {
      dispatches: [],
      outcome: 'failure',
    };
  }

  const mode = node.worklistMode ?? (node.replenish === 'when_empty' ? 'replenishing' : 'finite');
  const worklist = createEmptyRuntimeWorklist(node, artifactId, workerRoleId, mode);
  const dispatches: TaskDispatch[] = [];
  const maxBatches = Math.max(
    1,
    node.maxBatches ?? (mode === 'replenishing' ? specDefaultMaxBatches(context.spec) : 1),
  );

  for (let batchIndex = 0; batchIndex < maxBatches; batchIndex += 1) {
    let loaded = await ensureWorklistLoaded(context, node, worklist, artifactId, batchIndex > 0);
    if (!loaded) {
      return {
        dispatches,
        outcome: batchIndex === 0 ? 'failure' : 'success',
      };
    }

    synchronizeRuntimeWorklist(worklist, loaded.document);
    await publishWorklistUpdate(context, node, worklist);

    let processedItem = false;
    while (true) {
      const nextItem = selectNextReadyItem(loaded.document, worklist);
      if (!nextItem) {
        break;
      }

      processedItem = true;
      const assignment = buildWorklistItemAssignment(context.spec, context.request, node, workerRoleId, nextItem);
      markWorkItemRunning(worklist, nextItem);
      updateDocumentItemStatus(loaded.document, nextItem.id, 'running', worklist.items[nextItem.id]?.attempts ?? 1);
      await persistWorklistDocument(context.spec, artifactId, loaded);
      await publishWorklistUpdate(context, node, worklist);

      const dispatch = await context.runAssignment(assignment, node);
      dispatches.push(dispatch);
      const finalStatus = markWorkItemFromDispatch(worklist, nextItem, dispatch);
      updateDocumentItemStatus(
        loaded.document,
        nextItem.id,
        finalStatus,
        worklist.items[nextItem.id]?.attempts ?? 1,
      );
      await persistWorklistDocument(context.spec, artifactId, loaded);
      await publishWorklistUpdate(context, node, worklist);

      if (finalStatus === 'failed' && (node.stopOnItemFailure ?? true)) {
        return {
          dispatches,
          outcome: 'failure',
        };
      }
    }

    if (hasBlockingFailure(loaded.document, worklist)) {
      return {
        dispatches,
        outcome: 'failure',
      };
    }

    const hasPending = loaded.document.items.some(item => {
      const runtimeItem = worklist.items[item.id];
      return (runtimeItem?.status ?? normalizeWorkItemStatus(item.status)) === 'pending';
    });
    if (hasPending && !processedItem) {
      return {
        dispatches,
        outcome: 'failure',
      };
    }

    if (mode !== 'replenishing' || node.replenish !== 'when_empty') {
      return {
        dispatches,
        outcome: worklist.failedItemIds.length > 0 ? 'failure' : 'success',
      };
    }

    if (!node.plannerRoleId || !node.plannerPrompt || batchIndex + 1 >= maxBatches) {
      return {
        dispatches,
        outcome: worklist.failedItemIds.length > 0 ? 'failure' : 'success',
      };
    }

    const beforePending = countPendingItems(worklist);
    const plannerDispatch = await runWorklistPlanner(context, node, worklist, artifactId);
    if (!plannerDispatch) {
      return {
        dispatches,
        outcome: worklist.failedItemIds.length > 0 ? 'failure' : 'success',
      };
    }

    dispatches.push(plannerDispatch);
    loaded = await loadWorklistDocument(context.spec, artifactId);
    if (!loaded) {
      return {
        dispatches,
        outcome: 'failure',
      };
    }

    synchronizeRuntimeWorklist(worklist, loaded.document);
    await publishWorklistUpdate(context, node, worklist);

    if (countPendingItems(worklist) <= beforePending) {
      return {
        dispatches,
        outcome: worklist.failedItemIds.length > 0 ? 'failure' : 'success',
      };
    }
  }

  return {
    dispatches,
    outcome: worklist.failedItemIds.length > 0 ? 'failure' : 'success',
  };
}

function specDefaultMaxBatches(spec: WorkspaceSpec): number {
  return Math.max(1, spec.completionPolicy?.maxIterations ?? 3);
}

async function ensureWorklistLoaded(
  context: WorkflowNodeExecutionContext,
  node: WorkflowNodeSpec,
  worklist: WorkflowWorklistRuntimeState,
  artifactId: string,
  allowReplan: boolean,
): Promise<LoadedWorklistDocument | undefined> {
  let loaded = await loadWorklistDocument(context.spec, artifactId);
  if (loaded && loaded.document.items.length > 0) {
    return loaded;
  }

  if (!node.plannerRoleId || !node.plannerPrompt) {
    return loaded;
  }

  if (allowReplan || !loaded || loaded.document.items.length === 0) {
    const plannerDispatch = await runWorklistPlanner(context, node, worklist, artifactId);
    if (!plannerDispatch) {
      return loaded;
    }
  }

  return loadWorklistDocument(context.spec, artifactId);
}

async function runWorklistPlanner(
  context: WorkflowNodeExecutionContext,
  node: WorkflowNodeSpec,
  worklist: WorkflowWorklistRuntimeState,
  artifactId: string,
): Promise<TaskDispatch | undefined> {
  const role = context.spec.roles.find(value => value.id === node.plannerRoleId);
  if (!role) {
    return undefined;
  }

  const assignment = buildWorklistPlannerAssignment(
    context.spec,
    context.request,
    node,
    role,
    artifactId,
    worklist,
  );
  const dispatch = await context.runAssignment(assignment, node);
  worklist.batchCount += 1;
  worklist.lastUpdatedAt = new Date().toISOString();
  await publishWorklistUpdate(context, node, worklist);
  return dispatch;
}

function buildWorklistPlannerAssignment(
  spec: WorkspaceSpec,
  request: WorkspaceTurnRequest,
  node: WorkflowNodeSpec,
  role: RoleSpec,
  artifactId: string,
  worklist: WorkflowWorklistRuntimeState,
): WorkspaceTurnAssignment {
  const artifact = mustFindArtifact(spec, artifactId);
  const artifactPath = resolveArtifactAbsolutePath(spec, artifact.path);
  return {
    roleId: role.id,
    summary: node.title ? `${node.title} planner` : `Plan worklist for ${node.id}`,
    provider: resolveWorkflowNodeProvider(spec, role, node),
    model: resolveWorkflowNodeModel(spec, role, node),
    instruction: [
      `You are preparing structured work items for workflow node "${node.title ?? node.id}".`,
      node.stageId ? `Current stage: ${node.stageId}.` : null,
      `Write or update the worklist JSON at: ${artifactPath}.`,
      'Use valid JSON only.',
      'Preferred shape: {"version":1,"mode":"finite","items":[{"id":"task-id","title":"Short title","description":"Detailed instruction","status":"pending","attempts":0,"maxAttempts":2}]}',
      'A top-level "tasks" array is also accepted for compatibility, but prefer "items".',
      worklist.completedItemIds.length > 0
        ? `Already completed item ids: ${worklist.completedItemIds.join(', ')}.`
        : null,
      worklist.failedItemIds.length > 0
        ? `Previously failed item ids: ${worklist.failedItemIds.join(', ')}. Avoid reusing them unless you are explicitly retrying with a better plan.`
        : null,
      node.plannerPrompt ? `Planning instructions: ${node.plannerPrompt}` : null,
      `Original user request: ${request.message}`,
      'Only include actionable work items that should be executed next.',
    ]
      .filter(Boolean)
      .join('\n'),
    visibility:
      node.visibility ??
      request.visibility ??
      spec.activityPolicy?.defaultVisibility ??
      'public',
    workflowNodeId: node.id,
    ...(node.stageId ? { stageId: node.stageId } : {}),
  };
}

function buildWorklistItemAssignment(
  spec: WorkspaceSpec,
  request: WorkspaceTurnRequest,
  node: WorkflowNodeSpec,
  workerRoleId: string,
  item: WorkflowWorkItem,
): WorkspaceTurnAssignment {
  const role = spec.roles.find(value => value.id === workerRoleId);
  if (!role) {
    throw new Error(`Unknown worklist worker role: ${workerRoleId}`);
  }

  return {
    roleId: role.id,
    summary: `Work item ${item.id}: ${item.title}`,
    provider: resolveWorkflowNodeProvider(spec, role, node),
    model: resolveWorkflowNodeModel(spec, role, node),
    instruction: buildWorklistItemInstruction(node, item, request.message),
    visibility:
      node.visibility ??
      request.visibility ??
      spec.activityPolicy?.defaultVisibility ??
      'public',
    workflowNodeId: node.id,
    ...(node.stageId ? { stageId: node.stageId } : {}),
    workItemId: item.id,
  };
}

function buildWorklistItemInstruction(
  node: WorkflowNodeSpec,
  item: WorkflowWorkItem,
  requestMessage: string,
): string {
  const template = node.itemPromptTemplate?.trim();
  if (template) {
    return applyTemplate(template, item, requestMessage);
  }

  return [
    `You are executing work item "${item.title}" (${item.id}).`,
    `Item description:\n${item.description}`,
    item.goalsFile ? `Goals file: ${item.goalsFile}` : null,
    item.referenceFiles?.length ? `Reference files: ${item.referenceFiles.join(', ')}` : null,
    item.files?.length ? `Target files: ${item.files.join(', ')}` : null,
    item.acceptanceCriteria?.length
      ? `Acceptance criteria:\n${item.acceptanceCriteria.map(value => `- ${value}`).join('\n')}`
      : null,
    `Original user request: ${requestMessage}`,
    'Complete only this work item and report what changed.',
  ]
    .filter(Boolean)
    .join('\n');
}

function applyTemplate(
  template: string,
  item: WorkflowWorkItem,
  requestMessage: string,
): string {
  return template
    .replaceAll('{{id}}', item.id)
    .replaceAll('{{title}}', item.title)
    .replaceAll('{{description}}', item.description)
    .replaceAll('{{request}}', requestMessage);
}

function createEmptyRuntimeWorklist(
  node: WorkflowNodeSpec,
  artifactId: string,
  workerRoleId: string,
  mode: WorkflowWorklistMode,
): WorkflowWorklistRuntimeState {
  return {
    nodeId: node.id,
    artifactId,
    mode,
    ...(node.plannerRoleId ? { plannerRoleId: node.plannerRoleId } : {}),
    workerRoleId,
    batchCount: 0,
    completedItemIds: [],
    failedItemIds: [],
    items: {},
    lastUpdatedAt: new Date().toISOString(),
  };
}

async function publishWorklistUpdate(
  context: WorkflowNodeExecutionContext,
  node: WorkflowNodeSpec,
  worklist: WorkflowWorklistRuntimeState,
): Promise<void> {
  worklist.lastUpdatedAt = new Date().toISOString();
  if (context.hooks.onWorklistUpdated) {
    await context.hooks.onWorklistUpdated(node, cloneWorklistState(worklist));
  }
}

function cloneWorklistState(worklist: WorkflowWorklistRuntimeState): WorkflowWorklistRuntimeState {
  return {
    ...worklist,
    completedItemIds: [...worklist.completedItemIds],
    failedItemIds: [...worklist.failedItemIds],
    items: Object.fromEntries(
      Object.entries(worklist.items).map(([itemId, state]) => [itemId, { ...state }]),
    ),
  };
}

async function loadWorklistDocument(
  spec: WorkspaceSpec,
  artifactId: string,
): Promise<LoadedWorklistDocument | undefined> {
  const artifact = spec.artifacts?.find(value => value.id === artifactId);
  if (!artifact) {
    return undefined;
  }

  const artifactPath = resolveArtifactAbsolutePath(spec, artifact.path);
  try {
    const raw = await readFile(artifactPath, 'utf8');
    return parseWorklistDocument(raw);
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    if (message.includes('ENOENT')) {
      return {
        document: {
          version: 1,
          items: [],
        },
        sourceKey: 'items',
      };
    }
    throw error;
  }
}

function parseWorklistDocument(raw: string): LoadedWorklistDocument {
  const parsed = JSON.parse(raw) as
    | WorkflowTaskListArtifact
    | { version?: number; mode?: WorkflowWorklistMode; summary?: string; tasks?: WorkflowWorkItem[] };
  const sourceKey = Array.isArray((parsed as { tasks?: unknown[] }).tasks) ? 'tasks' : 'items';
  const items = sourceKey === 'tasks'
    ? (parsed as { tasks: WorkflowWorkItem[] }).tasks
    : (parsed as WorkflowTaskListArtifact).items;

  return {
    document: {
      version: 1,
      ...(parsed.mode ? { mode: parsed.mode } : {}),
      ...(parsed.summary ? { summary: parsed.summary } : {}),
      items: Array.isArray(items) ? items.map(normalizeWorkItem) : [],
    },
    sourceKey,
  };
}

async function persistWorklistDocument(
  spec: WorkspaceSpec,
  artifactId: string,
  loaded: LoadedWorklistDocument,
): Promise<void> {
  const artifact = mustFindArtifact(spec, artifactId);
  const artifactPath = resolveArtifactAbsolutePath(spec, artifact.path);
  await mkdir(path.dirname(artifactPath), { recursive: true });
  const payload =
    loaded.sourceKey === 'tasks'
      ? {
          version: loaded.document.version,
          ...(loaded.document.mode ? { mode: loaded.document.mode } : {}),
          ...(loaded.document.summary ? { summary: loaded.document.summary } : {}),
          tasks: loaded.document.items,
        }
      : loaded.document;
  await writeFile(`${artifactPath}`, `${JSON.stringify(payload, null, 2)}\n`, 'utf8');
}

function resolveArtifactAbsolutePath(spec: WorkspaceSpec, artifactPath: string): string {
  if (path.isAbsolute(artifactPath)) {
    return artifactPath;
  }
  return spec.cwd ? path.join(spec.cwd, artifactPath) : artifactPath;
}

function mustFindArtifact(spec: WorkspaceSpec, artifactId: string) {
  const artifact = spec.artifacts?.find(value => value.id === artifactId);
  if (!artifact) {
    throw new Error(`Unknown workflow artifact: ${artifactId}`);
  }
  return artifact;
}

function normalizeWorkItem(item: WorkflowWorkItem): WorkflowWorkItem {
  return {
    ...item,
    status: normalizeWorkItemStatus(item.status),
    attempts: Math.max(0, item.attempts ?? 0),
  };
}

function normalizeWorkItemStatus(status: WorkflowWorkItem['status']): WorkflowWorkItemStatus {
  switch (status) {
    case 'running':
    case 'completed':
    case 'failed':
    case 'blocked':
    case 'discarded':
      return status;
    case 'pending':
    default:
      return 'pending';
  }
}

function synchronizeRuntimeWorklist(
  worklist: WorkflowWorklistRuntimeState,
  document: WorkflowTaskListArtifact,
): void {
  if (document.mode) {
    worklist.mode = document.mode;
  }

  const completedItemIds: string[] = [];
  const failedItemIds: string[] = [];
  for (const item of document.items) {
    const previous = worklist.items[item.id];
    const status = previous?.status ?? normalizeWorkItemStatus(item.status);
    const attempts = Math.max(previous?.attempts ?? 0, item.attempts ?? 0);
    const nextState = {
      itemId: item.id,
      title: item.title,
      status,
      attempts,
      ...(item.maxAttempts !== undefined ? { maxAttempts: item.maxAttempts } : {}),
      ...(previous?.dispatchId ? { dispatchId: previous.dispatchId } : {}),
      ...(previous?.lastSummary ? { lastSummary: previous.lastSummary } : {}),
      updatedAt: new Date().toISOString(),
    };
    worklist.items[item.id] = nextState;
    if (status === 'completed') {
      completedItemIds.push(item.id);
    }
    if (status === 'failed' || status === 'discarded') {
      failedItemIds.push(item.id);
    }
  }
  worklist.completedItemIds = completedItemIds;
  worklist.failedItemIds = failedItemIds;
  worklist.lastUpdatedAt = new Date().toISOString();
}

function selectNextReadyItem(
  document: WorkflowTaskListArtifact,
  worklist: WorkflowWorklistRuntimeState,
): WorkflowWorkItem | undefined {
  return document.items.find(item => {
    const runtimeItem = worklist.items[item.id];
    const status = runtimeItem?.status ?? normalizeWorkItemStatus(item.status);
    if (status !== 'pending') {
      return false;
    }

    const dependencies = item.dependsOn ?? [];
    return dependencies.every(dependencyId => worklist.completedItemIds.includes(dependencyId));
  });
}

function markWorkItemRunning(
  worklist: WorkflowWorklistRuntimeState,
  item: WorkflowWorkItem,
): void {
  const previous = worklist.items[item.id];
  worklist.items[item.id] = {
    itemId: item.id,
    title: item.title,
    status: 'running',
    attempts: (previous?.attempts ?? 0) + 1,
    ...(item.maxAttempts !== undefined ? { maxAttempts: item.maxAttempts } : {}),
    ...(previous?.dispatchId ? { dispatchId: previous.dispatchId } : {}),
    ...(previous?.lastSummary ? { lastSummary: previous.lastSummary } : {}),
    updatedAt: new Date().toISOString(),
  };
  worklist.lastUpdatedAt = new Date().toISOString();
}

function markWorkItemFromDispatch(
  worklist: WorkflowWorklistRuntimeState,
  item: WorkflowWorkItem,
  dispatch: TaskDispatch,
): WorkflowWorkItemStatus {
  const previous = worklist.items[item.id];
  const maxAttempts = item.maxAttempts ?? previous?.maxAttempts;
  const finalStatus: WorkflowWorkItemStatus =
    dispatch.status === 'completed'
      ? 'completed'
      : maxAttempts !== undefined && (previous?.attempts ?? 0) >= maxAttempts
        ? 'discarded'
        : 'failed';

  worklist.items[item.id] = {
    itemId: item.id,
    title: item.title,
    status: finalStatus,
    attempts: previous?.attempts ?? 1,
    ...(maxAttempts !== undefined ? { maxAttempts } : {}),
    dispatchId: dispatch.dispatchId,
    ...(dispatch.lastSummary ? { lastSummary: dispatch.lastSummary } : dispatch.resultText ? { lastSummary: summarizeResult(dispatch.resultText) } : {}),
    updatedAt: new Date().toISOString(),
  };

  if (finalStatus === 'completed') {
    worklist.completedItemIds = uniqueIds([...worklist.completedItemIds, item.id]);
    worklist.failedItemIds = worklist.failedItemIds.filter(value => value !== item.id);
  } else {
    worklist.failedItemIds = uniqueIds([...worklist.failedItemIds, item.id]);
    worklist.completedItemIds = worklist.completedItemIds.filter(value => value !== item.id);
  }

  worklist.lastUpdatedAt = new Date().toISOString();
  return finalStatus;
}

function updateDocumentItemStatus(
  document: WorkflowTaskListArtifact,
  itemId: string,
  status: WorkflowWorkItemStatus,
  attempts: number,
): void {
  const item = document.items.find(value => value.id === itemId);
  if (!item) {
    return;
  }
  item.status = status;
  item.attempts = attempts;
}

function hasBlockingFailure(
  document: WorkflowTaskListArtifact,
  worklist: WorkflowWorklistRuntimeState,
): boolean {
  return document.items.some(item => {
    const runtimeItem = worklist.items[item.id];
    const status = runtimeItem?.status ?? normalizeWorkItemStatus(item.status);
    return status === 'failed' || status === 'discarded';
  });
}

function countPendingItems(worklist: WorkflowWorklistRuntimeState): number {
  return Object.values(worklist.items).filter(item => item.status === 'pending').length;
}

function uniqueIds(values: string[]): string[] {
  return Array.from(new Set(values));
}

function summarizeResult(resultText: string): string {
  return resultText.trim().split(/\r?\n/).find(Boolean)?.slice(0, 240) ?? resultText.slice(0, 240);
}

function resolveCompletionStatus(
  spec: WorkspaceSpec,
  nodeId: string,
  outcome: WorkflowEdgeCondition,
): CompletionStatus | undefined {
  if (spec.completionPolicy?.successNodeIds?.includes(nodeId)) {
    return 'done';
  }
  if (spec.completionPolicy?.failureNodeIds?.includes(nodeId)) {
    return outcome === 'failure' || outcome === 'fail' ? 'crash' : 'discarded';
  }
  return undefined;
}

function resolveNextWorkflowNodeId(
  workflow: WorkflowSpec,
  nodeId: string,
  outcome: WorkflowEdgeCondition,
): string | undefined {
  const outgoing = workflow.edges.filter(edge => edge.from === nodeId);
  const exact = outgoing.find(edge => edge.when === outcome);
  if (exact) {
    return exact.to;
  }

  if (
    outcome !== 'success' &&
    ['pass', 'approved', 'improved'].includes(outcome) &&
    outgoing.some(edge => edge.when === 'success')
  ) {
    return outgoing.find(edge => edge.when === 'success')?.to;
  }

  if (
    outcome !== 'failure' &&
    ['fail', 'rejected', 'equal_or_worse', 'crash', 'timeout'].includes(outcome) &&
    outgoing.some(edge => edge.when === 'failure')
  ) {
    return outgoing.find(edge => edge.when === 'failure')?.to;
  }

  return outgoing.find(edge => edge.when === 'always')?.to;
}

function inferWorkflowOutcome(
  node: WorkflowNodeSpec,
  dispatch: TaskDispatch | undefined,
  availableConditions: WorkflowEdgeCondition[],
): WorkflowEdgeCondition {
  if (!dispatch) {
    return availableConditions.includes('success') ? 'success' : 'always';
  }

  if (dispatch.status === 'failed') {
    return pickFailureCondition(availableConditions);
  }

  if (dispatch.status === 'stopped') {
    return availableConditions.includes('timeout')
      ? 'timeout'
      : pickFailureCondition(availableConditions);
  }

  const text = `${dispatch.resultText ?? ''}\n${dispatch.lastSummary ?? ''}`.toLowerCase();
  const positive = scoreText(text, POSITIVE_PATTERNS);
  const negative = scoreText(text, NEGATIVE_PATTERNS);

  if (availableConditions.includes('approved') || availableConditions.includes('rejected')) {
    if (negative > positive) {
      return availableConditions.includes('rejected')
        ? 'rejected'
        : pickFailureCondition(availableConditions);
    }
    return availableConditions.includes('approved') ? 'approved' : 'success';
  }

  if (availableConditions.includes('pass') || availableConditions.includes('fail')) {
    if (negative > positive) {
      return availableConditions.includes('fail')
        ? 'fail'
        : pickFailureCondition(availableConditions);
    }
    return availableConditions.includes('pass') ? 'pass' : 'success';
  }

  if (availableConditions.includes('improved') || availableConditions.includes('equal_or_worse')) {
    if (negative > positive) {
      return availableConditions.includes('equal_or_worse')
        ? 'equal_or_worse'
        : pickFailureCondition(availableConditions);
    }
    return availableConditions.includes('improved') ? 'improved' : 'success';
  }

  if (node.type === 'review' && negative > positive) {
    return availableConditions.includes('rejected')
      ? 'rejected'
      : pickFailureCondition(availableConditions);
  }

  return availableConditions.includes('success') ? 'success' : 'always';
}

function pickFailureCondition(
  availableConditions: WorkflowEdgeCondition[],
): WorkflowEdgeCondition {
  for (const candidate of [
    'failure',
    'fail',
    'rejected',
    'equal_or_worse',
    'crash',
    'timeout',
    'exhausted',
  ] as const) {
    if (availableConditions.includes(candidate)) {
      return candidate;
    }
  }
  return availableConditions.includes('success') ? 'success' : 'always';
}

function scoreText(text: string, patterns: RegExp[]): number {
  return patterns.reduce((score, pattern) => score + (pattern.test(text) ? 1 : 0), 0);
}

const POSITIVE_PATTERNS = [
  /\bapprove(d)?\b/,
  /\blgtm\b/,
  /\bship it\b/,
  /\bpass(ed)?\b/,
  /\bsuccess(ful)?\b/,
  /\blooks good\b/,
  /\bimprov(ed|ement)?\b/,
];

const NEGATIVE_PATTERNS = [
  /\breject(ed|ion)?\b/,
  /\bchanges requested\b/,
  /\bneeds changes\b/,
  /\bfail(ed|ure)?\b/,
  /\berror\b/,
  /\bregression\b/,
  /\bblock(ed|er)?\b/,
  /\bworse\b/,
];
