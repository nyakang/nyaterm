use bytes::Bytes;
use dashmap::DashMap as HashMap;
use std::{
    future::Future,
    pin::Pin,
    sync::{
        atomic::{AtomicU32, AtomicU64, Ordering},
        Arc,
    },
    task::{Context, Poll},
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncWrite},
    sync::{mpsc, oneshot},
    time::Sleep,
};

use super::{error::Error, Handler};
use crate::{
    client::{run, Config},
    de,
    extensions::{
        self, FsyncExtension, HardlinkExtension, LimitsExtension, Statvfs, StatvfsExtension,
    },
    protocol::{
        Attrs, Close, Data, Extended, ExtendedReply, FSetStat, FileAttributes, Fstat, Handle, Init,
        Lstat, MkDir, Name, Open, OpenDir, OpenFlags, Packet, Read, ReadDir, ReadLink, RealPath,
        Remove, Rename, RmDir, SetStat, Stat, Status, StatusCode, Symlink, Version, Write,
    },
};

pub type SftpResult<T> = Result<T, Error>;
type SharedRequests = HashMap<Option<u32>, oneshot::Sender<SftpResult<Packet>>>;

pub(crate) struct SessionInner {
    version: Option<u32>,
    requests: Arc<SharedRequests>,
}

impl SessionInner {
    pub fn reply(&mut self, id: Option<u32>, packet: Packet) -> SftpResult<()> {
        if let Some((_, sender)) = self.requests.remove(&id) {
            let validate = if id.is_some() && self.version.is_none() {
                Err(Error::UnexpectedPacket)
            } else if id.is_none() && self.version.is_some() {
                Err(Error::UnexpectedBehavior("Duplicate version".to_owned()))
            } else {
                Ok(())
            };

            // Ignore send error: receiver was dropped (request timed out).
            let _ = sender.send(validate.clone().map(|_| packet));

            return validate;
        }

        warn!("Packet {:?} for unknown recipient", id);
        Ok(())
    }
}

pub(crate) struct PendingRequest {
    id: Option<u32>,
    requests: Arc<SharedRequests>,
    receiver: oneshot::Receiver<SftpResult<Packet>>,
    timeout: Pin<Box<Sleep>>,
    completed: bool,
}

impl PendingRequest {
    fn new(
        id: Option<u32>,
        receiver: oneshot::Receiver<SftpResult<Packet>>,
        timeout: Duration,
        requests: Arc<SharedRequests>,
    ) -> Self {
        Self {
            id,
            requests,
            receiver,
            timeout: Box::pin(tokio::time::sleep(timeout)),
            completed: false,
        }
    }
}

impl Future for PendingRequest {
    type Output = SftpResult<Packet>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        match Pin::new(&mut self.receiver).poll(cx) {
            Poll::Ready(Ok(result)) => {
                self.completed = true;
                return Poll::Ready(result);
            }
            Poll::Ready(Err(_)) => {
                self.completed = true;
                return Poll::Ready(Err(Error::UnexpectedBehavior("sender dropped".into())));
            }
            Poll::Pending => {}
        }

        match self.timeout.as_mut().poll(cx) {
            Poll::Ready(()) => {
                self.completed = true;
                self.requests.remove(&self.id);
                Poll::Ready(Err(Error::Timeout))
            }
            Poll::Pending => Poll::Pending,
        }
    }
}

impl Drop for PendingRequest {
    fn drop(&mut self) {
        if !self.completed {
            self.requests.remove(&self.id);
        }
    }
}

fn fail_all_pending_requests(requests: &SharedRequests, error: Error) {
    let ids = requests
        .iter()
        .map(|entry| *entry.key())
        .collect::<Vec<_>>();

    for id in ids {
        if let Some((_, sender)) = requests.remove(&id) {
            let _ = sender.send(Err(error.clone()));
        }
    }
}

impl Handler for SessionInner {
    type Error = Error;

    async fn version(&mut self, packet: Version) -> Result<(), Self::Error> {
        let version = packet.version;
        self.reply(None, packet.into())?;
        self.version = Some(version);
        Ok(())
    }

    async fn name(&mut self, name: Name) -> Result<(), Self::Error> {
        self.reply(Some(name.id), name.into())
    }

    async fn status(&mut self, status: Status) -> Result<(), Self::Error> {
        self.reply(Some(status.id), status.into())
    }

