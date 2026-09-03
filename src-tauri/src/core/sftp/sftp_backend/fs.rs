//! Internal pieces of the SFTP backend moved out of `sftp_backend.rs`.

use super::*;

#[async_trait::async_trait]
impl RemoteFs for SftpBackend {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn backend_name(&self) -> &'static str {
        "sftp"
    }

    async fn home_dir(&self) -> AppResult<String> {
        let sftp = self.open_sftp().await?;
        let home = sftp.canonicalize(".").await?;
        let _ = sftp.close().await;

        if home.is_empty() {
            Err(AppError::Config(
                "Failed to determine home directory".to_string(),
            ))
        } else {
            Ok(home)
        }
    }

    async fn list_dir(&self, path: &str) -> AppResult<Vec<FileEntry>> {
        let path_ref = RemotePathRef::new(path, None)?;
        self.list_dir_ref(&path_ref).await
    }

    async fn list_dir_ref(&self, path: &RemotePathRef) -> AppResult<Vec<FileEntry>> {
        let sftp = self.open_sftp().await?;

        let path_bytes = normalize_remote_dir_path_bytes(&self.remote_path_bytes(path));
        if path.raw_path().is_some() {
            self.path_cache
                .write()
                .await
                .insert(path.display_path().to_string(), path_bytes.clone());
        }
        let dir = sftp.read_dir_bytes(path_bytes.clone()).await?;

        let mut pending = Vec::new();
        let mut uid_set = HashSet::new();
        let mut gid_set = HashSet::new();
        let normalized_path = normalize_remote_dir_path(path.display_path());

        for entry in dir {
            let name_from_entry = entry.file_name();
            if name_from_entry == "." || name_from_entry == ".." {
                continue;
            }

            // Get raw bytes for the file name to preserve original encoding
            let name_bytes = entry.file_name_bytes().to_vec();

            // Decode using the connection's encoding setting
            let name = self.decode_path_from_sftp(&name_bytes);

            let full_path = join_remote_child(&normalized_path, &name);

            let full_path_bytes = join_remote_child_bytes(&path_bytes, &name_bytes);
            let raw_path_token = raw_path_token(&full_path_bytes);
            self.path_cache
                .write()
                .await
                .insert(full_path.clone(), full_path_bytes.clone());

            let file_type = entry.file_type();
            let is_symlink = file_type == FileType::Symlink;

            // Use raw bytes for metadata operation to handle non-UTF-8 paths
            let is_symlink_to_dir = is_symlink
                && sftp
                    .metadata_bytes(full_path_bytes.clone())
                    .await
                    .ok()
                    .as_ref()
                    .map_or(false, sftp_attrs_is_dir);
            let is_dir = file_type == FileType::Dir || is_symlink_to_dir;
            let type_char = if is_dir {
                if is_symlink { 'l' } else { 'd' }
            } else if is_symlink {
                'l'
            } else {
                '-'
            };

            let attrs = entry.metadata();
            let size = attrs.size.unwrap_or(0);
            let perms = attrs.permissions.unwrap_or(0);
            let permissions = permissions_to_string(perms, type_char);
            let mtime = u64::from(attrs.mtime.unwrap_or(0));

            if attrs
                .user
                .as_deref()
                .is_none_or(|value| value.trim().is_empty())
            {
                if let Some(uid) = attrs.uid {
                    uid_set.insert(uid);
                }
            }
            if attrs
                .group
                .as_deref()
                .is_none_or(|value| value.trim().is_empty())
            {
                if let Some(gid) = attrs.gid {
                    gid_set.insert(gid);
                }
            }

            pending.push((
                name,
                is_dir,
                is_symlink,
                size,
                permissions,
                attrs,
                mtime,
                raw_path_token,
            ));
        }

        let _ = sftp.close().await;
        let user_names = self.resolve_uid_names(uid_set).await;
        let group_names = self.resolve_gid_names(gid_set).await;
        let entries = pending
            .into_iter()
            .map(
                |(name, is_dir, is_symlink, size, permissions, attrs, mtime, raw_path_token)| {
                    FileEntry {
                        name,
                        is_dir,
                        is_symlink,
                        size,
                        permissions,
                        owner: attrs
                            .uid
                            .and_then(|uid| user_names.get(&uid).cloned())
                            .unwrap_or_else(|| owner_or_id(&attrs.user, attrs.uid)),
                        group: attrs
                            .gid
                            .and_then(|gid| group_names.get(&gid).cloned())
                            .unwrap_or_else(|| group_or_id(&attrs.group, attrs.gid)),
                        mtime,
                        raw_path_token: Some(raw_path_token),
                    }
                },
            )
            .collect();
        Ok(entries)
    }

    async fn stat(&self, path: &str) -> AppResult<FileProperties> {
        let path_ref = RemotePathRef::new(path, None)?;
        self.stat_ref(&path_ref).await
    }

    async fn stat_ref(&self, path: &RemotePathRef) -> AppResult<FileProperties> {
        let sftp = self.open_sftp().await?;
        let raw_path = self.remote_path_bytes(path);
        let attrs = sftp.symlink_metadata_bytes(raw_path.clone()).await?;
        let is_symlink = sftp_attrs_is_symlink(&attrs);
        let symlink_target = if is_symlink {
            match sftp.read_link_bytes(raw_path.clone()).await {
                Ok(target) => Some(self.decode_path_from_sftp(&target)),
                Err(error) => {
                    let _ = sftp.close().await;
                    return Err(error.into());
                }
            }
        } else {
            None
        };
        let target_attrs = if is_symlink {
            sftp.metadata_bytes(raw_path).await.ok()
        } else {
            None
        };
        let _ = sftp.close().await;

        let perms = attrs.permissions.unwrap_or(0);
        let is_dir =
            sftp_attrs_is_dir(&attrs) || target_attrs.as_ref().map_or(false, sftp_attrs_is_dir);
        let type_char = if is_dir {
            if is_symlink { 'l' } else { 'd' }
        } else if is_symlink {
            'l'
        } else {
            '-'
        };
        let permissions = permissions_to_string(perms, type_char);
        let name = path
            .display_path()
            .split('/')
            .last()
            .unwrap_or(path.display_path())
            .to_string();
        let owner = if let Some(uid) = attrs.uid {
            self.resolve_uid_names(HashSet::from([uid]))
                .await
                .get(&uid)
                .cloned()
                .unwrap_or_else(|| owner_or_id(&attrs.user, attrs.uid))
        } else {
            owner_or_id(&attrs.user, attrs.uid)
        };
        let group = if let Some(gid) = attrs.gid {
            self.resolve_gid_names(HashSet::from([gid]))
                .await
                .get(&gid)
                .cloned()
                .unwrap_or_else(|| group_or_id(&attrs.group, attrs.gid))
        } else {
            group_or_id(&attrs.group, attrs.gid)
        };

        Ok(FileProperties {
            name,
            is_dir,
            is_symlink,
            symlink_target,
            size: attrs.size.unwrap_or(0),
            permissions,
            owner,
            group,
            uid: attrs.uid.map_or_else(String::new, |v| v.to_string()),
            gid: attrs.gid.map_or_else(String::new, |v| v.to_string()),
            mtime: u64::from(attrs.mtime.unwrap_or(0)),
            atime: u64::from(attrs.atime.unwrap_or(0)),
        })
    }

    async fn mkdir(&self, path: &str, mode: Option<String>) -> AppResult<()> {
        let sftp = self.open_sftp().await?;
        let path_bytes = self.encode_path_for_sftp(path);
        sftp.create_dir_bytes(path_bytes.clone()).await?;
        if let Some(ref m) = mode {
            apply_remote_mode_after_create_bytes(&sftp, path, path_bytes, m, "directory").await?;
        }
        let _ = sftp.close().await;
        Ok(())
    }

    async fn remove_file(&self, path: &str) -> AppResult<()> {
        let path_ref = RemotePathRef::new(path, None)?;
        self.remove_file_ref(&path_ref).await
    }

    async fn remove_file_ref(&self, path: &RemotePathRef) -> AppResult<()> {
        let sftp = self.open_sftp().await?;
        let raw_path = self.remote_path_bytes(path);

        let meta = match sftp.symlink_metadata_bytes(raw_path.clone()).await {
            Ok(meta) => meta,
            Err(error) if is_sftp_not_found(&error) => {
                let _ = sftp.close().await;
                return Ok(());
            }
            Err(error) => {
                let _ = sftp.close().await;
                return Err(error.into());
            }
        };

        if sftp_attrs_is_symlink(&meta) {
            ignore_sftp_not_found(sftp.remove_file_bytes(raw_path).await)?;
        } else if sftp_attrs_is_dir(&meta) {
            let _ = sftp.close().await;
            self.remove_dir_fast_ref(path).await?;
            return Ok(());
        } else {
            ignore_sftp_not_found(sftp.remove_file_bytes(raw_path).await)?;
        }
        let _ = sftp.close().await;
        Ok(())
    }

    async fn rename(&self, old_path: &str, new_path: &str) -> AppResult<()> {
        let old_ref = RemotePathRef::new(old_path, None)?;
        let new_ref = RemotePathRef::new(new_path, None)?;
        self.rename_ref(&old_ref, &new_ref).await
    }

    async fn rename_ref(
        &self,
        old_path: &RemotePathRef,
        new_path: &RemotePathRef,
    ) -> AppResult<()> {
        let sftp = self.open_sftp().await?;
        sftp.rename_bytes(
            self.remote_path_bytes(old_path),
            self.remote_path_bytes(new_path),
        )
        .await?;
        let _ = sftp.close().await;
        Ok(())
    }

    async fn create_file(&self, path: &str, mode: Option<String>) -> AppResult<()> {
        let sftp = self.open_sftp().await?;

        // For non-UTF-8 encodings, encode the path in the target encoding
        // and use open_bytes with WRITE flag to create the file
        let result = if self.encoding != "UTF-8" {
            let path_bytes = self.encode_path_for_sftp(path);
            use russh_sftp::protocol::OpenFlags;
            sftp.open_with_flags_bytes(
                path_bytes,
                OpenFlags::WRITE | OpenFlags::CREATE | OpenFlags::TRUNCATE,
            )
            .await
        } else {
            sftp.create(path).await
        };

        match result {
            Ok(file) => {
                drop(file);
                if let Some(ref m) = mode {
                    let path_bytes = self.encode_path_for_sftp(path);
                    apply_remote_mode_after_create_bytes(&sftp, path, path_bytes, m, "file")
                        .await?;
                }
                let _ = sftp.close().await;
                Ok(())
            }
            Err(error) => {
                let _ = sftp.close().await;
                Err(error.into())
            }
        }
    }

    async fn create_symlink(&self, link_path: &str, target_path: &str) -> AppResult<()> {
        let link_path = RemotePathRef::new(link_path, None)?;
        self.create_symlink_ref(&link_path, target_path).await
    }

    async fn create_symlink_ref(
        &self,
        link_path: &RemotePathRef,
        target_path: &str,
    ) -> AppResult<()> {
        let sftp = self.open_sftp().await?;
        sftp.symlink_openssh_bytes(
            self.encode_path_for_sftp(target_path),
            self.remote_path_bytes(link_path),
        )
        .await?;
        let _ = sftp.close().await;
        Ok(())
    }

    async fn update_attrs(&self, path: &str, update: &RemoteFileAttributeUpdate) -> AppResult<()> {
        let path_ref = RemotePathRef::new(path, None)?;
        self.update_attrs_ref(&path_ref, update).await
    }

    async fn update_attrs_ref(
        &self,
        path: &RemotePathRef,
        update: &RemoteFileAttributeUpdate,
    ) -> AppResult<()> {
        let mode = update
            .mode
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(parse_octal_mode)
            .transpose()?
            .map(|value| value & POSIX_MODE_MASK);
        let uid = match update
            .owner
            .as_deref()
            .map(str::trim)
            .filter(|v| !v.is_empty())
        {
            Some(owner) => Some(self.resolve_user_to_uid(owner).await?),
            None => None,
        };
        let gid = match update
            .group
            .as_deref()
            .map(str::trim)
            .filter(|v| !v.is_empty())
        {
            Some(group) => Some(self.resolve_group_to_gid(group).await?),
            None => None,
        };

        if mode.is_none() && uid.is_none() && gid.is_none() {
            return Ok(());
        }

        let sftp = self.open_sftp().await?;
        let path_bytes = self.remote_path_bytes(path);
        if update.recursive {
            apply_remote_attrs_recursive_bytes(
                &sftp,
                path.display_path(),
                path_bytes,
                mode,
                uid,
                gid,
            )
            .await?;
        } else {
            apply_remote_attrs_bytes(&sftp, path.display_path(), path_bytes, mode, uid, gid)
                .await?;
        }
        let _ = sftp.close().await;
        Ok(())
    }

    async fn read_file_text(&self, path: &str, max_bytes: u64) -> AppResult<RemoteTextFile> {
        use tokio::io::AsyncReadExt;

        let sftp = self.open_sftp().await?;
        let attrs = sftp.metadata(path).await?;
        let size = attrs.size.unwrap_or(0);
        let mtime = u64::from(attrs.mtime.unwrap_or(0));
        let type_bits = attrs.permissions.unwrap_or(0) & SFTP_FILE_TYPE_MASK;
        if type_bits == 0o040000 {
            let _ = sftp.close().await;
            return Err(AppError::Config(
                "Directories cannot be opened as text".to_string(),
            ));
        }
        if size > max_bytes {
            let _ = sftp.close().await;
            return Err(AppError::Config(format!(
                "File is too large to open as text ({} bytes > {} bytes)",
                size, max_bytes
            )));
        }

        let mut file = sftp
            .open(path)
            .await
            .map_err(|error| AppError::Channel(format!("Failed to open remote file: {error}")))?;
        let mut bytes = Vec::with_capacity(size as usize);
        file.read_to_end(&mut bytes)
            .await
            .map_err(|error| AppError::Channel(format!("Failed to read remote file: {error}")))?;
        let _ = sftp.close().await;

        ensure_text_bytes(&bytes, max_bytes)?;
        let file_hash = content_hash(&bytes);
        let content = String::from_utf8(bytes)
            .map_err(|_| AppError::Config("Only UTF-8 text files are supported".to_string()))?;

        Ok(RemoteTextFile {
            path: path.to_string(),
            content,
            size,
            mtime,
            mtime_nanos: None,
            content_hash: file_hash,
        })
    }

    async fn read_file_bytes(&self, path: &str, max_bytes: u64) -> AppResult<RemoteBinaryFile> {
        use tokio::io::AsyncReadExt;

        let sftp = self.open_sftp().await?;
        let attrs = sftp.metadata(path).await?;
        let size = attrs.size.unwrap_or(0);
        let mtime = u64::from(attrs.mtime.unwrap_or(0));
        let type_bits = attrs.permissions.unwrap_or(0) & SFTP_FILE_TYPE_MASK;
        if type_bits == 0o040000 {
            let _ = sftp.close().await;
            return Err(AppError::Config(
                "Directories cannot be previewed".to_string(),
            ));
        }
        if size > max_bytes {
            let _ = sftp.close().await;
            return Err(AppError::Config(format!(
                "File is too large to preview ({} bytes > {} bytes)",
                size, max_bytes
            )));
        }

        let mut file = sftp
            .open(path)
            .await
            .map_err(|error| AppError::Channel(format!("Failed to open remote file: {error}")))?;
        let mut bytes = Vec::with_capacity(size as usize);
        file.read_to_end(&mut bytes)
            .await
            .map_err(|error| AppError::Channel(format!("Failed to read remote file: {error}")))?;
        let _ = sftp.close().await;

        Ok(RemoteBinaryFile {
            path: path.to_string(),
            content_bytes: bytes,
            size,
            mtime,
            mtime_nanos: None,
        })
    }

    async fn write_file_text(
        &self,
        path: &str,
        content: &str,
        expected_mtime: Option<u64>,
        expected_size: Option<u64>,
        expected_hash: Option<&str>,
        force: bool,
    ) -> AppResult<WriteRemoteTextResult> {
        use tokio::io::AsyncReadExt;
        use tokio::io::AsyncWriteExt;

        let sftp = self.open_sftp().await?;
        let original_attrs = match sftp.metadata(path).await {
            Ok(attrs) => Some(attrs),
            Err(error) if force && is_sftp_not_found(&error) => None,
            Err(error) => {
                let _ = sftp.close().await;
                return Err(error.into());
            }
        };
        if !force {
            let attrs = original_attrs.as_ref().expect("metadata checked above");
            let current_mtime = u64::from(attrs.mtime.unwrap_or(0));
            let current_size = attrs.size.unwrap_or(0);
            if expected_mtime.is_some_and(|mtime| mtime != current_mtime)
                || expected_size.is_some_and(|size| size != current_size)
            {
                let _ = sftp.close().await;
                return Ok(WriteRemoteTextResult::conflict(
                    current_mtime,
                    current_size,
                    None,
                ));
            }

            if let Some(expected_hash) = expected_hash {
                let mut file = sftp.open(path).await.map_err(|error| {
                    AppError::Channel(format!("Failed to open remote file: {error}"))
                })?;
                let mut current_bytes = Vec::with_capacity(current_size as usize);
                file.read_to_end(&mut current_bytes)
                    .await
                    .map_err(|error| {
                        AppError::Channel(format!("Failed to read remote file: {error}"))
                    })?;
                drop(file);
                if content_hash(&current_bytes) != expected_hash {
                    let _ = sftp.close().await;
                    return Ok(WriteRemoteTextResult::conflict(
                        current_mtime,
                        current_size,
                        None,
                    ));
                }
            }
        }
        let original_permissions = original_attrs
            .as_ref()
            .and_then(|attrs| permissions_to_preserve_after_write(attrs.permissions));

        let mut file = sftp
            .create(path)
            .await
            .map_err(|error| AppError::Channel(format!("Failed to open remote file: {error}")))?;
        file.write_all(content.as_bytes())
            .await
            .map_err(|error| AppError::Channel(format!("Failed to write remote file: {error}")))?;
        file.shutdown()
            .await
            .map_err(|error| AppError::Channel(format!("Failed to close remote file: {error}")))?;
        drop(file);

        if let Some(permissions) = original_permissions {
            let path_bytes = self.encode_path_for_sftp(path);
            apply_remote_attrs_bytes(&sftp, path, path_bytes, Some(permissions), None, None)
                .await
                .map_err(|error| {
                    AppError::Channel(format!(
                        "Failed to restore remote file permissions: {error}"
                    ))
                })?;

            let actual_permissions = sftp
                .metadata(path)
                .await
                .ok()
                .and_then(|attrs| permissions_to_preserve_after_write(attrs.permissions));
            if actual_permissions != Some(permissions) {
                tracing::warn!(
                    remote_path = path,
                    requested_permissions = format!("{permissions:#06o}"),
                    actual_permissions = ?actual_permissions.map(|mode| format!("{mode:#06o}")),
                    "SFTP permission restore did not take effect; falling back to chmod"
                );

                self.exec_ok(&format!("chmod {permissions:04o} -- {}", sh_quote(path)))
                    .await
                    .map_err(|error| {
                        AppError::Channel(format!(
                            "Failed to restore remote file permissions with chmod: {error}"
                        ))
                    })?;

                let actual_permissions = sftp
                    .metadata(path)
                    .await
                    .ok()
                    .and_then(|attrs| permissions_to_preserve_after_write(attrs.permissions));
                if actual_permissions != Some(permissions) {
                    return Err(AppError::Channel(format!(
                        "Failed to restore remote file permissions: requested {permissions:#06o}, actual {}",
                        actual_permissions
                            .map(|mode| format!("{mode:#06o}"))
                            .unwrap_or_else(|| "unknown".to_string())
                    )));
                }
            }
        }

        let attrs = sftp.metadata(path).await?;
        let _ = sftp.close().await;
        Ok(WriteRemoteTextResult::saved(
            u64::from(attrs.mtime.unwrap_or(0)),
            attrs.size.unwrap_or(content.len() as u64),
            None,
            content_hash(content.as_bytes()),
        ))
    }

    async fn download_file(
        &self,
        app: &tauri::AppHandle,
        session_id: &str,
        remote_path: &str,
        local_path: &str,
        transfer_settings: &crate::config::TransferSettings,
        transfer_id: Option<String>,
    ) -> AppResult<()> {
        let max_retries = transfer_settings.max_transfer_retries;
        let safe_local_path = sanitize_local_download_target(local_path, remote_path);
        let actual_local_path =
            match resolve_local_path(&safe_local_path, &transfer_settings.duplicate_strategy) {
                Some(path) => path,
                None => {
                    let file_name = remote_path.split('/').last().unwrap_or(remote_path);
                    let transfer_id = transfer_id
                        .clone()
                        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
                    remember_transfer_target_external(
                        transfer_id.clone(),
                        safe_local_path.clone(),
                        "download".to_string(),
                        "file".to_string(),
                    );
                    let _ = app.emit(
                        "transfer-event",
                        &TransferEvent {
                            id: transfer_id,
                            session_id: session_id.to_string(),
                            file_name: file_name.to_string(),
                            remote_path: remote_path.to_string(),
                            local_path: safe_local_path,
                            direction: "download".to_string(),
                            kind: "file".to_string(),
                            status: "completed".to_string(),
                            size: 0,
                            bytes_transferred: 0,
                            total_size: 0,
                            parent_id: None,
                            item_count_total: None,
                            item_count_completed: None,
                            error_msg: None,
                        },
                    );
                    return Ok(());
                }
            };

        let mut last_err = None;
        for attempt in 0..=max_retries {
            if attempt > 0 {
                log_event(StructuredLog {
                    level: StructuredLogLevel::Info,
                    domain: "transfer.lifecycle".to_string(),
                    event: "transfer.retry".to_string(),
                    message: "Retrying download".to_string(),
                    ids: Some(serde_json::json!({ "session_id": session_id })),
                    data: Some(serde_json::json!({
                        "direction": "download",
                        "attempt": attempt,
                        "remote_path": remote_path,
                    })),
                    error: None,
                    client_timestamp: None,
                });
            }
            match download_remote_file_inner_with_controller(
                self,
                app,
                session_id,
                remote_path,
                &actual_local_path,
                transfer_settings,
                create_child_file_transfer_controller(
                    transfer_id.clone(),
                    session_id,
                    file_name_from_path(remote_path),
                    remote_path,
                    &actual_local_path,
                    "download",
                    None,
                ),
                None,
            )
            .await
            {
                Ok(()) => return Ok(()),
                Err(e) => {
                    if matches!(e, AppError::Cancelled(_)) {
                        return Err(e);
                    }
                    last_err = Some(e);
                }
            }
        }
        Err(last_err.unwrap())
    }

    async fn upload_file(
        &self,
        app: &tauri::AppHandle,
        session_id: &str,
        local_path: &str,
        remote_path: &str,
        transfer_settings: &crate::config::TransferSettings,
        transfer_id: Option<String>,
    ) -> AppResult<()> {
        let max_retries = transfer_settings.max_transfer_retries;
        let session_manager = app.state::<Arc<crate::core::SessionManager>>();
        let sftp_for_resolve = self.open_sftp().await?;
        let actual_remote_path = match resolve_remote_path(
            app,
            session_manager.inner(),
            &sftp_for_resolve,
            session_id,
            remote_path,
            &transfer_settings.duplicate_strategy,
        )
        .await
        {
            Some(path) => path,
            None => {
                let _ = sftp_for_resolve.close().await;
                return Ok(());
            }
        };
        let _ = sftp_for_resolve.close().await;

        let mut last_err = None;
        for attempt in 0..=max_retries {
            if attempt > 0 {
                log_event(StructuredLog {
                    level: StructuredLogLevel::Info,
                    domain: "transfer.lifecycle".to_string(),
                    event: "transfer.retry".to_string(),
                    message: "Retrying upload".to_string(),
                    ids: Some(serde_json::json!({ "session_id": session_id })),
                    data: Some(serde_json::json!({
                        "direction": "upload",
                        "attempt": attempt,
                        "local_path": local_path,
                    })),
                    error: None,
                    client_timestamp: None,
                });
            }
            match upload_local_file_inner_with_controller(
                self,
                app,
                session_id,
                local_path,
                &actual_remote_path,
                transfer_settings,
                create_child_file_transfer_controller(
                    transfer_id.clone(),
                    session_id,
                    file_name_from_path(&actual_remote_path),
                    &actual_remote_path,
                    local_path,
                    "upload",
                    None,
                ),
                None,
            )
            .await
            {
                Ok(()) => return Ok(()),
                Err(e) => {
                    if matches!(e, AppError::Cancelled(_)) {
                        return Err(e);
                    }
                    last_err = Some(e);
                }
            }
        }
        Err(last_err.unwrap())
    }

    async fn download_directory(
        &self,
        app: &tauri::AppHandle,
        session_id: &str,
        remote_path: &str,
        local_path: &str,
        transfer_id: Option<String>,
    ) -> AppResult<()> {
        let transfer_settings = crate::config::load_app_settings(app)
            .map(|s| s.transfer)
            .unwrap_or_default();
        let (request_kib, pipeline_depth, max_concurrent_writes) =
            sftp_pipeline_config(&transfer_settings, self.pipeline_depth_override);
        let transfer_started = Instant::now();
        let directory_controller = create_directory_transfer_controller(
            transfer_id,
            session_id,
            file_name_from_path(remote_path),
            remote_path,
            local_path,
            "download",
            0,
            0,
        );
        register_transfer(directory_controller.clone());
        let _ = app.emit(
            "transfer-event",
            &directory_controller.build_event("started", 0, None),
        );

        let result = async {
            let inventory = self
                .collect_remote_directory_inventory(remote_path, local_path, &directory_controller)
                .await?;
            self.download_remote_directory_files(
                app,
                inventory,
                directory_controller.clone(),
                &transfer_settings,
            )
            .await
        }
        .await;

        match result {
            Ok(summary) => {
                log_transfer_performance(
                    "download",
                    "directory",
                    summary.bytes,
                    transfer_started.elapsed(),
                    request_kib,
                    pipeline_depth,
                    max_concurrent_writes,
                    summary.small_file_concurrency,
                );
                directory_controller.update_progress(summary.bytes, summary.bytes);
                directory_controller.update_item_progress(summary.completed, summary.total_files);
                let _ = app.emit(
                    "transfer-event",
                    &directory_controller.build_event("completed", 0, None),
                );
                unregister_transfer(&directory_controller.id());
                Ok(())
            }
            Err(e) => {
                if matches!(e, AppError::Cancelled(_)) {
                    let _ = app.emit(
                        "transfer-event",
                        &directory_controller.build_event("cancelled", 0, None),
                    );
                    cleanup_cancelled_download(local_path).await;
                } else {
                    let _ = app.emit(
                        "transfer-event",
                        &directory_controller.build_event("error", 0, Some(e.to_string())),
                    );
                }
                unregister_transfer(&directory_controller.id());
                Err(e)
            }
        }
    }

    async fn upload_directory(
        &self,
        app: &tauri::AppHandle,
        session_id: &str,
        local_path: &str,
        remote_path: &str,
        transfer_settings: &crate::config::TransferSettings,
        transfer_id: Option<String>,
    ) -> AppResult<()> {
        let session_manager = app.state::<Arc<crate::core::SessionManager>>();
        let sftp_for_check = self.open_sftp().await?;
        if !ensure_remote_upload_target_allowed(
            app,
            session_manager.inner(),
            &sftp_for_check,
            session_id,
            remote_path,
            &transfer_settings.duplicate_strategy,
        )
        .await
        {
            let _ = sftp_for_check.close().await;
            return Ok(());
        }
        let _ = sftp_for_check.close().await;

        let (request_kib, pipeline_depth, max_concurrent_writes) =
            sftp_pipeline_config(transfer_settings, self.pipeline_depth_override);
        let transfer_started = Instant::now();
        let directory_controller = create_directory_transfer_controller(
            transfer_id,
            session_id,
            file_name_from_path(local_path),
            remote_path,
            local_path,
            "upload",
            0,
            0,
        );
        register_transfer(directory_controller.clone());
        let _ = app.emit(
            "transfer-event",
            &directory_controller.build_event("started", 0, None),
        );

        let result = async {
            let inventory = self
                .collect_local_directory_inventory(
                    local_path,
                    remote_path,
                    &directory_controller,
                    transfer_settings,
                )
                .await?;
            self.upload_local_directory_files(
                app,
                inventory,
                &directory_controller,
                transfer_settings,
            )
            .await
        }
        .await;

        match result {
            Ok(summary) => {
                log_transfer_performance(
                    "upload",
                    "directory",
                    summary.bytes,
                    transfer_started.elapsed(),
                    request_kib,
                    pipeline_depth,
                    max_concurrent_writes,
                    summary.small_file_concurrency,
                );
                directory_controller.update_progress(summary.bytes, summary.bytes);
                directory_controller.update_item_progress(summary.completed, summary.total_files);
                let _ = app.emit(
                    "transfer-event",
                    &directory_controller.build_event("completed", 0, None),
                );
                unregister_transfer(&directory_controller.id());
                Ok(())
            }
            Err(e) => {
                if matches!(e, AppError::Cancelled(_)) {
                    let _ = app.emit(
                        "transfer-event",
                        &directory_controller.build_event("cancelled", 0, None),
                    );
                    let _ = cleanup_cancelled_upload(self, remote_path).await;
                } else {
                    let _ = app.emit(
                        "transfer-event",
                        &directory_controller.build_event("error", 0, Some(e.to_string())),
                    );
                }
                unregister_transfer(&directory_controller.id());
                Err(e)
            }
        }
    }

    async fn copy_remote_file_to_local_with_controller(
        &self,
        app: &tauri::AppHandle,
        session_id: &str,
        remote_path: &str,
        local_path: &str,
        transfer_settings: &crate::config::TransferSettings,
        controller: Arc<TransferController>,
        parent_controller: Option<Arc<TransferController>>,
    ) -> AppResult<u64> {
        download_remote_file_inner_with_controller(
            self,
            app,
            session_id,
            remote_path,
            local_path,
            transfer_settings,
            controller,
            parent_controller,
        )
        .await?;
        tokio::fs::metadata(local_path)
            .await
            .map(|metadata| metadata.len())
            .map_err(|error| AppError::Channel(format!("Failed to read copied file size: {error}")))
    }

    async fn copy_local_file_to_remote_with_controller(
        &self,
        app: &tauri::AppHandle,
        session_id: &str,
        local_path: &str,
        remote_path: &str,
        transfer_settings: &crate::config::TransferSettings,
        controller: Arc<TransferController>,
        parent_controller: Option<Arc<TransferController>>,
    ) -> AppResult<u64> {
        upload_local_file_inner_with_controller(
            self,
            app,
            session_id,
            local_path,
            remote_path,
            transfer_settings,
            controller,
            parent_controller,
        )
        .await?;
        tokio::fs::metadata(local_path)
            .await
            .map(|metadata| metadata.len())
            .map_err(|error| AppError::Channel(format!("Failed to read copied file size: {error}")))
    }
}
