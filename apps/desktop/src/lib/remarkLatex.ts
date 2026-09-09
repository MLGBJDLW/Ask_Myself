import type { Root } from 'mdast';
import type { InlineMath } from 'mdast-util-math';
import type { Extension as FromMarkdownExtension } from 'mdast-util-from-markdown';
import type { Extension, Tokenizer, State } from 'micromark-util-types';
import type { Plugin } from 'unified';

declare module 'micromark-util-types' {
  interface TokenTypeMap {
    latexMath: 'latexMath';
    latexMathSequence: 'latexMathSequence';
    latexMathData: 'latexMathData';
  }
}

// Parse TeX delimiters at the Markdown tokenizer boundary. Code, escaped
// backslashes and link destinations retain Markdown's own literal semantics.
const tokenize: Tokenizer = function (effects, ok, nok) {
  let closing = 41;
  const start: State = code => {
    effects.enter('latexMath');
    effects.enter('latexMathSequence');
    effects.consume(code);
    return opening;
  };
  const opening: State = code => {
    if (code !== 40 && code !== 91) return nok(code);
    closing = code === 40 ? 41 : 93;
    effects.consume(code);
    effects.exit('latexMathSequence');
    return body;
  };
  const body: State = code => {
    if (code === null) return nok(code);
    if (code === -5 || code === -4 || code === -3) {
      effects.enter('lineEnding');
      effects.consume(code);
      effects.exit('lineEnding');
      return body;
    }
    if (code === 92) {
      effects.enter('latexMathSequence');
      effects.consume(code);
      return afterSlash;
    }
    effects.enter('latexMathData');
    return content(code);
  };
  const content: State = code => {
    if (code === null || code === 92 || code === -5 || code === -4 || code === -3) {
      effects.exit('latexMathData');
      return body(code);
    }
    effects.consume(code);
    return content;
  };
  const afterSlash: State = code => {
    if (code === closing) {
      effects.consume(code);
      effects.exit('latexMathSequence');
      effects.exit('latexMath');
      return ok;
    }
    // A double backslash belongs to TeX (for example a matrix row break).
    if (code === 92) { effects.consume(code); effects.exit('latexMathSequence'); return body; }
    effects.exit('latexMathSequence');
    return body(code);
  };
  return start;
};

export const remarkLatex: Plugin<[], Root> = function () {
  const data = this.data() as { micromarkExtensions?: Extension[]; fromMarkdownExtensions?: FromMarkdownExtension[] };
  const syntax: Extension = { text: { 92: { name: 'latexMath', tokenize } } };
  const fromMarkdown: FromMarkdownExtension = {
    enter: {
      latexMath(token) {
        const source = this.sliceSerialize(token);
        const value = source.slice(2, -2).trim();
        this.enter({
          type: 'inlineMath', value,
          data: {
            hName: 'code',
            hProperties: { className: ['language-math', source[1] === '[' ? 'math-display' : 'math-inline'] },
            hChildren: [{ type: 'text', value }],
          },
        } as InlineMath, token);
        this.buffer();
      },
    },
    exit: { latexMath(token) { this.resume(); this.exit(token); } },
  };
  (data.micromarkExtensions ??= []).push(syntax);
  (data.fromMarkdownExtensions ??= []).push(fromMarkdown);
  return tree => {
    const visit = (node: Root | Root['children'][number]) => {
      if (node.type === 'code' && /^(?:latex|tex)$/i.test(node.lang ?? '')) node.lang = 'math';
      if ('children' in node) node.children.forEach(child => visit(child as Root['children'][number]));
    };
    visit(tree);
  };
};