    async fn handle(&mut self, handle: Handle) -> Result<(), Self::Error> {
        self.reply(Some(handle.id), handle.into())
    }

    async fn data(&mut self, data: Data) -> Result<(), Self::Error> {
        self.reply(Some(data.id), data.into())
    }

    async fn attrs(&mut self, attrs: Attrs) -> Result<(), Self::Error> {
        self.reply(Some(attrs.id), attrs.into())
    }

    async fn extended_reply(&mut self, reply: ExtendedReply) -> Result<(), Self::Error> {
        self.reply(Some(reply.id), reply.into())
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Limits {
    pub packet_len: Option<u64>,
    pub read_len: Option<u64>,
    pub write_len: Option<u64>,
    pub open_handles: Option<u64>,
}

impl From<LimitsExtension> for Limits {
    fn from(limits: LimitsExtension) -> Self {
        Self {
            packet_len: (limits.max_packet_len > 0).then_some(limits.max_packet_len),
            read_len: (limits.max_read_len > 0).then_some(limits.max_read_len),
            write_len: (limits.max_write_len > 0).then_some(limits.max_write_len),
            open_handles: (limits.max_open_handles > 0).then_some(limits.max_open_handles),
        }
    }
}

/// Implements raw work with the protocol in request-response format.
/// If the server returns a `Status` packet and it has the code Ok
/// then the packet is returned as Ok in other error cases
/// the packet is stored as Err.
pub struct RawSftpSession {
    tx: mpsc::UnboundedSender<Bytes>,
    requests: Arc<SharedRequests>,
    next_req_id: AtomicU32,
    handles: AtomicU64,
    timeout: AtomicU64,
    limits: Limits,
}

macro_rules! into_with_status {
    ($result:ident, $packet:ident) => {
        match $result {
            Packet::$packet(p) => Ok(p),
            Packet::Status(p) => Err(p.into()),
            _ => Err(Error::UnexpectedPacket),
        }
    };
}

macro_rules! into_status {
    ($result:ident) => {
        match $result {
            Packet::Status(status) if status.status_code == StatusCode::Ok => Ok(status),
            Packet::Status(status) => Err(status.into()),
            _ => Err(Error::UnexpectedPacket),
        }
    };
}

impl RawSftpSession {
    pub fn new<S>(stream: S) -> Self
    where
        S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    {
        Self::new_with_config(stream, Config::default())
    }

    pub fn new_with_config<S>(stream: S, cfg: Config) -> Self
    where
        S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    {
        let req_map = Arc::new(HashMap::new());
        let inner = SessionInner {
            version: None,
            requests: req_map.clone(),
        };

        let fail_requests = req_map.clone();

        Self {
            tx: run(
                stream,
                inner,
                Arc::new(move |error| fail_all_pending_requests(&fail_requests, error)),
            ),
            requests: req_map,
            next_req_id: AtomicU32::new(1),
            handles: AtomicU64::new(0),
            timeout: AtomicU64::new(cfg.request_timeout_secs),
            limits: Limits::default(),
        }
    }

    /// Set the maximum response time in seconds.
    /// Default: 10 seconds
    pub fn set_timeout(&self, secs: u64) {
        self.timeout.store(secs, Ordering::Relaxed);
    }

    /// Setting limits. For the `limits@openssh.com` extension
    pub fn set_limits(&mut self, limits: Limits) {
        self.limits = limits;
    }

    fn send(&self, id: Option<u32>, packet: Packet) -> SftpResult<PendingRequest> {
        if self.tx.is_closed() {
            return Err(Error::UnexpectedBehavior("session closed".into()));
        }

        let bytes = Bytes::try_from(packet)?;

        if let Some(max_len) = self.limits.packet_len {
            if bytes.len() as u64 > max_len {
                return Err(Error::Limited("packet exceeds server limit".to_owned()));
            }
        }

        let (tx, rx) = oneshot::channel();
        self.requests.insert(id, tx);
        if let Err(error) = self.tx.send(bytes) {
            self.requests.remove(&id);
            return Err(error.into());
        }

        Ok(PendingRequest::new(
            id,
            rx,
            Duration::from_secs(self.timeout.load(Ordering::Relaxed)),
            self.requests.clone(),
        ))
    }

    async fn request(&self, id: Option<u32>, packet: Packet) -> SftpResult<Packet> {
        self.send(id, packet)?.await
    }

    fn use_next_id(&self) -> u32 {
        self.next_req_id.fetch_add(1, Ordering::Relaxed)
    }

    fn release_handle(&self) {
        if self
            .handles
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |count| {
                count.checked_sub(1)
            })
            .is_err()
        {
            warn!("attempt to release more SFTP handles than exist");
        }
    }

    #[cfg(test)]
    pub(crate) fn open_handle_count(&self) -> u64 {
        self.handles.load(Ordering::SeqCst)
    }

    /// Closes the inner channel stream. Called by [`Drop`]
    pub fn close_session(&self) -> SftpResult<()> {
        if self.tx.is_closed() {
            return Ok(());
        }

        Ok(self.tx.send(Bytes::new())?)
    }

    pub async fn init(&self) -> SftpResult<Version> {
        let result = self.request(None, Init::default().into()).await?;
        if let Packet::Version(version) = result {
            Ok(version)
        } else {
            Err(Error::UnexpectedPacket)
        }
    }

    pub async fn open<T: Into<String>>(
        &self,
        filename: T,
        flags: OpenFlags,
        attrs: FileAttributes,
    ) -> SftpResult<Handle> {
        if self
            .limits
            .open_handles
            .is_some_and(|h| self.handles.load(Ordering::SeqCst) >= h)
        {
            return Err(Error::Limited("handle limit reached".to_owned()));
        }

        let id = self.use_next_id();
        let result = self
            .request(
                Some(id),
                Open {
                    id,
                    filename: filename.into(),
                    filename_bytes: None,
                    pflags: flags,
                    attrs,
                }
                .into(),
            )
            .await?;

        if let Packet::Handle(_) = result {
            self.handles.fetch_add(1, Ordering::SeqCst);
        }

        into_with_status!(result, Handle)
    }

    /// Opens a file using raw bytes for the filename (preserves original encoding).
    pub async fn open_bytes(
        &self,
        filename_bytes: Vec<u8>,
        flags: OpenFlags,
        attrs: FileAttributes,
    ) -> SftpResult<Handle> {
        if self
            .limits
            .open_handles
            .is_some_and(|h| self.handles.load(Ordering::SeqCst) >= h)
        {
            return Err(Error::Limited("handle limit reached".to_owned()));
        }

        let id = self.use_next_id();
        let filename = String::from_utf8_lossy(&filename_bytes).into_owned();
        let result = self
            .request(
                Some(id),
                Open {
                    id,
                    filename,
                    filename_bytes: Some(filename_bytes),
                    pflags: flags,
                    attrs,
                }
                .into(),
            )
            .await?;

        if let Packet::Handle(_) = result {
            self.handles.fetch_add(1, Ordering::SeqCst);
        }

        into_with_status!(result, Handle)
    }

    pub async fn close<H: Into<String>>(&self, handle: H) -> SftpResult<Status> {
        let id = self.use_next_id();
        let result = self
            .request(
                Some(id),
                Close {
                    id,
                    handle: handle.into(),
                }
                .into(),
            )
            .await;

        self.release_handle();

        let result = result?;
        into_status!(result)
    }

    /// Sends a close packet in the background while keeping the request tracked.
    pub(crate) fn close_detached(self: &Arc<Self>, handle: String) {
        let session = Arc::clone(self);

        crate::client::runtime::spawn(async move {
            if let Err(error) = session.close(handle).await {
                trace!("detached SFTP handle close failed: {error}");
            }
        });
    }

    pub async fn read<H: Into<String>>(
        &self,
        handle: H,
        offset: u64,
        len: u32,
    ) -> SftpResult<Data> {
        if self.limits.read_len.is_some_and(|r| len as u64 > r) {
            return Err(Error::Limited("read limit reached".to_owned()));
        }

        let id = self.use_next_id();
        let result = self
            .request(
                Some(id),
                Read {
                    id,
                    handle: handle.into(),
                    offset,
                    len,
                }
                .into(),
            )
            .await?;

        into_with_status!(result, Data)
    }

    pub async fn write<H: Into<String>>(
        &self,
        handle: H,
        offset: u64,
        data: Vec<u8>,
    ) -> SftpResult<Status> {
        if self.limits.write_len.is_some_and(|w| data.len() as u64 > w) {
            return Err(Error::Limited("write limit reached".to_owned()));
        }

        let id = self.use_next_id();
        let result = self
            .request(
                Some(id),
                Write {
                    id,
                    handle: handle.into(),
                    offset,
                    data,
                }
                .into(),
            )
            .await?;

        into_status!(result)
    }

    /// Sends a write packet without awaiting the server's acknowledgement.
    pub(crate) fn write_nowait(
        &self,
        handle: String,
        offset: u64,
        data: Vec<u8>,
    ) -> SftpResult<PendingRequest> {
        if self.limits.write_len.is_some_and(|w| data.len() as u64 > w) {
            return Err(Error::Limited("write limit reached".to_owned()));
        }

        let id = self.use_next_id();
        self.send(
            Some(id),
            Write {
                id,
                handle,
                offset,
                data,
            }
            .into(),
        )
    }

    pub async fn lstat<P: Into<String>>(&self, path: P) -> SftpResult<Attrs> {
        let id = self.use_next_id();
        let result = self
            .request(
                Some(id),
                Lstat {
                    id,
                    path: path.into(),
                    path_bytes: None,
                }
                .into(),
            )
            .await?;

        into_with_status!(result, Attrs)
    }

    pub async fn lstat_bytes(&self, path_bytes: Vec<u8>) -> SftpResult<Attrs> {
        let id = self.use_next_id();
        let path = String::from_utf8_lossy(&path_bytes).into_owned();
        let result = self
            .request(
                Some(id),
                Lstat {
                    id,
                    path,
                    path_bytes: Some(path_bytes),
                }
                .into(),
            )
            .await?;

        into_with_status!(result, Attrs)
    }

    pub async fn fstat<H: Into<String>>(&self, handle: H) -> SftpResult<Attrs> {
        let id = self.use_next_id();
        let result = self
            .request(
                Some(id),
                Fstat {
                    id,
                    handle: handle.into(),
                }
                .into(),
            )
            .await?;

        into_with_status!(result, Attrs)
    }

    pub async fn setstat<P: Into<String>>(
        &self,
        path: P,
        attrs: FileAttributes,
    ) -> SftpResult<Status> {
        let id = self.use_next_id();
        let result = self
            .request(
                Some(id),
                SetStat {
                    id,
                    path: path.into(),
                    path_bytes: None,
                    attrs,
                }
                .into(),
            )
            .await?;

        into_status!(result)
    }

    pub async fn setstat_bytes(
        &self,
        path_bytes: Vec<u8>,
        attrs: FileAttributes,
    ) -> SftpResult<Status> {
        let id = self.use_next_id();
        let path = String::from_utf8_lossy(&path_bytes).into_owned();
        let result = self
            .request(
                Some(id),
                SetStat {
                    id,
                    path,
                    path_bytes: Some(path_bytes),
                    attrs,
                }
                .into(),
            )
            .await?;

        into_status!(result)
    }

    pub async fn fsetstat<H: Into<String>>(
        &self,
        handle: H,
        attrs: FileAttributes,
    ) -> SftpResult<Status> {
        let id = self.use_next_id();
        let result = self
            .request(
                Some(id),
                FSetStat {
                    id,
                    handle: handle.into(),
                    attrs,
                }
                .into(),
            )
            .await?;

        into_status!(result)
    }

    pub async fn opendir<P: Into<String>>(&self, path: P) -> SftpResult<Handle> {
        if self
            .limits
            .open_handles
            .is_some_and(|h| self.handles.load(Ordering::SeqCst) >= h)
        {
            return Err(Error::Limited("Handle limit reached".to_owned()));
        }

        let id = self.use_next_id();
        let result = self
            .request(
                Some(id),
                OpenDir {
                    id,
                    path: path.into(),
                    path_bytes: None,
                }
                .into(),
            )
            .await?;

        if let Packet::Handle(_) = result {
            self.handles.fetch_add(1, Ordering::SeqCst);
        }

        into_with_status!(result, Handle)
    }

    /// Opens a directory using raw bytes for the path (preserves original encoding).
    pub async fn opendir_bytes(&self, path_bytes: Vec<u8>) -> SftpResult<Handle> {
        if self
            .limits
            .open_handles
            .is_some_and(|h| self.handles.load(Ordering::SeqCst) >= h)
        {
            return Err(Error::Limited("Handle limit reached".to_owned()));
        }

        let id = self.use_next_id();
        let path = String::from_utf8_lossy(&path_bytes).into_owned();
        let result = self
            .request(
                Some(id),
                OpenDir {
                    id,
                    path,
                    path_bytes: Some(path_bytes),
                }
                .into(),
            )
            .await?;

        if let Packet::Handle(_) = result {
            self.handles.fetch_add(1, Ordering::SeqCst);
        }

        into_with_status!(result, Handle)
    }

    pub async fn readdir<H: Into<String>>(&self, handle: H) -> SftpResult<Name> {
        let id = self.use_next_id();
        let result = self
            .request(
                Some(id),
                ReadDir {
                    id,
                    handle: handle.into(),
                }
                .into(),
            )
            .await?;

        into_with_status!(result, Name)
    }

    pub async fn remove<T: Into<String>>(&self, filename: T) -> SftpResult<Status> {
        let id = self.use_next_id();
        let result = self
            .request(
                Some(id),
                Remove {
                    id,
                    filename: filename.into(),
                    filename_bytes: None,
                }
                .into(),
            )
            .await?;

        into_status!(result)
    }

    /// Removes a file using raw bytes for the filename (preserves original encoding).
    pub async fn remove_bytes(&self, filename_bytes: Vec<u8>) -> SftpResult<Status> {
        let id = self.use_next_id();
        let filename = String::from_utf8_lossy(&filename_bytes).into_owned();
        let result = self
            .request(
                Some(id),
                Remove {
                    id,
                    filename,
                    filename_bytes: Some(filename_bytes),
                }
                .into(),
            )
            .await?;

        into_status!(result)
    }

    pub async fn mkdir<P: Into<String>>(
        &self,
        path: P,
        attrs: FileAttributes,
    ) -> SftpResult<Status> {
        let id = self.use_next_id();
        let result = self
            .request(
                Some(id),
                MkDir {
                    id,
                    path: path.into(),
                    path_bytes: None,
                    attrs,
                }
                .into(),
            )
            .await?;

        into_status!(result)
    }

    pub async fn mkdir_bytes(
        &self,
        path_bytes: Vec<u8>,
        attrs: FileAttributes,
    ) -> SftpResult<Status> {
        let id = self.use_next_id();
        let path = String::from_utf8_lossy(&path_bytes).into_owned();
        let result = self
            .request(
                Some(id),
                MkDir {
                    id,
                    path,
                    path_bytes: Some(path_bytes),
                    attrs,
                }
                .into(),
            )
            .await?;

        into_status!(result)
    }

    pub async fn rmdir<P: Into<String>>(&self, path: P) -> SftpResult<Status> {
        let id = self.use_next_id();
        let result = self
            .request(
                Some(id),
                RmDir {
                    id,
                    path: path.into(),
                    path_bytes: None,
                }
                .into(),
            )
            .await?;

        into_status!(result)
    }

    pub async fn rmdir_bytes(&self, path_bytes: Vec<u8>) -> SftpResult<Status> {
        let id = self.use_next_id();
        let path = String::from_utf8_lossy(&path_bytes).into_owned();
        let result = self
            .request(
                Some(id),
                RmDir {
                    id,
                    path,
                    path_bytes: Some(path_bytes),
                }
                .into(),
            )
            .await?;

        into_status!(result)
    }

    pub async fn realpath<P: Into<String>>(&self, path: P) -> SftpResult<Name> {
        let id = self.use_next_id();
        let result = self
            .request(
                Some(id),
                RealPath {
                    id,
                    path: path.into(),
                }
                .into(),
            )
            .await?;

        into_with_status!(result, Name)
    }

    pub async fn stat<P: Into<String>>(&self, path: P) -> SftpResult<Attrs> {
        let id = self.use_next_id();
        let result = self
            .request(
                Some(id),
                Stat {
                    id,
                    path: path.into(),
                    path_bytes: None,
                }
                .into(),
            )
            .await?;

        into_with_status!(result, Attrs)
    }

    /// Queries metadata about the remote file using raw bytes (preserves original encoding).
    pub async fn stat_bytes(&self, path_bytes: Vec<u8>) -> SftpResult<Attrs> {
        let id = self.use_next_id();
        let path = String::from_utf8_lossy(&path_bytes).into_owned();
        let result = self
            .request(
                Some(id),
                Stat {
                    id,
                    path,
                    path_bytes: Some(path_bytes),
                }
                .into(),
            )
            .await?;

        into_with_status!(result, Attrs)
    }

    pub async fn rename<O, N>(&self, oldpath: O, newpath: N) -> SftpResult<Status>
    where
        O: Into<String>,
        N: Into<String>,
    {
        let id = self.use_next_id();
        let result = self
            .request(
                Some(id),
                Rename {
                    id,
                    oldpath: oldpath.into(),
                    newpath: newpath.into(),
                    oldpath_bytes: None,
                    newpath_bytes: None,
                }
                .into(),
            )
            .await?;

        into_status!(result)
    }

    pub async fn rename_bytes(
        &self,
        oldpath_bytes: Vec<u8>,
        newpath_bytes: Vec<u8>,
    ) -> SftpResult<Status> {
        let id = self.use_next_id();
        let oldpath = String::from_utf8_lossy(&oldpath_bytes).into_owned();
        let newpath = String::from_utf8_lossy(&newpath_bytes).into_owned();
        let result = self
            .request(
                Some(id),
                Rename {
                    id,
                    oldpath,
                    newpath,
                    oldpath_bytes: Some(oldpath_bytes),
                    newpath_bytes: Some(newpath_bytes),
                }
                .into(),
            )
            .await?;

        into_status!(result)
    }

    pub async fn readlink<P: Into<String>>(&self, path: P) -> SftpResult<Name> {
        let id = self.use_next_id();
        let result = self
            .request(
                Some(id),
                ReadLink {
                    id,
                    path: path.into(),
                    path_bytes: None,
                }
                .into(),
            )
            .await?;

        into_with_status!(result, Name)
    }

    pub async fn readlink_bytes(&self, path_bytes: Vec<u8>) -> SftpResult<Name> {
        let id = self.use_next_id();
        let path = String::from_utf8_lossy(&path_bytes).into_owned();
        let result = self
            .request(
                Some(id),
                ReadLink {
                    id,
                    path,
                    path_bytes: Some(path_bytes),
                }
                .into(),
            )
            .await?;

        into_with_status!(result, Name)
    }

    pub async fn symlink<P, T>(&self, path: P, target: T) -> SftpResult<Status>
    where
        P: Into<String>,
        T: Into<String>,
    {
        let id = self.use_next_id();
        let result = self
            .request(
                Some(id),
                Symlink {
                    id,
                    linkpath: path.into(),
                    targetpath: target.into(),
                    linkpath_bytes: None,
                    targetpath_bytes: None,
                }
                .into(),
            )
            .await?;

        into_status!(result)
    }

    pub async fn symlink_openssh<T, L>(&self, target: T, link: L) -> SftpResult<Status>
    where
        T: Into<String>,
        L: Into<String>,
    {
        let id = self.use_next_id();
        let result = self
            .request(
                Some(id),
                Symlink {
                    id,
                    linkpath: target.into(),
                    targetpath: link.into(),
                    linkpath_bytes: None,
                    targetpath_bytes: None,
                }
                .into(),
            )
            .await?;

        into_status!(result)
    }

    pub async fn symlink_openssh_bytes(
        &self,
        target_bytes: Vec<u8>,
        link_bytes: Vec<u8>,
    ) -> SftpResult<Status> {
        let id = self.use_next_id();
        let linkpath = String::from_utf8_lossy(&target_bytes).into_owned();
        let targetpath = String::from_utf8_lossy(&link_bytes).into_owned();
        let result = self
            .request(
                Some(id),
                Symlink {
                    id,
                    linkpath,
                    targetpath,
                    linkpath_bytes: Some(target_bytes),
                    targetpath_bytes: Some(link_bytes),
                }
                .into(),
            )
            .await?;

        into_status!(result)
    }

    /// Equivalent to `SSH_FXP_EXTENDED`. Allows protocol expansion.
    /// The extension can return any packet, so it's not specific
    pub async fn extended<R: Into<String>>(&self, request: R, data: Vec<u8>) -> SftpResult<Packet> {
        let id = self.use_next_id();
        self.request(
            Some(id),
            Extended {
                id,
                request: request.into(),
                data,
            }
            .into(),
        )
        .await
    }

    pub async fn limits(&self) -> SftpResult<LimitsExtension> {
        match self.extended(extensions::LIMITS, vec![]).await? {
            Packet::ExtendedReply(reply) => {
                Ok(de::from_bytes::<LimitsExtension>(&mut reply.data.into())?)
            }
            Packet::Status(status) if status.status_code != StatusCode::Ok => {
                Err(Error::Status(status))
            }
            _ => Err(Error::UnexpectedPacket),
        }
    }

    pub async fn hardlink<O, N>(&self, oldpath: O, newpath: N) -> SftpResult<Status>
    where
        O: Into<String>,
        N: Into<String>,
    {
        let result = self
            .extended(
                extensions::HARDLINK,
                HardlinkExtension {
                    oldpath: oldpath.into(),
                    newpath: newpath.into(),
                }
                .try_into()?,
            )
            .await?;

        into_status!(result)
    }

    pub async fn fsync<H: Into<String>>(&self, handle: H) -> SftpResult<Status> {
        let result = self
            .extended(
                extensions::FSYNC,
                FsyncExtension {
                    handle: handle.into(),
                }
                .try_into()?,
            )
            .await?;

        into_status!(result)
    }

    pub async fn statvfs<P>(&self, path: P) -> SftpResult<Statvfs>
    where
        P: Into<String>,
    {
        let result = self
            .extended(
                extensions::STATVFS,
                StatvfsExtension { path: path.into() }.try_into()?,
            )
            .await?;

        match result {
            Packet::ExtendedReply(reply) => Ok(de::from_bytes::<Statvfs>(&mut reply.data.into())?),
            Packet::Status(status) if status.status_code != StatusCode::Ok => {
                Err(Error::Status(status))
            }
            _ => Err(Error::UnexpectedPacket),
        }
    }
}

impl Drop for RawSftpSession {
    fn drop(&mut self) {
        let _ = self.close_session();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncWriteExt, DuplexStream};

