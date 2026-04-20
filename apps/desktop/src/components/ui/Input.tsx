import { forwardRef, type InputHTMLAttributes } from 'react';

interface InputProps extends InputHTMLAttributes<HTMLInputElement> {
  icon?: React.ReactNode;
  error?: string;
}

export const Input = forwardRef<HTMLInputElement, InputProps>(
  ({ icon, error, className = '', ...props }, ref) => {
    return (
      <div className="relative w-full">
        {icon && (
          <div className="absolute left-3 top-1/2 -translate-y-1/2 text-text-tertiary pointer-events-none">
            {icon}
          </div>
        )}
        <input
          ref={ref}
          className={`
            w-full h-10 bg-surface-1 border border-border rounded-md
            text-sm text-text-primary placeholder:text-text-tertiary
            transition-all duration-fast ease-out
            hover:border-border-hover
            focus:border-accent focus:ring-1 focus:ring-accent/30 focus:outline-none
            ${icon ? 'pl-10' : 'pl-3.5'} pr-3.5
            ${error ? 'border-danger focus:border-danger focus:ring-danger/30' : ''}
            ${className}
          `}
          {...props}
        />
        {error && (
          <p className="mt-1.5 text-xs text-danger">{error}</p>
        )}
      </div>
    );
  }
);

Input.displayName = 'Input';
