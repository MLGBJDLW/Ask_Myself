import { providerCredentialScope } from '../src/lib/providerCredentials';

function assert(condition: unknown, message: string): asserts condition {
  if (!condition) throw new Error(message);
}

function assertEqual<T>(actual: T, expected: T, message: string): void {
  assert(Object.is(actual, expected), `${message}: expected ${String(expected)}, got ${String(actual)}`);
}

const alibabaChat = 'https://dashscope.aliyuncs.com/compatible-mode/v1';
assertEqual(
  providerCredentialScope('alibaba_model_studio', alibabaChat),
  'alibaba-model-studio:beijing',
  'canonical Alibaba chat endpoint should share its regional credential',
);

for (const endpoint of [
  'https://dashscope.aliyuncs.com/evil',
  `${alibabaChat}?workspace=other`,
  `${alibabaChat}#edited`,
  'http://dashscope.aliyuncs.com/compatible-mode/v1',
  'https://dashscope.aliyuncs.com:8443/compatible-mode/v1',
]) {
  assert(
    providerCredentialScope('alibaba_model_studio', endpoint).startsWith('endpoint:'),
    `edited endpoint must keep an isolated credential scope: ${endpoint}`,
  );
}

assertEqual(
  providerCredentialScope('open_ai', 'https://api.meta.ai/v1'),
  'meta-model-api',
  'the exact Meta Model API endpoint should keep its own credential scope',
);
for (const endpoint of [
  'https://api.meta.ai/v2',
  'https://api.meta.ai.evil.example/v1',
  'http://api.meta.ai/v1',
]) {
  assert(
    providerCredentialScope('open_ai', endpoint).startsWith('endpoint:'),
    `edited Meta endpoints must keep isolated credentials: ${endpoint}`,
  );
}

assertEqual(
  providerCredentialScope('zhipu', 'https://open.bigmodel.cn/api/paas/v4'),
  'zhipu:china',
  'China Zhipu Model API should keep its regional credential scope',
);
assertEqual(
  providerCredentialScope('zhipu', 'https://api.z.ai/api/paas/v4'),
  'zhipu:international',
  'international Z.ai Model API should keep a separate credential scope',
);
assertEqual(
  providerCredentialScope('zhipu'),
  'zhipu:china',
  'the legacy/default Zhipu provider identity should continue to resolve to China',
);
for (const endpoint of [
  'https://open.bigmodel.cn/api/coding/paas/v4',
  'https://api.z.ai/api/coding/paas/v4',
  'https://api.z.ai/api/paas/v4?workspace=other',
  'http://api.z.ai/api/paas/v4',
]) {
  assert(
    providerCredentialScope('zhipu', endpoint).startsWith('endpoint:'),
    `Coding Plan and edited Z.ai routes must keep isolated credentials: ${endpoint}`,
  );
}
