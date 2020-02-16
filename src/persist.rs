//! A module that is responsible for storing and restoring the progress.
//!
//! The file format is very simple, it is represented as follows:
//!
//! | offset | size    | description
//! | 0      | 8       | the latest block number that was scanned.
//! | 8      | 3000000 | canvas 1000 x 1000 of RGB pixels. Each color component is represented by
//! |        |         | 8 bit, so a pixel is represented by 3 bytes.
//!
//! This format is a bit wasteful, it consumes 3Mb disk space. But it should be fine. The good news
//! is the pixels can be updated as a simple write operation.

use crate::command::{Chunk, CANVAS_HEIGHT, CANVAS_WIDTH};
use anyhow::Result;
use async_std::fs::{File, OpenOptions};
use async_std::io::{prelude::*, SeekFrom};
use async_std::path::Path;
use async_std::sync::{Arc, RwLock};

struct FileOps<'a>(&'a mut File);
impl<'a> FileOps<'a> {
    pub async fn read_block_num(&mut self) -> Result<u64> {
        self.0.seek(SeekFrom::Start(0u64)).await?;
        let mut buf = [0u8; 8];
        self.0.read_exact(&mut buf).await?;
        Ok(u64::from_le_bytes(buf))
    }

    pub async fn read_image_data(&mut self) -> Result<Vec<u8>> {
        self.0.seek(SeekFrom::Start(8u64)).await?;
        let mut buf = vec![0u8; 3 * CANVAS_HEIGHT * CANVAS_WIDTH];
        self.0.read_exact(&mut buf).await?;
        Ok(buf)
    }
}

struct Inner {
    file: File,
}

#[derive(Clone)]
pub struct Persister {
    inner: Arc<RwLock<Inner>>,
}

impl Persister {
    /// Asynchronously apply a sequence of commands and potentially flush the changes to the disk.
    pub async fn apply(&mut self, chunk: &Chunk) -> Result<()> {
        let mut inner = self.inner.write().await;

        inner.file.seek(SeekFrom::Start(0u64)).await?;
        {
            let block_num_buf = chunk.block_num.to_le_bytes();
            inner.file.write_all(&block_num_buf[..]).await?;
        }

        for cmd in &chunk.cmds {
            let idx = (cmd.y as usize * CANVAS_WIDTH + cmd.x as usize) * 3;
            inner.file.seek(SeekFrom::Start(8u64 + idx as u64)).await?;

            let rgb_buf = [cmd.rgb.0, cmd.rgb.1, cmd.rgb.2];
            inner.file.write_all(&rgb_buf[..]).await?;
        }

        inner.file.flush().await?;

        Ok(())
    }

    /// Returns the last persisted block number.
    pub async fn block_num(&mut self) -> Result<u64> {
        let mut inner = self.inner.write().await;
        FileOps(&mut inner.file).read_block_num().await
    }

    /// Returns the persisted image data.
    pub async fn image_data(&mut self) -> Result<Vec<u8>> {
        let mut inner = self.inner.write().await;
        FileOps(&mut inner.file).read_image_data().await
    }

    /// Shuts down the persister and awaits until all pending commands are stored.
    pub async fn shutdown(&self) -> Result<()> {
        let inner = self.inner.read().await;
        Ok(inner.file.sync_all().await?)
    }
}

pub async fn start(path: impl AsRef<Path>) -> Result<Persister> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .open(path)
        .await?;

    // Unconditionally set length of the file. It won't harm if the file is already initialized
    // and it necessary if it is not.
    let new_len = 8 + 3 * CANVAS_WIDTH * CANVAS_HEIGHT;
    file.set_len(new_len as u64).await?;

    let persister = Persister {
        inner: Arc::new(RwLock::new(Inner { file })),
    };
    Ok(persister)
}
