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
