import { TextPresentation, type StreamingMode } from '../src/lib/streaming/textPresentation';

function assert(value: unknown, message: string): asserts value { if (!value) throw new Error(message); }
function fixture(mode: StreamingMode, reduceMotion = false) {
  let time = 0, nextId = 0;
  const timers = new Map<number, { at: number; callback: () => void }>();
  const paints: string[] = [];
  const projection = new TextPresentation('', mode, reduceMotion, text => paints.push(text), {
    now: () => time,
    schedule: (callback, delay) => { const id = ++nextId; timers.set(id, { at: time + delay, callback }); return id; },
    cancel: handle => { timers.delete(handle as number); },
  });
  return { projection, paints, timers, advance(ms: number) {
    const end = time + ms;
    for (;;) {
      const next = [...timers].filter(([, timer]) => timer.at <= end).sort((a, b) => a[1].at - b[1].at)[0];
      if (!next) break;
      time = next[1].at; timers.delete(next[0]); next[1].callback();
    }
    time = end;
  } };
}

for (const [mode, firstPaint] of [['chunked', 180], ['balanced', 50], ['smooth', 32]] as const) {
  const f = fixture(mode);
  f.projection.update('A'.repeat(200), true);
  f.advance(firstPaint - 1);
  assert(f.paints.length === 0, `${mode} should batch until its deadline`);
  f.advance(1);
  assert(f.paints.length > 0, `${mode} should paint at its deadline`);
  assert(mode === 'smooth' ? f.paints[0].length < 200 : f.paints[0].length === 200, `${mode} reveal behavior`);
  f.projection.dispose();
  assert(f.timers.size === 0, `${mode} disposes scheduled work`);
}
{
  const f = fixture('smooth');
  let canonical = '';
  for (let n = 0; n < 24; n++) { canonical += '你好🙂'; f.projection.update(canonical, true); f.advance(10); }
  f.advance(240);
  assert(f.paints[f.paints.length - 1] === canonical, 'continuous arrivals must catch up within the fixed reveal window');
  assert(f.paints.length >= 3 && f.paints.length <= 15, 'smooth rendering is bounded');
  for (const text of f.paints) assert(!/[\ud800-\udbff]$/.test(text), 'do not split emoji surrogate pairs');
  f.projection.update(`${canonical} pending`, true);
  f.projection.update('Final **exact** answer', false);
  assert(f.paints[f.paints.length - 1] === 'Final **exact** answer' && f.timers.size === 0, 'terminal text bypasses delay');
  f.advance(1000);
  assert(f.paints[f.paints.length - 1] === 'Final **exact** answer', 'cancelled callbacks cannot repaint old text');
  f.projection.update('replacement', true);
  assert(f.paints[f.paints.length - 1] === 'replacement', 'non-prefix corrections are immediate');
}
{
  const f = fixture('smooth', true);
  f.projection.update('Reduced motion content', true);
  f.advance(32);
  assert(f.paints[0] === 'Reduced motion content' && f.timers.size === 0, 'reduced motion disables gradual reveal');
}
console.log('ok - text presentation modes, deadlines, unicode, terminal/reset and reduced motion');