    async fn read_client_packet(server: &mut DuplexStream) -> Packet {
        let mut bytes = crate::utils::read_packet(server, u32::MAX)
            .await
            .expect("server should read client packet");
        Packet::try_from(&mut bytes).expect("client packet should decode")
    }

    async fn write_server_packet(server: &mut DuplexStream, packet: Packet) {
        let bytes = Bytes::try_from(packet).expect("server packet should encode");
        server
            .write_all(&bytes)
            .await
            .expect("server should write packet");
    }

    async fn initialize_session(session: &RawSftpSession, server: &mut DuplexStream) {
        let client = session.init();
        let server = async {
            match read_client_packet(server).await {
                Packet::Init(_) => {}
                packet => panic!("expected init packet, got {packet:?}"),
            }
            write_server_packet(server, Version::new().into()).await;
        };

        let (client_result, _) = tokio::join!(client, server);
        client_result.expect("session should initialize");
    }

    async fn open_test_handle(session: &RawSftpSession, server: &mut DuplexStream) -> String {
        let client = session.open("remote.txt", OpenFlags::READ, FileAttributes::empty());
        let server = async {
            let id = match read_client_packet(server).await {
                Packet::Open(open) => open.id,
                packet => panic!("expected open packet, got {packet:?}"),
            };
            write_server_packet(
                server,
                Handle {
                    id,
                    handle: "handle-1".to_string(),
                }
                .into(),
            )
            .await;
        };

        let (client_result, _) = tokio::join!(client, server);
        client_result.expect("open should succeed").handle
    }

