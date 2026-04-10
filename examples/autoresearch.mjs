import path from 'node:path';
import {
  ClaudeAgentWorkspace,
  createAutoresearchTemplate,
  createClaudeWorkspaceProfile,
  instantiateWorkspace,
} from '../dist/index.js';
import { attachConsoleLogger, createScratchDir, printFileIfExists } from './_shared.mjs';

const cwd = await createScratchDir('cteno-autoresearch');
const workspace = new ClaudeAgentWorkspace({
  spec: instantiateWorkspace(
    createAutoresearchTemplate(),
    {
      id: `autoresearch-${Date.now()}`,
      name: 'Autoresearch Smoke',
      cwd,
    },
    createClaudeWorkspaceProfile(),
  ),
});

const stopLogging = attachConsoleLogger(workspace, 'autoresearch');

try {
  await workspace.start();
  const dispatch = await workspace.runRoleTask({
    roleId: 'scout',
    summary: 'Research the product pattern behind @mentions',
    instruction:
      'Use web research if helpful, then create a concise markdown brief at research/10-scout/mention-patterns.md comparing how Slack, GitHub, or similar collaboration tools handle @mentions and directed attention. Include 3 short source links and a final section called Implications for Cteno.',
  }, { timeoutMs: 240000, resultTimeoutMs: 30000 });

  console.log('\nFINAL DISPATCH');
  console.log(JSON.stringify(dispatch, null, 2));
  await printFileIfExists(path.join(cwd, 'research/10-scout/mention-patterns.md'));
} finally {
  stopLogging();
  await workspace.close();
}
