import {
  reduceCompanionBehavior,
  resolveAnimationFrame,
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
