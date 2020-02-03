#![recursion_limit = "1024"]

use anyhow::anyhow;
use log::{debug, info, warn};
use sdl2::event::Event;
use sdl2::keyboard::Keycode;
use sdl2::pixels::{Color, PixelFormatEnum};
use sdl2::rect::Rect;
use sdl2::surface::Surface;
use std::env;
use std::sync::mpsc;
use std::time::Duration;

mod block_feed;
mod chain_data;
mod command;
mod persist;

const PERSISTED_DATA_FILENAME: &str = "persisted_data";

fn rpc_hostname() -> String {
    env::var("RPC_HOST").unwrap_or_else(|_| "ws://localhost:1234".to_string())
}

fn go_fullscreen() -> bool {
    let mut fullscreen_var = match env::var("FULLSCREEN") {
        Ok(fullscreen_var) => fullscreen_var,
        Err(_) => return true,
    };
    fullscreen_var.make_ascii_lowercase();

    match &*fullscreen_var {
        "0" | "false" | "no" | "n" => false,
        _ => true,
    }
}

fn main() -> anyhow::Result<()> {
    env_logger::init();
    let _ = dotenv::dotenv();

    let (mut persisted_data, mut persister) = persist::init(PERSISTED_DATA_FILENAME)?;

    let sdl_context = sdl2::init().unwrap();
    let video_subsystem = sdl_context.video().unwrap();

    let mut window = video_subsystem
        .window("rust-sdl2 demo", 1000, 1000)
        .position_centered()
        .build()
        .unwrap();

    if go_fullscreen() {
        window
            .set_fullscreen(sdl2::video::FullscreenType::True)
            .map_err(|msg| anyhow!(msg))?;
    }

    let mut canvas = window.into_canvas().build().unwrap();

    canvas.set_draw_color(Color::RGB(0, 0, 0));
    canvas.clear();
    canvas.present();
    let mut event_pump = sdl_context.event_pump().unwrap();

    // Create the texture to draw the contents of the remarkable image.
    //
    // Initialize it with the persisted data.
    let creator = canvas.texture_creator();
    let mut texture = creator.create_texture_streaming(
        PixelFormatEnum::RGB24,
        command::CANVAS_WIDTH as u32,
        command::CANVAS_HEIGHT as u32,
    )?;
    texture.update(
        None,
        &persisted_data.image_data[..],
        3 * command::CANVAS_WIDTH,
    )?;

    // Start the block feed.
    let start_block_num = persisted_data.block_num;
    let (tx, rx) = mpsc::channel();
    let _handle = async_std::task::spawn(async move {
        let mut stream = block_feed::ChunkStream::new(&rpc_hostname(), start_block_num)
            .await
            .unwrap();
        loop {
            let chunk = stream.next().await;
            if chunk.block_num % 1000 == 0 {
                info!("current block: {}", chunk.block_num);
            }
            if !chunk.cmds.is_empty() {
                info!("{} has {} cmds", chunk.block_num, chunk.cmds.len());
            }
            persister.apply(&chunk).await;
            if let Err(_) = tx.send(chunk) {
                // The other end hung-up. We treat it as a shutdown signal.
                break;
            }
        }

        debug!("Shutting down the persister");
        if let Err(err) = persister.shutdown().await {
            warn!(
                "An error occured while shutting down the persister: {}",
                err
            );
        }
    });

    'running: loop {
        for event in event_pump.poll_iter() {
            match event {
                Event::Quit { .. }
                | Event::KeyDown {
                    keycode: Some(Keycode::Escape),
                    ..
                } => break 'running,
                _ => {}
            }
        }

        loop {
            match rx.try_recv() {
                Ok(chunk) => {
                    if chunk.cmds.is_empty() {
                        continue;
                    }

                    for cmd in &chunk.cmds {
                        texture.update(
                            Rect::new(cmd.x as i32, cmd.y as i32, 1, 1),
                            &[cmd.rgb.0, cmd.rgb.1, cmd.rgb.2],
                            3000,
                        )?;
                    }
                }
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => break 'running,
            }
        }

        canvas.set_draw_color(Color::RGB(0, 0, 0));
        canvas.clear();
        canvas
            .copy(&texture, None, None)
            .map_err(|msg| anyhow!(msg))?;
        canvas.present();

        ::std::thread::sleep(Duration::new(0, 1_000_000_000u32 / 60));
    }

    Ok(())
}
