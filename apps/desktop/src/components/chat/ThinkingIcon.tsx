import './ThinkingIcon.css';

// Lucide's ISC-licensed brain outline, also used by the app's static icons.
const paths = [
  'M12 18V5',
  'M15 13a4.17 4.17 0 0 1-3-4 4.17 4.17 0 0 1-3 4',
  'M17.598 6.5A3 3 0 1 0 12 5a3 3 0 1 0-5.598 1.5',
  'M17.997 5.125a4 4 0 0 1 2.526 5.77',
  'M18 18a4 4 0 0 0 2-7.464',
  'M19.967 17.483A4 4 0 1 1 12 18a4 4 0 1 1-7.967-.517',
  'M6 18a4 4 0 0 1-2-7.464',
  'M6.003 5.125a4 4 0 0 0-2.526 5.77',
];

export function ThinkingIcon({ active = false, size = 15 }: { active?: boolean; size?: number }) {
  return (
    <svg
      data-testid="thinking-brain" data-active={active ? 'true' : 'false'}
      className={`thinking-brain shrink-0 ${active ? 'text-accent' : ''}`}
      width={size} height={size} viewBox="0 0 24 24"
      fill="none" stroke="currentColor" strokeWidth="1.8"
      strokeLinecap="round" strokeLinejoin="round" aria-hidden="true" focusable="false"
    >
      <g className="thinking-brain-outline">
        {paths.map(d => <path key={d} d={d} />)}
      </g>
      {active && <g className="thinking-brain-ink">
        {paths.map((d, index) => <path key={d} d={d} pathLength={1} style={{ animationDelay: `${index * 85}ms` }} />)}
      </g>}
    </svg>
  );
}
