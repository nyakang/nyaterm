//! FTP has no portable atomic replace command. Preserve an old target until a
//! replacement is confirmed, and retain recovery files after ambiguous replies.
//! The existence check cannot provide compare-and-swap or atomic no-replace
//! semantics against another client; FTP servers expose no portable primitive
//! for those guarantees.
use crate::ftp::{FtpEntry, FtpEntryKind, FtpError};

pub(super) trait Replacement {
    fn stat(&self, path: &str) -> Result<FtpEntry, FtpError>;
    fn rename(&self, from: &str, to: &str) -> Result<(), FtpError>;
    fn remove(&self, path: &str) -> Result<(), FtpError>;
}

fn uncertain(error: &FtpError) -> bool {
    matches!(
        error,
        FtpError::Connection | FtpError::Timeout | FtpError::Unavailable
    )
}

pub(super) fn commit(
    files: &impl Replacement,
    temporary: &str,
    destination: &str,
    replace_existing: bool,
) -> Result<Option<String>, FtpError> {
    let exists = match files.stat(destination) {
        Ok(entry) if entry.kind == FtpEntryKind::File => true,
        Ok(_) => return Err(FtpError::InvalidPath),
        Err(FtpError::NotFound) => false,
        Err(error) => return Err(error),
    };
    if exists && !replace_existing {
        return Err(FtpError::InvalidPath);
    }
    let recovery = |backup_path| FtpError::RecoveryRequired {
        temporary_path: temporary.to_owned(),
        backup_path,
    };
    let first = match files.rename(temporary, destination) {
        Ok(()) => return Ok(None),
        Err(error) if uncertain(&error) => return Err(recovery(None)),
        Err(error) => error,
    };
    if !exists {
        return Err(first);
    }
    let backup = format!("{destination}.nyaterm-backup-{}", nyaterm_core::uuid());
    match files.rename(destination, &backup) {
        Ok(()) => {}
        Err(error) if uncertain(&error) => return Err(recovery(Some(backup))),
        Err(error) => return Err(error),
    }
    match files.rename(temporary, destination) {
        Ok(()) => {
            // The new target is committed. Cleanup failure is a successful save
            // with a retained backup, not a reason to roll the target back.
            Ok(files.remove(&backup).err().map(|_| backup))
        }
        Err(error) if uncertain(&error) => Err(recovery(Some(backup))),
        Err(error) => {
            // Never overwrite a third party's file while trying to roll back.
            if !matches!(files.stat(destination), Err(FtpError::NotFound))
                || files.rename(&backup, destination).is_err()
            {
                return Err(recovery(Some(backup)));
            }
            Err(error)
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::ftp::replace::{Replacement, commit};
    use crate::ftp::{FtpEntry, FtpEntryKind, FtpError};
    use std::cell::RefCell;
    use std::collections::{HashMap, VecDeque};

    struct Files {
        nodes: RefCell<HashMap<String, Vec<u8>>>,
        renames: RefCell<VecDeque<Result<(), FtpError>>>,
        cleanup_fails: bool,
    }
    impl Files {
        fn new(renames: Vec<Result<(), FtpError>>) -> Self {
            Self {
                nodes: RefCell::new(HashMap::from([
                    ("/target".into(), b"old".to_vec()),
                    ("/new".into(), b"new".to_vec()),
                ])),
                renames: RefCell::new(renames.into()),
                cleanup_fails: false,
            }
        }
        fn read(&self, path: &str) -> Option<Vec<u8>> {
            self.nodes.borrow().get(path).cloned()
        }
        fn backup(&self) -> Option<String> {
            self.nodes
                .borrow()
                .keys()
                .find(|p| p.contains("nyaterm-backup-"))
                .cloned()
        }
    }
    impl Replacement for Files {
        fn stat(&self, path: &str) -> Result<FtpEntry, FtpError> {
            let size = self.read(path).ok_or(FtpError::NotFound)?.len() as u64;
            Ok(FtpEntry {
                name: path.into(),
                path: path.into(),
                kind: FtpEntryKind::File,
                size,
                modified: None,
            })
        }
        fn rename(&self, from: &str, to: &str) -> Result<(), FtpError> {
            self.renames.borrow_mut().pop_front().unwrap_or(Ok(()))?;
            let mut nodes = self.nodes.borrow_mut();
            let content = nodes.remove(from).ok_or(FtpError::NotFound)?;
            nodes.insert(to.into(), content);
            Ok(())
        }
        fn remove(&self, path: &str) -> Result<(), FtpError> {
            if self.cleanup_fails {
                return Err(refused());
            }
            self.nodes.borrow_mut().remove(path);
            Ok(())
        }
    }
    fn refused() -> FtpError {
        FtpError::Protocol("550".into())
    }

    #[test]
    fn atomic_rename_replaces_without_deleting_the_original_first() {
        let files = Files::new(vec![]);
        assert_eq!(commit(&files, "/new", "/target", true), Ok(None));
        assert_eq!(files.read("/target").unwrap(), b"new");
        assert!(files.backup().is_none());
    }
    #[test]
    fn refused_overwrite_moves_old_target_aside_until_commit_is_confirmed() {
        let files = Files::new(vec![Err(refused()), Ok(()), Ok(())]);
        assert_eq!(commit(&files, "/new", "/target", true), Ok(None));
        assert_eq!(files.read("/target").unwrap(), b"new");
        assert!(files.backup().is_none());
    }
    #[test]
    fn refused_second_rename_restores_old_target_and_preserves_new_file() {
        let files = Files::new(vec![Err(refused()), Ok(()), Err(refused()), Ok(())]);
        assert_eq!(commit(&files, "/new", "/target", true), Err(refused()));
        assert_eq!(files.read("/target").unwrap(), b"old");
        assert_eq!(files.read("/new").unwrap(), b"new");
    }
    #[test]
    fn lost_commit_reply_keeps_both_recovery_files() {
        let files = Files::new(vec![Err(refused()), Ok(()), Err(FtpError::Connection)]);
        assert!(matches!(
            commit(&files, "/new", "/target", true),
            Err(FtpError::RecoveryRequired {
                backup_path: Some(_),
                ..
            })
        ));
        assert_eq!(files.read(&files.backup().unwrap()).unwrap(), b"old");
        assert_eq!(files.read("/new").unwrap(), b"new");
    }
    #[test]
    fn failed_rollback_does_not_delete_the_backup() {
        let files = Files::new(vec![Err(refused()), Ok(()), Err(refused()), Err(refused())]);
        assert!(matches!(
            commit(&files, "/new", "/target", true),
            Err(FtpError::RecoveryRequired { .. })
        ));
        assert_eq!(files.read(&files.backup().unwrap()).unwrap(), b"old");
    }
    #[test]
    fn cleanup_failure_reports_a_successful_commit_with_backup() {
        let mut files = Files::new(vec![Err(refused()), Ok(()), Ok(())]);
        files.cleanup_fails = true;
        let backup = commit(&files, "/new", "/target", true).unwrap().unwrap();
        assert_eq!(files.read("/target").unwrap(), b"new");
        assert_eq!(files.read(&backup).unwrap(), b"old");
    }
    #[test]
    fn ambiguous_first_rename_never_tries_destructive_fallback() {
        let files = Files::new(vec![Err(FtpError::Timeout)]);
        assert!(matches!(
            commit(&files, "/new", "/target", true),
            Err(FtpError::RecoveryRequired {
                backup_path: None,
                ..
            })
        ));
        assert_eq!(files.read("/target").unwrap(), b"old");
        assert!(files.backup().is_none());
    }
}

#[cfg(test)]
mod create_tests {
    use crate::ftp::replace::{Replacement, commit};
    use crate::ftp::{FtpEntry, FtpEntryKind, FtpError};
    struct RacingTarget;
    impl Replacement for RacingTarget {
        fn stat(&self, path: &str) -> Result<FtpEntry, FtpError> {
            Ok(FtpEntry {
                name: "target".into(),
                path: path.into(),
                kind: FtpEntryKind::File,
                size: 3,
                modified: None,
            })
        }
        fn rename(&self, _: &str, _: &str) -> Result<(), FtpError> {
            panic!("a new upload cannot overwrite a racing target")
        }
        fn remove(&self, _: &str) -> Result<(), FtpError> {
            panic!("a racing target cannot be removed")
        }
    }
    #[test]
    fn new_upload_refuses_a_target_created_during_streaming() {
        assert_eq!(
            commit(&RacingTarget, "/temporary", "/target", false),
            Err(FtpError::InvalidPath)
        );
    }
}
