import assert from 'node:assert/strict';
import fs from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';

export async function createScratchDir(prefix) {
  return fs.mkdtemp(path.join(os.tmpdir(), `${prefix}-`));
}

export async function readRequiredFile(filePath) {
  try {
    return await fs.readFile(filePath, 'utf8');
  } catch (error) {
    throw new Error(`Expected file to exist: ${filePath}\n${error}`);
  }
}

export async function runWorkspaceScenario({
  workspace,
  task,
  expectedRoleId,
  outputFile,
  timeoutMs = 180_000,
  resultTimeoutMs = 20_000,
}) {
  const events = [];
  const stopListening = workspace.onEvent(event => {
    events.push(event);
  });

  try {
    await workspace.start();
    const dispatch = await workspace.runRoleTask(task, { timeoutMs, resultTimeoutMs });
    const fileText = await readRequiredFile(outputFile);

    const initializedEvents = events.filter(event => event.type === 'workspace.initialized');
    assert.ok(initializedEvents.length >= 1, 'Expected at least one workspace.initialized event');

    const sessionAwareInit = initializedEvents.find(event => typeof event.sessionId === 'string' && event.sessionId.length > 0);
    assert.ok(sessionAwareInit, 'Expected a workspace.initialized event with a sessionId after dispatch started');

    const queuedEvent = events.find(event => event.type === 'dispatch.queued' && event.dispatch.dispatchId === dispatch.dispatchId);
    const startedEvent = events.find(event => event.type === 'dispatch.started' && event.dispatch.dispatchId === dispatch.dispatchId);
    const completedEvent = events.find(event => event.type === 'dispatch.completed' && event.dispatch.dispatchId === dispatch.dispatchId);
    const resultEvent = events.find(event => event.type === 'dispatch.result' && event.dispatch.dispatchId === dispatch.dispatchId);

    assert.ok(queuedEvent, 'Expected dispatch.queued event');
    assert.ok(startedEvent, 'Expected dispatch.started event');
    assert.ok(completedEvent, 'Expected dispatch.completed event');
    assert.ok(resultEvent, 'Expected dispatch.result event');

    assert.equal(dispatch.roleId, expectedRoleId);
    assert.equal(dispatch.status, 'completed');
    assert.ok(dispatch.providerTaskId, 'Expected providerTaskId on completed dispatch');
    assert.ok(dispatch.toolUseId, 'Expected toolUseId on completed dispatch');
    assert.ok(dispatch.resultText && dispatch.resultText.trim().length > 0, 'Expected non-empty resultText');
    assert.ok(fileText.trim().length > 0, 'Expected generated file to be non-empty');

    return {
      dispatch,
      events,
      fileText,
      outputFile,
    };
  } finally {
    stopListening();
    await workspace.close();
  }
}

export function countMarkdownLinks(text) {
  return (text.match(/\[[^\]]+\]\(https?:\/\/[^)]+\)/g) ?? []).length;
}
