import {
  reduceCompanionBehavior,
  resolveAnimationFrame,
  resolveLookDirection,
  resolveWalkStep,
  selectCompanionAnimation,
  taskBehavior,
  type CompanionAnimationPack,
} from '../src/features/companion/runtime';

function assert(condition: unknown, message: string): asserts condition {
  if (!condition) throw new Error(message);
}

function assertEqual<T>(actual: T, expected: T, message: string): void {
  if (actual !== expected) throw new Error(`${message}: expected ${String(expected)}, got ${String(actual)}`);
}

assertEqual(reduceCompanionBehavior('idle', { type: 'hoverStarted' }), 'hovering', 'hover enters');
assertEqual(reduceCompanionBehavior('hovering', { type: 'hoverEnded' }), 'idle', 'hover exits');
assertEqual(reduceCompanionBehavior('idle', { type: 'clicked', clickCount: 1 }), 'clicked', 'single click');
assertEqual(reduceCompanionBehavior('clicked', { type: 'clicked', clickCount: 3 }), 'beingPetted', 'repeated click pets');
assertEqual(reduceCompanionBehavior('idle', { type: 'dragStarted' }), 'dragging', 'drag starts');
assertEqual(reduceCompanionBehavior('dragging', { type: 'dragEnded' }), 'dropped', 'drag drops');
assertEqual(taskBehavior('runningTool'), 'reactingToTool', 'tool task reaction');
assertEqual(taskBehavior('succeeded'), 'reactingToSuccess', 'success task reaction');
assertEqual(taskBehavior('failed'), 'reactingToFailure', 'failure task reaction');

assertEqual(resolveAnimationFrame(0, 10, 4, true).index, 0, 'animation starts at first frame');
assertEqual(resolveAnimationFrame(350, 10, 4, true).index, 3, 'elapsed clock selects frame');
assertEqual(resolveAnimationFrame(450, 10, 4, true).index, 0, 'loop wraps without interval drift');
const terminalFrame = resolveAnimationFrame(450, 10, 4, false);
assertEqual(terminalFrame.index, 3, 'single-pass animation holds final frame');
assert(terminalFrame.completed, 'single-pass animation completes');

const leftBoundary = resolveWalkStep(3, 'left', 1_000, { minX: 0, maxX: 100 }, 10);
assertEqual(leftBoundary.x, 0, 'walk clamps to left boundary');
assertEqual(leftBoundary.direction, 'right', 'walk turns at left boundary');
assert(leftBoundary.turned, 'walk reports a boundary turn');
const rightBoundary = resolveWalkStep(97, 'right', 1_000, { minX: 0, maxX: 100 }, 10);
assertEqual(rightBoundary.x, 100, 'walk clamps to right boundary');
assertEqual(rightBoundary.direction, 'left', 'walk turns at right boundary');

assertEqual(resolveLookDirection({ x: 50, y: 0 }, { x: 50, y: 50 }), 0, 'look up');
assertEqual(resolveLookDirection({ x: 100, y: 50 }, { x: 50, y: 50 }), 4, 'look right');
assertEqual(resolveLookDirection({ x: 50, y: 100 }, { x: 50, y: 50 }), 8, 'look down');
assertEqual(resolveLookDirection({ x: 0, y: 50 }, { x: 50, y: 50 }), 12, 'look left');
assertEqual(resolveLookDirection({ x: 55, y: 55 }, { x: 50, y: 50 }), null, 'look dead zone');
const pointAtDegrees = (degrees: number) => {
  const radians = degrees * Math.PI / 180;
  return { x: 50 + Math.sin(radians) * 100, y: 50 - Math.cos(radians) * 100 };
};
assertEqual(resolveLookDirection(pointAtDegrees(12), { x: 50, y: 50 }, 0, null), 1, 'raw look sector');
assertEqual(resolveLookDirection(pointAtDegrees(12), { x: 50, y: 50 }, 0, 0, 4), 0, 'look hysteresis holds');
assertEqual(resolveLookDirection(pointAtDegrees(18), { x: 50, y: 50 }, 0, 0, 4), 1, 'look hysteresis releases');
assertEqual(
  resolveLookDirection(pointAtDegrees(1), { x: 50, y: 50 }, 0, 15, 16),
  15,
  'look hysteresis wraps symmetrically across the 15-to-0 seam',
);

const pack: CompanionAnimationPack = {
  contentHash: 'hash',
  animations: {
    idle: { frames: [0], fps: 8, looping: true, fallback: null },
    petting: { frames: [0, 1], fps: 12, looping: false, fallback: 'idle' },
  },
};
assertEqual(
  selectCompanionAnimation(pack, 'idle', 'beingPetted')?.key,
  'petting',
  'behavior animation wins over task fallback',
);

console.log('ok - companion runtime state, clock, and boundary contracts');
