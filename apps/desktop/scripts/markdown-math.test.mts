import assert from 'node:assert/strict';
import test from 'node:test';
import { unified } from 'unified';
import remarkParse from 'remark-parse';
import remarkMath from 'remark-math';
import { remarkLatex } from '../src/lib/remarkLatex.ts';

const parser = unified().use(remarkParse).use(remarkMath).use(remarkLatex);

test('TeX delimiters survive streaming prefixes and Markdown containers', () => {
  const source = '\\[\n\\begin{bmatrix}1 & 2 \\\\ 3 & 4\\end{bmatrix}\n\\]';
  for (let end = 0; end <= source.length; end++) assert.doesNotThrow(() => parser.parse(source.slice(0, end)));
  const equation = parser.parse(source).children[0];
  assert.equal(equation.type, 'paragraph');
  if (equation.type !== 'paragraph') throw new Error('Expected paragraph');
  assert.equal(equation.children[0].type, 'inlineMath');
  assert.ok(JSON.stringify(equation).includes('math-display'));
  for (const content of ['> \\[\n> x+y\n> \\]', '- \\(x+y\\)']) {
    assert.ok(JSON.stringify(parser.parse(content)).includes('inlineMath'));
  }
});

test('code, escaped delimiters and link destinations remain literal', () => {
  for (const content of ['`\\(literal\\)`', '```js\nconst x = "\\(literal\\)"\n```', '\\\\(escaped\\\\)', '[link](https://example.com/\\(literal\\))']) {
    assert.ok(!JSON.stringify(parser.parse(content)).includes('inlineMath'), content);
  }
});