    async fn close_test_handle(
        session: &RawSftpSession,
        server: &mut DuplexStream,
        handle: String,
        status_code: StatusCode,
    ) -> SftpResult<Status> {
        let client = session.close(handle);
        let server = async {
            let id = match read_client_packet(server).await {
                Packet::Close(close) => close.id,
                packet => panic!("expected close packet, got {packet:?}"),
            };
            write_server_packet(
                server,
                Packet::status(id, status_code, &status_code.to_string(), "en-US"),
            )
            .await;
        };

        let (client_result, _) = tokio::join!(client, server);
        client_result
    }

    fn test_config(request_timeout_secs: u64) -> Config {
        Config {
            request_timeout_secs,
            ..Config::default()
        }
    }

    #[tokio::test]
    async fn write_nowait_times_out_and_cleans_pending_request() {
        let (client, _server) = tokio::io::duplex(4096);
        let session = RawSftpSession::new_with_config(client, test_config(0));
        let pending = session
            .write_nowait("handle".to_string(), 0, b"payload".to_vec())
            .expect("write request should be queued");

        assert_eq!(session.requests.len(), 1);

        let error = pending.await.expect_err("write ack should time out");

        assert!(matches!(error, Error::Timeout));
        assert!(session.requests.is_empty());
    }

    #[tokio::test]
    async fn dropped_pending_request_cleans_pending_map() {
        let (client, _server) = tokio::io::duplex(4096);
        let session = RawSftpSession::new_with_config(client, test_config(10));
        let pending = session
            .write_nowait("handle".to_string(), 0, b"payload".to_vec())
            .expect("write request should be queued");

        assert_eq!(session.requests.len(), 1);
        drop(pending);

        assert!(session.requests.is_empty());
    }

