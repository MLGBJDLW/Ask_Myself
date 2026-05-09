import { type ReactNode } from 'react';
import { Button } from './Button';

interface EmptyStateProps {
  icon: ReactNode;
  title: string;
  description: string;
  action?: {
    label: string;
    onClick: () => void;
  };
}

export function EmptyState({ icon, title, description, action }: EmptyStateProps) {
  return (
    <div className="mx-auto flex max-w-md flex-col items-center justify-center px-5 py-14 text-center">
      <div className="mb-4 rounded-xl border border-border bg-surface-2 p-3.5 text-text-tertiary shadow-sm">
        {icon}
      </div>
      <h3 className="mb-1.5 text-base font-semibold text-text-primary">{title}</h3>
      {description && (
        <p className="mb-5 max-w-sm text-sm leading-relaxed text-text-tertiary">{description}</p>
      )}
      {action && (
        <Button variant="primary" size="sm" onClick={action.onClick}>
          {action.label}
        </Button>
      )}
    </div>
  );
}
