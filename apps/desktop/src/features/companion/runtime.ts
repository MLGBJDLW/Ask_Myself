export type RuntimeCompanionState =
  | 'idle'
  | 'thinking'
  | 'searching'
  | 'browsing'
  | 'readingFiles'
  | 'runningTool'
  | 'coding'
  | 'waitingForApproval'
  | 'waitingForUser'
  | 'reviewing'
  | 'succeeded'
  | 'failed'
  | 'cancelled'
  | 'sleeping';

export interface RuntimeCompanionAnimation {
  frames: number[];
  fps: number;
  looping: boolean;
  fallback: string | null;
}

export interface CompanionAnimationPack {
  contentHash: string;
  animations: Record<string, RuntimeCompanionAnimation>;
}

export type CompanionBehaviorState =
  | 'idle'
  | 'hovering'
  | 'beingPetted'
  | 'clicked'
  | 'dragging'
  | 'dropped'
  | 'walkingLeft'
  | 'walkingRight'
  | 'waving'
  | 'sleeping'
  | 'reactingToTool'
  | 'reactingToSuccess'
  | 'reactingToFailure';

export type CompanionBehaviorEvent =
  | { type: 'hoverStarted' }
  | { type: 'hoverEnded' }
  | { type: 'clicked'; clickCount: number }
  | { type: 'dragStarted' }
  | { type: 'dragEnded' }
  | { type: 'idleGesture'; gesture: 'wave' | 'sleep' }
  | { type: 'walkStarted'; direction: 'left' | 'right' }
  | { type: 'walkTurned'; direction: 'left' | 'right' }
  | { type: 'taskStateChanged'; state: RuntimeCompanionState }
  | { type: 'animationCompleted' };

const TOOL_STATES = new Set<RuntimeCompanionState>([
  'searching',
  'browsing',
  'readingFiles',
  'runningTool',
  'coding',
]);

export function taskBehavior(state: RuntimeCompanionState): CompanionBehaviorState {
  if (state === 'succeeded') return 'reactingToSuccess';
  if (state === 'failed' || state === 'cancelled') return 'reactingToFailure';
  if (TOOL_STATES.has(state)) return 'reactingToTool';
  if (state === 'sleeping') return 'sleeping';
  return 'idle';
}

export function reduceCompanionBehavior(
  current: CompanionBehaviorState,
  event: CompanionBehaviorEvent,
): CompanionBehaviorState {
  switch (event.type) {
    case 'hoverStarted':
      return current === 'idle' ? 'hovering' : current;
    case 'hoverEnded':
      return current === 'hovering' ? 'idle' : current;
    case 'clicked':
      return event.clickCount >= 3 ? 'beingPetted' : 'clicked';
    case 'dragStarted':
      return 'dragging';
    case 'dragEnded':
      return current === 'dragging' ? 'dropped' : current;
    case 'idleGesture':
      return event.gesture === 'sleep' ? 'sleeping' : 'waving';
    case 'walkStarted':
    case 'walkTurned':
      return event.direction === 'left' ? 'walkingLeft' : 'walkingRight';
    case 'taskStateChanged':
      return current === 'dragging' || current === 'beingPetted'
        ? current
        : taskBehavior(event.state);
    case 'animationCompleted':
      return current === 'dragging' ? current : 'idle';
  }
}

const BEHAVIOR_ANIMATION_CANDIDATES: Record<CompanionBehaviorState, string[]> = {
  idle: [],
  hovering: ['hovering', 'lookUp', 'idle'],
  beingPetted: ['beingPetted', 'petting', 'happy', 'waving', 'idle'],
  clicked: ['clicked', 'jumping', 'waving', 'idle'],
  dragging: ['dragging', 'pickedUp', 'idle'],
  dropped: ['dropped', 'landing', 'jumping', 'idle'],
  walkingLeft: ['moveLeft', 'walkingLeft', 'walking', 'idle'],
  walkingRight: ['moveRight', 'walkingRight', 'walking', 'idle'],
  waving: ['waving', 'wave', 'idle'],
  sleeping: ['sleeping', 'sleep', 'idle'],
  reactingToTool: [],
  reactingToSuccess: ['succeeded', 'waving', 'jumping', 'idle'],
  reactingToFailure: ['failed', 'cancelled', 'idle'],
};

