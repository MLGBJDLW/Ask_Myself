import type { HTMLAttributes, ReactNode } from 'react';

export type BadgeVariant =
  | 'default'
  | 'neutral'
  | 'muted'
  | 'accent'
  | 'success'
  | 'warning'
  | 'danger'
  | 'info'
  | 'blue'
  | 'cyan'
  | 'teal'
  | 'purple'
  | 'pink'
  | 'orange'
  | 'amber'
  | 'slate';

const variantStyles: Record<BadgeVariant, string> = {
  default: 'border-border/60 bg-surface-3 text-text-secondary',
  neutral: 'border-border/55 bg-surface-2/85 text-text-secondary',
  muted: 'border-border/45 bg-surface-1/70 text-text-tertiary',
  accent: 'border-accent/25 bg-accent/10 text-accent',
  success: 'border-success/25 bg-success/10 text-success',
  warning: 'border-warning/25 bg-warning/10 text-warning',
  danger: 'border-danger/25 bg-danger/10 text-danger',
  info: 'border-info/25 bg-info/10 text-info',
  blue: 'border-blue-500/25 bg-blue-500/10 text-blue-400',
  cyan: 'border-cyan-500/25 bg-cyan-500/10 text-cyan-400',
  teal: 'border-teal-500/25 bg-teal-500/10 text-teal-400',
  purple: 'border-purple-500/25 bg-purple-500/10 text-purple-400',
  pink: 'border-pink-500/25 bg-pink-500/10 text-pink-400',
  orange: 'border-orange-500/25 bg-orange-500/10 text-orange-400',
  amber: 'border-amber-500/25 bg-amber-500/10 text-amber-400',
  slate: 'border-slate-500/25 bg-slate-500/10 text-slate-400',
};

interface BadgeProps extends HTMLAttributes<HTMLSpanElement> {
  variant?: BadgeVariant;
  icon?: ReactNode;
  children: ReactNode;
  className?: string;
}

export function Badge({ variant = 'default', icon, children, className = '', ...props }: BadgeProps) {
  return (
    <span
      className={`inline-flex items-center gap-1 rounded-full border px-2 py-0.5 text-[11px] font-medium ${variantStyles[variant]} ${className}`}
      {...props}
    >
      {icon}
      {children}
    </span>
  );
}
