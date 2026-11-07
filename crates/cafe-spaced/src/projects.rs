//! Approved local cwd records; callers serialize mutations and admission under the Host mutex.
use crate::sessions::{self, Error, Result};
use cafe_space_ipc::project_root::{self, Directory};
use cafe_space_protocol::*;
use rusqlite::{Connection, OptionalExtension};
use std::path::Path;
#[cfg(test)]
use std::path::PathBuf;

struct Record {
    summary: ProjectSummary,
    root: String,
    identity: String,
}

fn record(database: &Connection, id: &str) -> Result<Option<Record>> {
    let value = database
        .query_row(
            "SELECT id,name,cwd_fence,root,identity FROM projects WHERE id=?1",
            [id],
            |row| {
                Ok(Record {
                    summary: ProjectSummary {
                        id: row.get(0)?,
                        name: row.get(1)?,
                        cwd_fence: row.get(2)?,
                    },
                    root: row.get(3)?,
                    identity: row.get(4)?,
                })
            },
        )
        .optional()?;
    if let Some(value) = &value {
        validate(&value.summary)?;
    }
    Ok(value)
}

fn validate(summary: &ProjectSummary) -> Result<()> {
    let frame = ServerMessage::Response(ServerResponse {
        message_type: ResponseMessageType::Response,
        protocol_version: PROTOCOL_VERSION,
        request_id: "validation".into(),
        result: RequestResult::Applied(Box::new(AppliedResult {
            result: Response::Project(summary.clone()),
        })),
    });
    encode_server_message(&frame).map_err(|_| Error::Rejected("INVALID_PROJECT"))?;
    Ok(())
}

fn root_error(error: std::io::Error) -> Error {
    Error::Rejected(match error.to_string().as_str() {
        "PROJECT_ROOT_NOT_UTF8" => "PROJECT_ROOT_NOT_UTF8",
        "PROJECT_ROOT_UNSUPPORTED" => "PROJECT_ROOT_UNSUPPORTED",
        "PROJECT_ROOT_UNAVAILABLE" => "PROJECT_ROOT_UNAVAILABLE",
        _ => "PROJECT_ROOT_IDENTITY_UNAVAILABLE",
    })
}

fn canonical(root: &str, data: &Path) -> Result<Directory> {
    let root = project_root::inspect(Path::new(root)).map_err(root_error)?;
    if project_root::is_within(&root, data).map_err(root_error)? {
        return Err(Error::Rejected("PROJECT_ROOT_UNSUPPORTED"));
    }
    Ok(root)
}

pub(crate) fn list(database: &Connection) -> Result<Vec<ProjectSummary>> {
    let mut statement =
        database.prepare("SELECT id,name,cwd_fence FROM projects ORDER BY rowid LIMIT 1001")?;
    let rows = statement.query_map([], |row| {
        Ok(ProjectSummary {
            id: row.get(0)?,
            name: row.get(1)?,
            cwd_fence: row.get(2)?,
        })
    })?;
    let mut items = Vec::new();
    for row in rows {
        if items.len() == 1000 {
            return Err(Error::Rejected("PROJECT_LIMIT"));
        }
        let item = row?;
        validate(&item)?;
        items.push(item);
    }
    Ok(items)
}

pub(crate) fn token() -> Result<String> {
    let mut random = [0u8; 16];
    getrandom::fill(&mut random).map_err(|_| Error::Rejected("PROJECT_ID_FAILED"))?;
    Ok(random.iter().map(|b| format!("{b:02x}")).collect())
}

