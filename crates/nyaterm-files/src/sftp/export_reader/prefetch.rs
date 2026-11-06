use crate::sftp::SftpTransferControl;
use futures::{StreamExt as _, future::BoxFuture, stream::FuturesUnordered};
use std::collections::BTreeMap;

type RangeReader<'a> =
    Box<dyn Fn(u64, usize) -> BoxFuture<'a, anyhow::Result<Vec<u8>>> + Send + 'a>;
type BlockResult = (u64, anyhow::Result<Vec<u8>>);

/// The window counts both ready blocks and in-flight allocations. Futures are
/// owned here (not detached tasks), so resetting a seek or dropping the consumer
/// also drops all obsolete reads. An error stays attached to its block until used.
pub(super) struct Prefetch<'a> {
    reader: RangeReader<'a>,
    control: &'a SftpTransferControl,
    size: Option<u64>,
    block_size: usize,
    concurrency: usize,
    slots: usize,
    base: u64,
    blocks: BTreeMap<u64, Option<anyhow::Result<Vec<u8>>>>,
    pending: FuturesUnordered<BoxFuture<'a, BlockResult>>,
}

impl<'a> Prefetch<'a> {
    pub(super) fn new(
        reader: RangeReader<'a>,
        control: &'a SftpTransferControl,
        size: Option<u64>,
        block_size: usize,
        concurrency: usize,
    ) -> Self {
        Self {
            reader,
            control,
            size,
            block_size,
            concurrency: if size.is_some() { concurrency } else { 1 },
            slots: if size.is_some() { 2 * concurrency } else { 1 },
            base: 0,
            blocks: BTreeMap::new(),
            pending: FuturesUnordered::new(),
        }
    }

    fn position(&mut self, offset: u64) {
        let base = offset / self.block_size as u64 * self.block_size as u64;
        if !self.blocks.contains_key(&base) {
            self.pending.clear();
            self.blocks.clear();
        } else {
            // Keep obsolete in-flight slots charged until their reads finish.
            self.blocks
                .retain(|offset, data| *offset >= base || data.is_none());
        }
        self.base = base;
        self.schedule();
    }

    fn schedule(&mut self) {
        if self.control.is_paused() || self.control.is_cancelled() {
            return;
        }
        for index in 0..self.slots {
            if self.pending.len() >= self.concurrency || self.blocks.len() >= self.slots {
                break;
            }
            let Some(offset) = self.base.checked_add(index as u64 * self.block_size as u64) else {
                break;
            };
            if self.size.is_some_and(|size| offset >= size) {
                break;
            }
            if self.blocks.contains_key(&offset) {
                continue;
            }
            let length = self.size.map_or(self.block_size, |size| {
                (size - offset).min(self.block_size as u64) as usize
            });
            let future = (self.reader)(offset, length);
            self.pending
                .push(Box::pin(async move { (offset, future.await) }));
            self.blocks.insert(offset, None);
        }
    }

    /// Called in the request loop even when the native consumer is idle.
    pub(super) async fn advance(&mut self) {
        if self.control.wait_if_paused().await.is_err() {
            // The enclosing until_cancelled owns the cancellation result.
            std::future::pending::<()>().await;
        }
        self.schedule();
        if self.pending.is_empty() {
            std::future::pending::<()>().await;
        }
        if let Some((offset, result)) = self.pending.next().await {
            if offset < self.base {
                self.blocks.remove(&offset);
            } else {
                self.blocks.insert(offset, Some(result));
            }
        }
        self.schedule();
    }

    pub(super) fn has_pending(&self) -> bool {
        !self.pending.is_empty()
    }

    pub(super) async fn read(&mut self, offset: u64, length: usize) -> anyhow::Result<Vec<u8>> {
        let mut output = Vec::with_capacity(length);
        while output.len() < length {
            self.control.wait_if_paused().await?;
            let cursor = offset
                .checked_add(output.len() as u64)
                .ok_or_else(|| anyhow::anyhow!("invalid export range"))?;
            if self.size.is_some_and(|size| cursor >= size) {
                break;
            }
            self.position(cursor);
            while self.blocks.get(&self.base).is_none_or(Option::is_none) {
                let control = self.control;
                control.until_cancelled(self.advance()).await?;
                self.control.wait_if_paused().await?;
                self.schedule();
            }
            // A cached reply must obey pause/cancellation just like a network read.
            self.control.wait_if_paused().await?;
            let Some(Some(block)) = self.blocks.get_mut(&self.base) else {
                unreachable!();
            };
            if block.is_err() {
                // The worker terminates on a consumed failure. Retain its typed
                // cause so compatibility timeout/closed-session eviction works.
                return Err(std::mem::replace(block, Ok(Vec::new())).unwrap_err());
            }
            let data = block.as_ref().unwrap();
            let start = (cursor - self.base) as usize;
            if start >= data.len() {
                break;
            }
            let count = (length - output.len()).min(data.len() - start);
            output.extend_from_slice(&data[start..start + count]);
            if data.len() < self.block_size && self.size.is_none() {
                break;
            }
        }
        Ok(output)
    }
}

#[cfg(test)]
mod tests;
