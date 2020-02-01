//! A type that represents a command for this remarkable app.

use std::fmt::Write;

// TODO: Constrain the size of the canvas.

pub const CANVAS_WIDTH: usize = 1000;
pub const CANVAS_HEIGHT: usize = 1000;

#[derive(Debug)]
pub struct Command {
    pub x: u16,
    pub y: u16,
    pub rgb: (u8, u8, u8),
}

impl Command {
    pub fn parse(data: &[u8]) -> Option<Self> {
        const MAGIC: &[u8] = &[0x13, 0x37];
        if !data.starts_with(MAGIC) {
            return None;
        }
        if data.len() != 8 {
            return None;
        }

        let mut coord_buf = String::with_capacity(6);
        for x in &data[2..5] {
            write!(coord_buf, "{:02x}", x).unwrap();
        }
        let x = coord_buf[0..3].parse::<u16>().ok()?;
        let y = coord_buf[3..6].parse::<u16>().ok()?;

        let rgb = (data[5], data[6], data[7]);

        Some(Self { x, y, rgb })
    }
}

pub struct Chunk {
    pub block_num: u64,
    pub cmds: Vec<Command>,
}