    #[tokio::test]
    async fn stream_write_failure_wakes_pending_request() {
        let (client, server) = tokio::io::duplex(64);
        drop(server);
        let session = RawSftpSession::new_with_config(client, test_config(10));
        let pending = session
            .write_nowait("handle".to_string(), 0, vec![0; 1024])
            .expect("write request should be queued before write task observes closure");

        let error = tokio::time::timeout(Duration::from_secs(1), pending)
            .await
            .expect("pending request should be failed promptly")
            .expect_err("stream write failure should fail the request");

        assert!(matches!(error, Error::IO(_) | Error::UnexpectedBehavior(_)));
        assert!(session.requests.is_empty());
    }

    #[tokio::test]
    async fn open_success_increments_handle_count() {
        let (client, mut server) = tokio::io::duplex(4096);
        let session = RawSftpSession::new_with_config(client, test_config(10));
        initialize_session(&session, &mut server).await;

        let _handle = open_test_handle(&session, &mut server).await;

        assert_eq!(session.open_handle_count(), 1);
    }

    #[tokio::test]
    async fn close_success_decrements_handle_count() {
        let (client, mut server) = tokio::io::duplex(4096);
        let session = RawSftpSession::new_with_config(client, test_config(10));
        initialize_session(&session, &mut server).await;
        let handle = open_test_handle(&session, &mut server).await;

        close_test_handle(&session, &mut server, handle, StatusCode::Ok)
            .await
            .expect("close should succeed");

        assert_eq!(session.open_handle_count(), 0);
    }

