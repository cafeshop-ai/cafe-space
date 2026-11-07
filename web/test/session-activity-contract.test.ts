import { describe, expect, it } from 'vitest';
import proposal from '../../fixtures/session-activity/proposal.json';
import provenance from '../../fixtures/session-activity/source-provenance.json';
import clocks from '../../fixtures/session-activity/clock-examples.json';
import wire from '../../fixtures/session-activity/wire-v6.json';
import { sessionActivityEvent } from '../src/protocol-fixtures.generated';
import { applySessionEvent, type SessionDetail } from '../src/state/sessionDetail';

describe('session activity fixtures and client reduction, not backend acceptance', () => {
  it('pins wire and schema versions without claiming native acceptance', () => {
    expect(proposal.status).toBe('implementation_pending_native_acceptance');
    expect(proposal.versions).toEqual({
      currentFullProtocol: 7, proposedFullProtocol: 7,
      currentSchema: 6, proposedSchema: 6, frozenControl: 2, ownerSettings: 1,
    });
    expect(proposal.fields.lastActivityAt.unit).toBe('unix_milliseconds');
    expect(proposal.fields.lastActivityAt.maximum).toBe(8_640_000_000_000_000);
    expect(Number.isSafeInteger(proposal.fields.lastActivityAt.maximum)).toBe(true);
    expect(new Date(proposal.fields.lastActivityAt.maximum).getTime()).toBe(proposal.fields.lastActivityAt.maximum);
    expect(proposal.fields.preview.maxUtf8Bytes).toBe(256);
    expect(proposal.fields.preview.visibility).toEqual(['local', 'remote']);
  });

  it('keeps private provenance separate from public wire fields', () => {
    expect(proposal.storage.status).toBe('schema_6_pending_native_acceptance');
    expect(proposal.storage.fixture).toBe('fixtures/migrations/v6.sql');
    expect(proposal.storage.columns).toEqual(['session_id', 'last_activity_at', 'preview', 'last_user_message_id', 'last_user_message_order']);
    expect(proposal.storage.legacy).toContain('no backfill');
    expect(proposal.storage.sourceOrder).toContain('not lexical');
    expect(proposal.storage.lifetime).toContain('survives transcript-cache eviction');
    expect(proposal.versions.currentSchema).toBe(6);
    expect(proposal.storage.receipts).toContain('cache eviction does not erase duplicate detection');
  });

  it('types shared wire metadata and clears the open transcript preview by replacement', () => {
    expect(wire.fullProtocolVersion).toBe(7);
    expect(wire.controlVersion).toBe(2);
    expect(sessionActivityEvent).toEqual(wire.event);
    const session = sessionActivityEvent.event.data.session;
    const before: SessionDetail = {
      eventStreamId: sessionActivityEvent.eventStreamId, sequence: 0,
      view: { session, messages: [], approvals: [], historyTruncated: false },
    };
    const { preview: _preview, ...cleared } = session;
    const after = applySessionEvent(before, before.eventStreamId, 1, { kind: 'session_changed', data: { session: cleared } });
    expect(typeof after).toBe('object');
    if (typeof after !== 'object') throw Error('metadata event must keep the open session');
    expect(after.view.session).not.toHaveProperty('preview');
    expect(after.view.session.lastActivityAt).toBe(session.lastActivityAt);
    expect(after.view.session.state).toEqual(session.state);
    expect(after.view.messages).toEqual(before.view.messages);
  });

  it('checks clock goldens without claiming Rust execution or persisted activity', () => {
    expect(clocks.status).toBe('shared_rules_not_current_wire_or_persistence');
    expect(clocks.maximum).toBe(proposal.fields.lastActivityAt.maximum);
    const valid = (value: number | null): value is number => value !== null
      && Number.isSafeInteger(value) && value > 0 && value <= clocks.maximum;
    for (const value of clocks.cases) {
      if (value.invalidPrevious) {
        expect(valid(value.previous)).toBe(false);
        expect(value.expected).toBeUndefined();
        continue;
      }
      expect(value.previous === null || valid(value.previous)).toBe(true);
      const candidates = [value.previous, value.observed].filter(valid);
      expect(value.expected).toBe(candidates.length ? Math.max(...candidates) : null);
      if (valid(value.previous)) expect(value.expected).toBeGreaterThanOrEqual(value.previous);
    }
  });

  it('keeps metadata examples optional, bounded and safe for JavaScript numbers', () => {
    expect(proposal.metadataExamples.find(item => item.name === 'legacy_unknown')?.value).toEqual({});
    for (const { value } of proposal.metadataExamples) {
      if ('lastActivityAt' in value) {
        expect(Number.isSafeInteger(value.lastActivityAt)).toBe(true);
        expect(value.lastActivityAt).toBeGreaterThanOrEqual(proposal.fields.lastActivityAt.minimum);
        expect(value.lastActivityAt).toBeLessThanOrEqual(proposal.fields.lastActivityAt.maximum);
      }
      if ('preview' in value) {
        expect(value.preview).toBeTruthy();
        expect(new TextEncoder().encode(value.preview).length).toBeLessThanOrEqual(proposal.fields.preview.maxUtf8Bytes);
      }
    }
  });

  it('keeps preview goldens single-line and distinguishes omission from empty text', () => {
    for (const { expected } of proposal.previewExamples) {
      if (expected === null) continue;
      expect(expected).not.toBe('');
      expect(expected).not.toMatch(/^ | $| {2}/);
      expect(expected.replaceAll(' ', '')).not.toMatch(/[\p{White_Space}\p{Cc}\u202a-\u202e\u2066-\u2069]/u);
      expect(new TextEncoder().encode(expected).length).toBeLessThanOrEqual(proposal.fields.preview.maxUtf8Bytes);
    }
    expect(proposal.previewExamples.find(item => item.name === 'empty_latest_message')?.expected).toBeNull();
    expect(proposal.previewExamples.find(item => item.name === 'emoji_preserved')?.expected).toBe('Synthetic ☕ 👩‍💻');
  });

  it('checks byte-boundary arithmetic without claiming a production normalizer', () => {
    const limit = proposal.fields.preview.maxUtf8Bytes;
    for (const value of proposal.boundaryExamples) {
      expect(Array.from(value.scalar)).toHaveLength(1);
      const scalarBytes = new TextEncoder().encode(value.scalar).length;
      expect(value.expectedCount).toBe(Math.min(value.count, Math.floor(limit / scalarBytes)));
      expect(new TextEncoder().encode(value.scalar.repeat(value.expectedCount)).length).toBe(value.expectedUtf8Bytes);
      expect(value.expectedUtf8Bytes).toBeLessThanOrEqual(limit);
    }
  });

  it('pins indistinguishable display examples without deleting literal user text', () => {
    expect(provenance.status).toBe('current_display_collision_future_preview_requirement');
    expect(proposal.updates.association).toContain('same observation');
    for (const value of provenance.cases) {
      const block = value.nativeOpaque.content[0];
      const bytes = new TextEncoder().encode(JSON.stringify(block)).length;
      const label = block.type === 'image' ? 'Image' : 'Unsupported content';
      expect(value.sameDisplayText).toBe(`[${label} not displayed: ${bytes} encoded JSON bytes]`);
      expect(value.nativeLiteral.content).toBe(value.sameDisplayText);
      expect(value.opaquePreview).toBeNull();
      expect(value.literalPreview).toBe(value.nativeLiteral.content);
    }
  });

  it('checks deterministic ordering goldens without changing daemon or UI sorting', () => {
    const sorted = [...proposal.sorting.input].sort((a, b) => {
      const activity = (b.lastActivityAt ?? 0) - (a.lastActivityAt ?? 0);
      return activity || (a.id < b.id ? -1 : a.id > b.id ? 1 : 0);
    });
    expect(sorted.map(item => item.id)).toEqual(proposal.sorting.expectedIds);
    expect(proposal.updates.noActivity).toContain('dispatch without observation');
    expect(proposal.updates.noActivity).toContain('identical create retry');
    expect(proposal.updates.noActivity).toContain('daemon recovery');
    expect(proposal.fields.preview.excludedRoles).toEqual(['assistant', 'tool', 'system']);
    expect(proposal.fields.preview.excludedParts).toEqual(['thinking', 'tool_call']);
  });
});
