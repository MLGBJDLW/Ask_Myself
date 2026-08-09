/** Browser-side scenario builder that emits only the production v2 RunEvent
 * wire protocol. The Tauri listener never receives the concise scenario input. */
export const RUN_EVENT_FIXTURE_INIT_SCRIPT = String.raw`
(() => {
  const sequences = new Map();
  const answerOffsets = new Map();
  const thinkingOffsets = new Map();
  const blockGenerations = new Map();
  const toolRuns = new Map();
  const bytes = value => new TextEncoder().encode(value).byteLength;
  const owner = { id: 'e2e', name: 'E2E fixture', capability: 'test', description: 'Playwright fixture' };
  const capabilities = {
    inputStreaming: 'none', renderKind: 'generic', readOnly: true,
    destructive: false, concurrencySafe: true, interruptBehavior: 'block', resourceKeys: [],
  };
  const presentation = kind => ({
    visibility: kind === 'status' ? 'developer' : 'user',
    persistence: 'durable',
    displayKind: kind === 'outputDelta' ? 'output'
      : kind.startsWith('tool') ? 'tool'
      : kind === 'done' ? 'completion'
      : kind === 'error' ? 'error' : 'status',
    importance: kind === 'error' ? 'high' : 'normal',
  });
  const runFor = (conversationId, callId, toolName) => {
    const key = conversationId + ':' + callId;
    const existing = toolRuns.get(key);
    if (existing) return existing;
    const run = {
      callId, toolName, owner, status: 'running', arguments: '', renderKind: 'generic', capabilities,
    };
    toolRuns.set(key, run);
    return run;
  };
  const rotateBlocks = conversationId => {
    blockGenerations.set(conversationId, (blockGenerations.get(conversationId) || 0) + 1);
    answerOffsets.set(conversationId, 0);
    thinkingOffsets.set(conversationId, 0);
  };

  window.__toRunEventFixture = (eventName, input) => {
    if (eventName !== 'agent://run-event' || input.runEvent) return { eventName, payload: input };
    const conversationId = String(input.conversationId || 'conversation-e2e');
    const runId = String(input.runId || 'run-' + conversationId);
    const turnId = String(input.turnId || 'turn-' + conversationId);
    const type = String(input.type || 'status');
    if (type === 'thinking' && !String(input.content || '')) {
      return {
        eventName: 'agent://heartbeat',
        payload: { conversationId, runId, turnId },
      };
    }

    const eventSeq = (sequences.get(conversationId) || 0) + 1;
    sequences.set(conversationId, eventSeq);
    let kind = type;
    let phase = 'responding';
    let label = type;
    let status = 'running';
    let payload = { ...input };
    delete payload.conversationId;
    delete payload.runId;
    delete payload.turnId;
    delete payload.type;

    if (type === 'textDelta' || type === 'thinking') {
      const channel = type === 'thinking' ? 'thinking' : 'answer';
      const offsets = channel === 'thinking' ? thinkingOffsets : answerOffsets;
      const delta = String(type === 'thinking' ? input.content || '' : input.delta || '');
      const offset = offsets.get(conversationId) || 0;
      offsets.set(conversationId, offset + bytes(delta));
      kind = 'outputDelta';
      label = channel === 'thinking' ? 'Reasoning update' : 'Assistant response';
      const generation = blockGenerations.get(conversationId) || 0;
      payload = { blockId: channel + '-' + conversationId + '-' + generation, channel, offset, delta };
    } else if (type === 'toolCallStart' || type === 'toolRunStarted') {
      const source = input.run || input;
      const run = { ...runFor(conversationId, String(source.callId), String(source.toolName)), ...source, status: 'running' };
      toolRuns.set(conversationId + ':' + run.callId, run);
      kind = 'toolStarted'; phase = 'tooling'; label = run.toolName; status = 'running'; payload = { run };
      rotateBlocks(conversationId);
    } else if (type === 'toolCallProgress' || type === 'toolRunUpdated') {
      const source = input.run || input;
      const run = { ...runFor(conversationId, String(source.callId), String(source.toolName)), ...source, status: source.status || 'running' };
      toolRuns.set(conversationId + ':' + run.callId, run);
      kind = 'toolProgress'; phase = 'tooling'; label = run.toolName; status = run.status; payload = { run };
    } else if (type === 'toolCallResult' || type === 'toolRunCompleted') {
      const source = input.run || input;
      const run = {
        ...runFor(conversationId, String(source.callId), String(source.toolName)), ...source,
        status: source.status || (source.isError ? 'failed' : 'completed'),
      };
      toolRuns.set(conversationId + ':' + run.callId, run);
      kind = 'toolCompleted'; phase = 'tooling'; label = run.toolName; status = run.status; payload = { run };
      rotateBlocks(conversationId);
    } else if (type === 'done') {
      kind = 'done'; phase = 'done'; label = 'Final answer produced'; status = input.status || 'completed';
    } else if (type === 'error') {
      kind = 'error'; phase = 'done'; label = String(input.message || 'Agent execution failed'); status = input.status || 'failed';
    } else if (type === 'connectionState') {
      const connection = input.state || {};
      kind = 'recoveryAttempt';
      label = connection.state === 'reconnecting' ? 'Reconnecting to the provider'
        : connection.state === 'recovered' ? 'Provider connection recovered'
        : connection.state === 'degraded' ? 'Provider connection degraded'
        : connection.state === 'offline' ? 'Provider is offline'
        : connection.state === 'failed' ? 'Provider connection failed' : 'Provider connection update';
      payload = { state: connection };
    } else if (type === 'controllerStatus') {
      kind = 'status'; label = String(input.content || input.code || 'Controller status');
      payload = { content: input.content, tone: input.tone, code: input.code };
    } else if (type === 'usageUpdate') {
      kind = 'usageUpdated'; phase = 'accounting'; label = 'Token usage updated';
    } else if (type === 'autoCompacted') {
      kind = 'autoCompacted'; phase = 'compacting'; label = 'Conversation context compacted';
    } else if (type === 'steering') {
      kind = 'status'; label = String(input.content || 'Steering accepted'); status = 'accepted'; payload = { content: input.content };
    }

    return {
      eventName,
      payload: {
        conversationId,
        runEvent: {
          version: 2, runId, turnId, eventSeq, kind, phase, label, status, payload,
          ...presentation(kind), createdAt: new Date().toISOString(),
          ...(type === 'steering' ? { displayKind: 'steering', visibility: 'user' } : {}),
        },
      },
    };
  };
})();
`;
