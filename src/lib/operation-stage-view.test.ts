import { describe, expect, it } from 'vitest';
import { operationStageLabel, stageDurationLabel } from './operation-stage-view';

describe('operation stage presentation', () => {
  it('translates known native stages without hiding unknown diagnostics', () => {
    expect(operationStageLabel('desktop_graceful_wait')).toBe('Espera pelo fechamento do Codex');
    expect(operationStageLabel('future_stage')).toBe('future stage');
  });

  it('keeps subsecond timings visible', () => {
    expect(stageDurationLabel(18)).toBe('18 ms');
    expect(stageDurationLabel(2_083)).toBe('2.08 s');
    expect(stageDurationLabel(65_000)).toBe('1 min 5 s');
  });
});