pub(crate) fn add(
    database: &Connection,
    data: &Path,
    params: AddProjectParams,
) -> Result<(ProjectSummary, bool)> {
    let root = canonical(&params.root, data)?;
    let existing: Option<String> = database
        .query_row(
            "SELECT id FROM projects WHERE identity=?1",
            [&root.identity],
            |row| row.get(0),
        )
        .optional()?;
    if let Some(id) = existing {
        return Ok((
            record(database, &id)?
                .ok_or(Error::Rejected("PROJECT_NOT_APPROVED"))?
                .summary,
            false,
        ));
    }
    let same_path: bool = database.query_row(
        "SELECT EXISTS(SELECT 1 FROM projects WHERE root=?1)",
        [root
            .path
            .to_str()
            .ok_or(Error::Rejected("PROJECT_ROOT_NOT_UTF8"))?],
        |row| row.get(0),
    )?;
    if same_path {
        return Err(Error::Rejected("PROJECT_ROOT_CHANGED"));
    }
    if list(database)?.len() >= 1000 {
        return Err(Error::Rejected("PROJECT_LIMIT"));
    }
    let name = match params.name {
        Some(name) => name,
        None => root
            .path
            .file_name()
            .and_then(|s| s.to_str())
            .ok_or(Error::Rejected("PROJECT_ROOT_NOT_UTF8"))?
            .into(),
    };
    let summary = ProjectSummary {
        id: token()?,
        name,
        cwd_fence: token()?,
    };
    validate(&summary)?;
    database.execute(
        "INSERT INTO projects (id,name,root,identity,cwd_fence) VALUES (?1,?2,?3,?4,?5)",
        rusqlite::params![
            summary.id,
            summary.name,
            root.path
                .to_str()
                .ok_or(Error::Rejected("PROJECT_ROOT_NOT_UTF8"))?,
            root.identity,
            summary.cwd_fence
        ],
    )?;
    Ok((summary, true))
}

fn current(database: &Connection, id: &str, fence: &str) -> Result<Record> {
    let record = record(database, id)?.ok_or(Error::Rejected("PROJECT_NOT_APPROVED"))?;
    if record.summary.cwd_fence != fence {
        return Err(Error::Rejected("STALE_CWD_FENCE"));
    }
    Ok(record)
}
fn idle(database: &Connection, id: &str) -> Result<()> {
    if sessions::list(database)?.iter().any(|s| {
        s.project_id == id && !matches!(s.state, SessionState::Stopped | SessionState::Failed(_))
    }) {
        return Err(Error::Rejected("PROJECT_IN_USE"));
    }
    Ok(())
}

pub(crate) fn update(
    database: &Connection,
    data: &Path,
    params: UpdateProjectParams,
) -> Result<(ProjectSummary, bool)> {
    let mut record = current(database, &params.project_id, &params.expected_cwd_fence)?;
    if params.root.is_some() {
        idle(database, &params.project_id)?;
    }
    let old = (
        record.summary.clone(),
        record.root.clone(),
        record.identity.clone(),
    );
    if let Some(root) = params.root {
        let root = canonical(&root, data)?;
        let duplicate: Option<String> = database
            .query_row(
                "SELECT id FROM projects WHERE identity=?1 AND id<>?2",
                rusqlite::params![root.identity, params.project_id],
                |r| r.get(0),
            )
            .optional()?;
        if duplicate.is_some() {
            return Err(Error::Rejected("PROJECT_ROOT_ALREADY_APPROVED"));
        }
        if root.identity != record.identity {
            record.summary.cwd_fence = token()?;
        }
        record.identity = root.identity;
        record.root = root
            .path
            .to_str()
            .ok_or(Error::Rejected("PROJECT_ROOT_NOT_UTF8"))?
            .into();
    }
    if let Some(name) = params.name {
        record.summary.name = name;
    }
    validate(&record.summary)?;
    let changed = old
        != (
            record.summary.clone(),
            record.root.clone(),
            record.identity.clone(),
        );
    if changed {
        database.execute(
            "UPDATE projects SET name=?1,root=?2,identity=?3,cwd_fence=?4 WHERE id=?5",
            rusqlite::params![
                record.summary.name,
                record.root,
                record.identity,
                record.summary.cwd_fence,
                params.project_id
            ],
        )?;
    }
    Ok((record.summary, changed))
}

pub(crate) fn remove(database: &Connection, params: RemoveProjectParams) -> Result<()> {
    current(database, &params.project_id, &params.expected_cwd_fence)?;
    idle(database, &params.project_id)?;
    database.execute("DELETE FROM projects WHERE id=?1", [&params.project_id])?;
    Ok(())
}

pub(crate) fn root(database: &Connection, id: &str) -> Result<ProjectRoot> {
    let record = record(database, id)?.ok_or(Error::Rejected("PROJECT_NOT_APPROVED"))?;
    Ok(ProjectRoot {
        project_id: record.summary.id,
        cwd_fence: record.summary.cwd_fence,
        root: record.root,
    })
}

