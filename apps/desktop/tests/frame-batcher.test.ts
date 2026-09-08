import { ConversationFrameBatcher } from '../src/lib/streaming/frameBatcher';

function assert(value: unknown, message: string): asserts value {
  if (!value) throw new Error(message);
}

async function main(): Promise<void> {
  const frames: Array<() => void> = [];
  const delivered: string[] = [];
  const batcher = new ConversationFrameBatcher(
    id => delivered.push(id), callback => frames.push(callback),
  );
  batcher.schedule('background-conversation');
  batcher.schedule('background-conversation');
  // Native WebViews may stop animation frames when occluded. State delivery
  // still has to continue so recovery, task controls and sidebar state work.
  await new Promise(resolve => setTimeout(resolve, 150));
  assert(delivered.join(',') === 'background-conversation', 'a paused animation frame must not freeze stream notifications');
  batcher.schedule('next-conversation');
  frames[0]();
  assert(delivered.length === 1, 'a late frame from the previous flush cannot consume newer work');
  frames[1]();
  assert(delivered.join(',') === 'background-conversation,next-conversation', 'the new frame delivers exactly once');
  console.log('ok - stream notifications survive paused and late animation frames');
}

void main();
