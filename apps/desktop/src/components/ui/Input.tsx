import { forwardRef, type InputHTMLAttributes } from 'react';

interface InputProps extends InputHTMLAttributes<HTMLInputElement> {
  icon?: React.ReactNode;
  error?: string;
}

export const Input = forwardRef<HTMLInputElement, InputProps>(
  ({ icon, error, className = '', ...props }, ref) => {
    return (
      <div className="w-full">
        <div className="relative">
          {icon && (
            <div className="pointer-events-none absolute left-3 top-1/2 flex h-5 w-5 -translate-y-1/2 items-center justify-center leading-none text-text-tertiary [&>svg]:block [&>svg]:shrink-0">
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
        </div>
        {error && (
          <p className="mt-1.5 text-xs text-danger">{error}</p>
        )}
      </div>
    );
  }
);

Input.displayName = 'Input';