/// Return the approved spawn cwd only after checking the current directory identity and generation.
pub(crate) fn checked(
    database: &Connection,
    data: &Path,
    id: &str,
    fence: &str,
) -> Result<(ProjectSummary, Directory)> {
    let record = current(database, id, fence)?;
    let root = canonical(&record.root, data)?;
    if root.identity != record.identity {
        return Err(Error::Rejected("PROJECT_ROOT_CHANGED"));
    }
    Ok((record.summary, root))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> (tempfile::TempDir, PathBuf, Connection) {
        let temp = tempfile::tempdir().unwrap();
        let data = temp.path().join("data");
        std::fs::create_dir(&data).unwrap();
        let database = Connection::open_in_memory().unwrap();
        database
            .execute_batch(include_str!("../../../fixtures/migrations/v2.sql"))
            .unwrap();
        database
            .execute_batch(include_str!("../../../fixtures/migrations/v3.sql"))
            .unwrap();
        database
            .execute_batch(include_str!("../../../fixtures/migrations/v6.sql"))
            .unwrap();
        (temp, data, database)
    }
    #[test]
    fn identity_deduplication_rotation_removal_and_reapproval_are_fenced() {
        let (temp, data, db) = fixture();
        let a = temp.path().join("a");
        let b = temp.path().join("b");
        std::fs::create_dir_all(a.join("child")).unwrap();
        std::fs::create_dir(&b).unwrap();
        let add_params = || AddProjectParams {
            root: a.to_str().unwrap().into(),
            name: None,
        };
        let (first, changed) = add(&db, &data, add_params()).unwrap();
        assert!(changed);
        assert_eq!(first.name, "a");
        assert_eq!(first.id.len(), 32);
        assert_eq!(first.cwd_fence.len(), 32);
        let writes = db.total_changes();
        assert_eq!(
            add(&db, &data, add_params()).unwrap(),
            (first.clone(), false)
        );
        assert_eq!(db.total_changes(), writes);
        let nested = add(
            &db,
            &data,
            AddProjectParams {
                root: a.join("child").to_str().unwrap().into(),
                name: None,
            },
        )
        .unwrap()
        .0;
        assert_ne!(nested.id, first.id);
        let same = update(
            &db,
            &data,
            UpdateProjectParams {
                project_id: first.id.clone(),
                expected_cwd_fence: first.cwd_fence.clone(),
                root: Some(a.join("child/..").to_str().unwrap().into()),
                name: None,
            },
        )
        .unwrap()
        .0;
        assert_eq!(same.cwd_fence, first.cwd_fence);
        let next = update(
            &db,
            &data,
            UpdateProjectParams {
                project_id: first.id.clone(),
                expected_cwd_fence: first.cwd_fence.clone(),
                root: Some(b.to_str().unwrap().into()),
                name: None,
            },
        )
        .unwrap()
        .0;
        assert_ne!(next.cwd_fence, first.cwd_fence);
        assert!(matches!(
            checked(&db, &data, &first.id, &first.cwd_fence),
            Err(Error::Rejected("STALE_CWD_FENCE"))
        ));
        remove(
            &db,
            RemoveProjectParams {
                project_id: next.id.clone(),
                expected_cwd_fence: next.cwd_fence.clone(),
            },
        )
        .unwrap();
        assert!(matches!(
            checked(&db, &data, &next.id, &next.cwd_fence),
            Err(Error::Rejected("PROJECT_NOT_APPROVED"))
        ));
        let fresh = add(
            &db,
            &data,
            AddProjectParams {
                root: b.to_str().unwrap().into(),
                name: None,
            },
        )
        .unwrap()
        .0;
        assert_ne!(fresh.id, next.id);
        assert_ne!(fresh.cwd_fence, next.cwd_fence);
    }
    #[test]
    fn roots_fail_closed_after_replacement_and_data_descendants_are_forbidden() {
        let (temp, data, db) = fixture();
        let a = temp.path().join("a");
        std::fs::create_dir(&a).unwrap();
        let first = add(
            &db,
            &data,
            AddProjectParams {
                root: a.to_str().unwrap().into(),
                name: None,
            },
        )
        .unwrap()
        .0;
        std::fs::rename(&a, temp.path().join("old")).unwrap();
        assert!(matches!(
            checked(&db, &data, &first.id, &first.cwd_fence),
            Err(Error::Rejected("PROJECT_ROOT_UNAVAILABLE"))
        ));
        // A label-only edit still works when its directory is unavailable.
        let renamed = update(
            &db,
            &data,
            UpdateProjectParams {
                project_id: first.id.clone(),
                expected_cwd_fence: first.cwd_fence.clone(),
                root: None,
                name: Some("Renamed".into()),
            },
        )
        .unwrap()
        .0;
        assert_eq!(renamed.cwd_fence, first.cwd_fence);
        std::fs::create_dir(&a).unwrap();
        assert!(matches!(
            checked(&db, &data, &first.id, &first.cwd_fence),
            Err(Error::Rejected("PROJECT_ROOT_CHANGED"))
        ));
        let writes = db.total_changes();
        assert!(matches!(
            add(
                &db,
                &data,
                AddProjectParams {
                    root: a.to_str().unwrap().into(),
                    name: None,
                }
            ),
            Err(Error::Rejected("PROJECT_ROOT_CHANGED"))
        ));
        assert_eq!(db.total_changes(), writes);
        let reapproved = update(
            &db,
            &data,
            UpdateProjectParams {
                project_id: first.id.clone(),
                expected_cwd_fence: first.cwd_fence.clone(),
                root: Some(a.to_str().unwrap().into()),
                name: None,
            },
        )
        .unwrap()
        .0;
        assert_ne!(reapproved.cwd_fence, first.cwd_fence);
        std::fs::create_dir(data.join("child")).unwrap();
        for path in [&data, data.join("child").as_path()] {
            assert!(matches!(
                add(
                    &db,
                    &data,
                    AddProjectParams {
                        root: path.to_str().unwrap().into(),
                        name: None
                    }
                ),
                Err(Error::Rejected("PROJECT_ROOT_UNSUPPORTED"))
            ));
        }
    }
    #[test]
    fn active_sessions_block_changes_but_terminal_history_survives_removal() {
        let (temp, data, db) = fixture();
        let a = temp.path().join("a");
        std::fs::create_dir(&a).unwrap();
        let project = add(
            &db,
            &data,
            AddProjectParams {
                root: a.to_str().unwrap().into(),
                name: None,
            },
        )
        .unwrap()
        .0;
        let params = CreateSessionParams {
            session_id: "session".into(),
            project_id: project.id.clone(),
            runtime_id: "pi".into(),
            expected_cwd_fence: project.cwd_fence.clone(),
            start: SessionStart::New,
        };
        let runtime = RuntimeSummary {
            id: "pi".into(),
            name: "fake".into(),
            version: None,
            available: true,
            tool_approval: ToolApproval::Unknown,
        };
        let session = sessions::create(&db, &params, Some(&project), Some(&runtime))
            .unwrap()
            .0;
        let remove_params = || RemoveProjectParams {
            project_id: project.id.clone(),
            expected_cwd_fence: project.cwd_fence.clone(),
        };
        assert!(matches!(
            remove(&db, remove_params()),
            Err(Error::Rejected("PROJECT_IN_USE"))
        ));
        let renamed = update(
            &db,
            &data,
            UpdateProjectParams {
                project_id: project.id.clone(),
                expected_cwd_fence: project.cwd_fence.clone(),
                root: None,
                name: Some("x".into()),
            },
        )
        .unwrap()
        .0;
        assert_eq!(renamed.name, "x");
        assert_eq!(renamed.cwd_fence, project.cwd_fence);
        assert!(matches!(
            update(
                &db,
                &data,
                UpdateProjectParams {
                    project_id: project.id.clone(),
                    expected_cwd_fence: project.cwd_fence.clone(),
                    root: Some(a.to_str().unwrap().into()),
                    name: None,
                }
            ),
            Err(Error::Rejected("PROJECT_IN_USE"))
        ));
        let mut stopped = session.clone();
        stopped.state = SessionState::Stopped;
        stopped.stop_reason = Some(SessionStopReason::AgentExited);
        let stopped = sessions::transition(&db, &session, stopped).unwrap().0;
        remove(&db, remove_params()).unwrap();
        assert_eq!(sessions::get(&db, "session").unwrap(), Some(stopped));
    }
}
