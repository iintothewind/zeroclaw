import * as ProgressPrimitive from '@radix-ui/react-progress';
import type { ComponentPropsWithoutRef } from 'react';

export interface ProgressProps
  extends ComponentPropsWithoutRef<typeof ProgressPrimitive.Root> {
  /** Extra classes for the filled indicator (e.g. a status color override). */
  indicatorClassName?: string;
}

/**
 * Determinate progress bar built on the Radix Progress primitive. The Radix
 * root supplies `role="progressbar"` plus `aria-valuenow/min/max`, so callers
 * get correct accessibility for free. Track and indicator are token-classed to
 * the Operator Console palette (`bg-pc-border` / `bg-pc-accent`).
 */
export function Progress({
  className = '',
  value = 0,
  indicatorClassName = '',
  ...props
}: ProgressProps) {
  const pct = Math.min(Math.max(value ?? 0, 0), 100);
  return (
    <ProgressPrimitive.Root
      className={`relative h-2 w-full overflow-hidden rounded-full bg-pc-border ${className}`}
      value={value}
      {...props}
    >
      <ProgressPrimitive.Indicator
        className={`h-full w-full rounded-full bg-pc-accent transition-transform duration-300 ease-out ${indicatorClassName}`}
        style={{ transform: `translateX(-${100 - pct}%)` }}
      />
    </ProgressPrimitive.Root>
  );
}

export default Progress;
