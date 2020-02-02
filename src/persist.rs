//! A module that is responsible for storing and restoring the progress.

use crate::command::{Chunk, CANVAS_HEIGHT, CANVAS_WIDTH};
use anyhow::{anyhow, Result};
use async_std::fs::{File, OpenOptions};
use async_std::io::{prelude::*, SeekFrom};
use async_std::path::Path;
use log::info;

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

pub struct PersistedData {
    pub block_num: u64,
    pub image_data: Vec<u8>,
}

impl PersistedData {
    async fn read(file: &mut File) -> Result<Self> {
        let mut ops = FileOps(file);
        Ok(Self {
            block_num: ops.read_block_num().await?,
            image_data: ops.read_image_data().await?,
        })
    }

    fn empty() -> Self {
        Self {
            block_num: 0,
            image_data: vec![0; 3 * CANVAS_WIDTH * CANVAS_HEIGHT],
        }
    }
}

pub struct Persister {
    file: File,
}

impl Persister {
    /// Asynchronously apply a sequence of commands and potentially flush the changes to the disk.
    pub async fn apply(&mut self, chunk: &Chunk) -> Result<()> {
        self.file.seek(SeekFrom::Start(0u64)).await?;
        {
            let block_num_buf = chunk.block_num.to_le_bytes();
            self.file.write_all(&block_num_buf[..]).await?;
        }

        for cmd in &chunk.cmds {
            let idx = (cmd.y as usize * CANVAS_WIDTH + cmd.x as usize) * 3;
            self.file.seek(SeekFrom::Start(8u64 + idx as u64)).await?;

            let rgb_buf = [cmd.rgb.0, cmd.rgb.1, cmd.rgb.2];
            self.file.write_all(&rgb_buf[..]).await?;
        }

        self.file.flush().await?;

        Ok(())
    }

    /// Shuts down the persister and awaits until all pending commands are stored.
    pub async fn shutdown(self) -> Result<()> {
        Ok(self.file.sync_all().await?)
    }
}

pub fn init(path: impl AsRef<Path>) -> Result<(PersistedData, Persister)> {
    async_std::task::block_on(async {
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .open(path)
            .await?;

        let data = match PersistedData::read(&mut file).await {
            Ok(data) => data,
            Err(e) => {
                info!("Cannot load persisted data: {}. Initializing new one", e);
                let new_len = 8 + 3 * CANVAS_WIDTH * CANVAS_HEIGHT;
                file.set_len(new_len as u64).await?;
                PersistedData::empty()
            }
        };

        let persister = Persister { file };

        Ok((data, persister))
    })
}
