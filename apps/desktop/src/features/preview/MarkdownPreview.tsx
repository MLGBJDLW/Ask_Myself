import ReactMarkdown from 'react-markdown';
import {
  markdownComponents,
  markdownRemarkPlugins,
  rehypePlugins,
} from '../../components/chat/markdownComponents';

export default function MarkdownPreview({ content }: { content: string }) {
  return (
    <div className="prose prose-sm prose-invert max-w-none px-5 py-4 text-text-primary">
      <ReactMarkdown
        remarkPlugins={markdownRemarkPlugins}
        rehypePlugins={rehypePlugins}
        components={markdownComponents}
      >
        {content}
      </ReactMarkdown>
    </div>
  );
}
