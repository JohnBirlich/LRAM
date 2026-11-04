import assert from 'node:assert/strict';
import { test } from 'node:test';
import { estimateCapacity } from '../src/features/monitor/capacity.ts';

const frame = (cpu = 2, resident = 400, commit = 600) => ({
  clients: Array.from({ length: 3 }, () => ({ cpu, resident, commit, gpu: 0.5, vram: 300 })),
  systemCpu: 15, availableMb: 24 * 1024, totalMb: 32 * 1024,
});
const frames = () => Array.from({ length: 31 }, () => frame());

test('waits for enough complete frames', () => {
  assert.equal(estimateCapacity(frames().slice(0, 29)), null);
  assert.equal(estimateCapacity(Array.from({ length: 30 }, () => ({ ...frame(), clients: [] }))), null);
});
test('reports observed load separately from capacity and reserves system headroom', () => {
  const r = estimateCapacity(frames());
  assert.deepEqual(r.cpu, { low: 2, high: 2 });
  assert.deepEqual(r.ram, { low: 400, high: 400 });
  assert.deepEqual(r.vram, { low: 300, high: 300 });
  assert.equal(r.count, 3);
  assert.ok(r.low >= 3 && r.high >= r.low && r.high < 40);
});
test('idle samples never produce infinity', () => {
  const r = estimateCapacity(Array.from({ length: 30 }, () => frame(0)));
  assert.ok(Number.isFinite(r.high));
  assert.deepEqual(r.cpu, { low: 0, high: 0 });
});
test('missing GPU stays unknown, never becomes zero', () => {
  const fs = frames(); fs[2].clients[0].gpu = null; fs[2].clients[0].vram = null;
  const r = estimateCapacity(fs);
  assert.equal(r.gpu, null); assert.equal(r.vram, null);
});
test('missing, corrupt or changing cohort invalidates estimate', () => {
  const fs = frames(); fs[0].clients.pop(); assert.equal(estimateCapacity(fs), null);
  for (const bad of [NaN, Infinity, -1]) {
    const data = frames(); data[0].clients[0].cpu = bad;
    assert.equal(estimateCapacity(data), null);
  }
});
test('no free headroom predicts no additional clients', () => {
  const r = estimateCapacity(frames().map((f) => ({ ...f, availableMb: 512, systemCpu: 95 })));
  assert.equal(r.low, 3); assert.equal(r.high, 3);
});
test('the heaviest client constrains the projection', () => {
  const base = estimateCapacity(frames());
  const expensive = frames(); expensive.forEach((f) => { f.clients[2].cpu = 15; });
  assert.ok(estimateCapacity(expensive).high < base.high);
});
test('private commit and non-Roblox system load both reduce headroom', () => {
  const base = estimateCapacity(frames());
  assert.ok(estimateCapacity(frames().map((f) => ({ ...f, systemCpu: 70 }))).high < base.high);
  assert.ok(estimateCapacity(Array.from({ length: 30 }, () => frame(2, 400, 4000))).high < base.high);
});
