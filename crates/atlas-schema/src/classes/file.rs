//! File System Activity (OCSF 1001), spec section 5.5.

use atlas_proto::v1 as wire;
use atlas_proto::v1::file_system_activity::Activity as W;

use crate::convert::{Result, require};
use crate::objects::{File, ProcessRef};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileSystemActivity {
    pub actor: ProcessRef,
    /// For `Rename`: the original file.
    pub file: File,
    pub action: FileAction,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileAction {
    Create,
    Read,
    Update,
    Delete,
    Rename { file_result: File },
    SetAttributes,
}

impl From<FileSystemActivity> for wire::FileSystemActivity {
    fn from(v: FileSystemActivity) -> Self {
        let activity = match v.action {
            FileAction::Create => W::Create(wire::FileCreate {}),
            FileAction::Read => W::Read(wire::FileRead {}),
            FileAction::Update => W::Update(wire::FileUpdate {}),
            FileAction::Delete => W::Delete(wire::FileDelete {}),
            FileAction::Rename { file_result } => W::Rename(wire::FileRename { file_result: Some(file_result.into()) }),
            FileAction::SetAttributes => W::SetAttributes(wire::FileSetAttributes {}),
        };
        Self { actor: Some(v.actor.into()), file: Some(v.file.into()), activity: Some(activity) }
    }
}

impl FileSystemActivity {
    pub(crate) fn from_wire(w: wire::FileSystemActivity) -> Result<Self> {
        Ok(Self {
            actor: ProcessRef::required(w.actor, "", "actor.process")?,
            file: File::required(w.file, "", "file")?,
            action: match require(w.activity, "", "activity")? {
                W::Create(_) => FileAction::Create,
                W::Read(_) => FileAction::Read,
                W::Update(_) => FileAction::Update,
                W::Delete(_) => FileAction::Delete,
                W::Rename(r) => FileAction::Rename { file_result: File::required(r.file_result, "", "file_result")? },
                W::SetAttributes(_) => FileAction::SetAttributes,
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::SchemaErrorKind;
    use crate::objects::test_support::{file, proc_ref};

    fn activity(action: FileAction) -> FileSystemActivity {
        FileSystemActivity { actor: proc_ref(), file: file("C:\\a.txt"), action }
    }

    #[test]
    fn every_action_round_trips() {
        let actions = [
            FileAction::Create,
            FileAction::Read,
            FileAction::Update,
            FileAction::Delete,
            FileAction::Rename { file_result: file("C:\\b.txt") },
            FileAction::SetAttributes,
        ];
        for action in actions {
            let a = activity(action);
            assert_eq!(FileSystemActivity::from_wire(a.clone().into()).unwrap(), a);
        }
    }

    #[test]
    fn rename_without_result_is_missing_file_result() {
        let mut w = wire::FileSystemActivity::from(activity(FileAction::Rename { file_result: file("C:\\b.txt") }));
        let Some(W::Rename(r)) = w.activity.as_mut() else { unreachable!() };
        r.file_result = None;
        let e = FileSystemActivity::from_wire(w).unwrap_err();
        assert_eq!((e.field_path.as_str(), e.kind), ("file_result", SchemaErrorKind::Missing));
    }
}
