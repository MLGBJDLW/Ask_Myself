import { createUndoActionGate } from '../src/lib/undoActionGate';

function assertEqual(actual: number, expected: number, message: string): void {
  if (actual !== expected) throw new Error(`${message}: expected ${expected}, got ${actual}`);
}

async function main() {
  let confirms = 0;
  let undos = 0;
  const confirmed = createUndoActionGate(
    () => { confirms += 1; },
    () => { undos += 1; },
  );

  await Promise.all([confirmed.confirm(), confirmed.confirm(), confirmed.undo()]);
  assertEqual(confirms, 1, 'confirm should run once');
  assertEqual(undos, 0, 'late undo must not run after confirm');

  const undone = createUndoActionGate(
    () => { confirms += 1; },
    () => { undos += 1; },
  );
  await Promise.all([undone.undo(), undone.confirm(), undone.undo()]);
  assertEqual(confirms, 1, 'confirm must not run after undo');
  assertEqual(undos, 1, 'undo should run once');
}

void main().catch(error => {
  setTimeout(() => { throw error; }, 0);
});
