import { test } from 'node:test';
import assert from 'node:assert/strict';
import { formatDuration } from '../src/audioManager.ts';

test('formatDuration formats valid seconds correctly', () => {
  assert.equal(formatDuration(0), '0:00');
  assert.equal(formatDuration(9), '0:09');
  assert.equal(formatDuration(15), '0:15');
  assert.equal(formatDuration(59), '0:59');
  assert.equal(formatDuration(60), '1:00');
  assert.equal(formatDuration(65), '1:05');
  assert.equal(formatDuration(125), '2:05');
  assert.equal(formatDuration(3600), '60:00');
});

test('formatDuration handles edge cases and invalid inputs safely', () => {
  assert.equal(formatDuration(NaN), '0:00');
  assert.equal(formatDuration(-10), '0:00');
  assert.equal(formatDuration(Infinity), '0:00');
});
