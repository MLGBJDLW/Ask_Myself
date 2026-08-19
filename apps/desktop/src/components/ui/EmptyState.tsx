import { type ReactNode } from 'react';
import { Button } from './Button';

interface EmptyStateProps {
  icon: ReactNode;
  title: string;
  description: string;
  quote?: string;
  action?: {
    label: string;
    onClick: () => void;
  };
}

export function EmptyState({ icon, title, description, quote, action }: EmptyStateProps) {
  return (
    <div className="mx-auto flex max-w-md flex-col items-center justify-center px-5 py-14 text-center" data-theme-component="card">
      <div className="mb-4 rounded-xl border border-border bg-surface-2 p-3.5 text-text-tertiary shadow-sm">
        {icon}
      </div>
      <h3 className="mb-1.5 text-base font-semibold text-text-primary">{title}</h3>
      {description && (
        <p className="mb-5 max-w-sm text-sm leading-relaxed text-text-tertiary">{description}</p>
      )}
      {quote && (
        <blockquote className={`${action ? 'mb-5' : ''} max-w-sm border-l-2 border-accent/45 pl-3 text-left text-xs italic leading-relaxed text-text-tertiary`}>
          {quote}
        </blockquote>
      )}
      {action && (
        <Button variant="primary" size="sm" onClick={action.onClick}>
          {action.label}
        </Button>
      )}
    </div>
  );
}