export const STATE_ANIMATION_CANDIDATES: Record<RuntimeCompanionState, string[]> = {
  idle: ['idle'],
  thinking: ['thinking', 'running', 'review', 'idle'],
  searching: ['searching', 'running', 'moveRight', 'idle'],
  browsing: ['browsing', 'running', 'moveRight', 'idle'],
  readingFiles: ['readingFiles', 'review', 'running', 'idle'],
  runningTool: ['runningTool', 'running', 'idle'],
  coding: ['coding', 'running', 'review', 'idle'],
  waitingForApproval: ['waitingForApproval', 'waiting', 'idle'],
  waitingForUser: ['waitingForUser', 'waiting', 'waving', 'idle'],
  reviewing: ['reviewing', 'review', 'idle'],
  succeeded: ['succeeded', 'waving', 'jumping', 'idle'],
  failed: ['failed', 'idle'],
  cancelled: ['cancelled', 'failed', 'idle'],
  sleeping: ['sleeping', 'idle'],
};

export interface SelectedCompanionAnimation {
  key: string;
  animation: RuntimeCompanionAnimation;
}

export function selectCompanionAnimation(
  pack: CompanionAnimationPack | null,
  state: RuntimeCompanionState,
  behavior: CompanionBehaviorState,
): SelectedCompanionAnimation | null {
  if (!pack) return null;
  const candidates = [
    ...BEHAVIOR_ANIMATION_CANDIDATES[behavior],
    ...STATE_ANIMATION_CANDIDATES[state],
  ];
  for (const key of candidates) {
    const animation = pack.animations[key];
    if (animation) return { key, animation };
  }
  const [key, animation] = Object.entries(pack.animations)[0] ?? [];
  return key && animation ? { key, animation } : null;
}

export function resolveAnimationFrame(
  elapsedMs: number,
  fps: number,
  frameCount: number,
  looping: boolean,
): { index: number; completed: boolean } {
  if (frameCount <= 1 || fps <= 0) return { index: 0, completed: !looping };
  const absoluteFrame = Math.max(0, Math.floor((elapsedMs * fps) / 1_000));
  if (looping) return { index: absoluteFrame % frameCount, completed: false };
  return {
    index: Math.min(frameCount - 1, absoluteFrame),
    completed: absoluteFrame >= frameCount,
  };
}

export interface WalkBounds {
  minX: number;
  maxX: number;
}

export function resolveWalkStep(
  x: number,
  direction: 'left' | 'right',
  elapsedMs: number,
  bounds: WalkBounds,
  speedPixelsPerSecond = 28,
): { x: number; direction: 'left' | 'right'; turned: boolean } {
  const delta = Math.max(0, elapsedMs) * speedPixelsPerSecond / 1_000;
  const candidate = direction === 'left' ? x - delta : x + delta;
  if (candidate <= bounds.minX) {
    return { x: bounds.minX, direction: 'right', turned: direction !== 'right' };
  }
  if (candidate >= bounds.maxX) {
    return { x: bounds.maxX, direction: 'left', turned: direction !== 'left' };
  }
  return { x: candidate, direction, turned: false };
}

export async function decodeImageSource(
  source: string,
  createImage: () => HTMLImageElement = () => new Image(),
): Promise<void> {
  const image = createImage();
  image.src = source;
  if (typeof image.decode === 'function') {
    await image.decode();
    return;
  }
  if (image.complete && image.naturalWidth > 0) return;
  await new Promise<void>((resolve, reject) => {
    image.onload = () => resolve();
    image.onerror = () => reject(new Error('Companion image failed to decode'));
  });
}
