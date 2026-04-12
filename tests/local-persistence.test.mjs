import assert from 'node:assert/strict';
import { access, mkdtemp } from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import test from 'node:test';

import {
  ClaudeAgentWorkspace,
  CodexSdkWorkspace,
  LocalWorkspacePersistence,
  createClaudeWorkspaceProfile,
  createCodexWorkspaceProfile,
  createCodingStudioTemplate,
  instantiateWorkspace,
} from '../dist/index.js';

function makeRuntimeState(spec) {
  return {
    workspaceId: spec.id,
    status: 'running',
    provider: spec.provider,
    sessionId: 'root-session',
    startedAt: new Date().toISOString(),
    roles: Object.fromEntries(spec.roles.map(role => [role.id, role])),
    dispatches: {},
    members: Object.fromEntries(
      spec.roles.map(role => [
        role.id,
        {
          memberId: role.id,
          workspaceId: spec.id,
          roleId: role.id,
          roleName: role.name,
          ...(role.direct !== undefined ? { direct: role.direct } : {}),
          status: 'idle',
        },
      ]),
    ),
    activities: [],
    workflowRuntime: {
      mode: 'group_chat',
    },
  };
}

test('claude adapter restores and deletes a persisted workspace', async () => {
  const cwd = await mkdtemp(path.join(os.tmpdir(), 'mar-ts-claude-'));
  const template = createCodingStudioTemplate();
  const spec = instantiateWorkspace(
    template,
    { id: 'claude-restore', name: 'Claude Restore', cwd },
    createClaudeWorkspaceProfile(),
  );
  const persistence = LocalWorkspacePersistence.fromSpec(spec);
  await persistence.initializeWorkspace(spec);
  await persistence.persistRuntime({
    state: makeRuntimeState(spec),
    events: [],
    providerState: {
      workspaceId: spec.id,
      provider: spec.provider,
      rootConversationId: 'claude-root-session',
      memberBindings: {},
      updatedAt: new Date().toISOString(),
    },
  });

  const workspace = await ClaudeAgentWorkspace.restoreFromLocal({ cwd, workspaceId: spec.id });
  assert.equal(workspace.getSnapshot().sessionId, 'root-session');
  assert.equal(workspace.getPersistenceRoot(), path.join(cwd, '.multi-agent-runtime', spec.id));

  await workspace.deleteWorkspace();
  await assert.rejects(access(path.join(cwd, '.multi-agent-runtime', spec.id)));
});

test('codex adapter restores and deletes a persisted workspace', async () => {
  const cwd = await mkdtemp(path.join(os.tmpdir(), 'mar-ts-codex-'));
  const template = createCodingStudioTemplate();
  const spec = instantiateWorkspace(
    template,
    { id: 'codex-restore', name: 'Codex Restore', cwd },
    createCodexWorkspaceProfile(),
  );
  const persistence = LocalWorkspacePersistence.fromSpec(spec);
  await persistence.initializeWorkspace(spec);
  await persistence.persistRuntime({
    state: makeRuntimeState(spec),
    events: [],
    providerState: {
      workspaceId: spec.id,
      provider: spec.provider,
      rootConversationId: 'codex-root-thread',
      memberBindings: {
        prd: {
          roleId: 'prd',
          providerConversationId: 'thread-prd-123',
          kind: 'thread',
          updatedAt: new Date().toISOString(),
        },
      },
      updatedAt: new Date().toISOString(),
    },
  });

  const workspace = await CodexSdkWorkspace.restoreFromLocal({ cwd, workspaceId: spec.id });
  assert.equal(workspace.getSnapshot().sessionId, 'root-session');
  assert.equal(workspace.getPersistenceRoot(), path.join(cwd, '.multi-agent-runtime', spec.id));

  await workspace.deleteWorkspace();
  await assert.rejects(access(path.join(cwd, '.multi-agent-runtime', spec.id)));
});