    #[tokio::test]
    async fn close_status_error_decrements_handle_count() {
        let (client, mut server) = tokio::io::duplex(4096);
        let session = RawSftpSession::new_with_config(client, test_config(10));
        initialize_session(&session, &mut server).await;
        let handle = open_test_handle(&session, &mut server).await;

        let error = close_test_handle(&session, &mut server, handle, StatusCode::Failure)
            .await
            .expect_err("close status failure should be returned");

        assert!(matches!(
            error,
            Error::Status(status) if status.status_code == StatusCode::Failure
        ));
        assert_eq!(session.open_handle_count(), 0);
    }

    #[tokio::test]
    async fn close_timeout_decrements_handle_count() {
        let (client, mut server) = tokio::io::duplex(4096);
        let session = RawSftpSession::new_with_config(client, test_config(10));
        initialize_session(&session, &mut server).await;
        let handle = open_test_handle(&session, &mut server).await;
        session.set_timeout(0);

        let error = session
            .close(handle)
            .await
            .expect_err("close should time out");

        assert!(matches!(error, Error::Timeout));
        assert_eq!(session.open_handle_count(), 0);
    }

    #[tokio::test]
    async fn close_stream_failure_decrements_handle_count() {
        let (client, mut server) = tokio::io::duplex(4096);
        let session = RawSftpSession::new_with_config(client, test_config(10));
        initialize_session(&session, &mut server).await;
        let handle = open_test_handle(&session, &mut server).await;
        drop(server);

        let error = session
            .close(handle)
            .await
            .expect_err("close should fail after stream closes");

        assert!(matches!(error, Error::IO(_) | Error::UnexpectedBehavior(_)));
        assert_eq!(session.open_handle_count(), 0);
    }

    #[tokio::test]
    async fn repeated_release_does_not_underflow() {
        let (client, _server) = tokio::io::duplex(4096);
        let session = RawSftpSession::new_with_config(client, test_config(10));

        session.release_handle();
        session.release_handle();

        assert_eq!(session.open_handle_count(), 0);
    }
}
