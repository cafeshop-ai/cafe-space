import { describe, expect, it } from 'vitest';
import { projectRequests, projectEvents, projectResponse, projectLocalOnly, projectStaleFence, getProjectRootRequest, projectRootResponse } from '../src/protocol-fixtures.generated';

describe('project administration wire fixtures', () => {
  it('keeps shared lists/events path-free, with one local-only root read', () => {
    expect(projectRequests.map(frame => frame.request.method)).toEqual(['add_project', 'add_project', 'update_project', 'remove_project', 'update_project']);
    expect(projectRequests[0].request.params.root).toBe('/synthetic/project-root');
    expect(projectRequests[1].request.params.root).toBe('C:\\synthetic\\project-root');
    expect(projectRequests[2].request.params.expectedCwdFence).toBe('cwd-fixture');
    expect(projectRequests[3].request.params.expectedCwdFence).toBe('cwd-next');
    expect(projectRequests[4].request.params).toEqual({projectId: 'project-fixture', expectedCwdFence: 'cwd-next', name: 'Renamed 茶'});
    expect(projectResponse.result.data.result.data).toEqual({ id: 'project-fixture', name: 'Fixture 茶', cwdFence: 'cwd-fixture' });
    for (const frame of [...projectEvents, projectResponse, projectLocalOnly, projectStaleFence]) {
      expect(JSON.stringify(frame)).not.toContain('root');
      expect(JSON.parse(JSON.stringify(frame))).toEqual(frame);
    }
    expect(projectLocalOnly.result.data.error.code).toBe('LOCAL_ONLY');
    expect(projectStaleFence.result.data.error.code).toBe('STALE_CWD_FENCE');
    expect(getProjectRootRequest.request.method).toBe('get_project_root');
    expect(projectRootResponse.result.data.result.kind).toBe('project_root');
    expect(projectRootResponse.result.data.result.data.root).toBe('/synthetic/project-root');
    expect(projectEvents.map(frame => frame.sequence)).toEqual([1, 2, 3]);
    expect(projectEvents[1].event.data.project.cwdFence).toBe('cwd-next');
    expect(projectEvents[2].event.data.projectId).toBe('project-fixture');
  });
});
