#![recursion_limit = "1024"]

use anyhow::anyhow;
use futures::{future::FutureExt, pin_mut, select};
use log::{debug, info, warn};
use sdl2::event::Event;
use sdl2::keyboard::Keycode;
use sdl2::pixels::{Color, PixelFormatEnum};
use sdl2::rect::Rect;
use std::sync::mpsc;
use std::time::Duration;

mod block_feed;
mod chain_data;
mod command;
mod config;
mod persist;

fn main() -> anyhow::Result<()> {
    env_logger::init();
    let config = config::obtain();

    let (persisted_data, mut persister) = persist::init(&config.persisted_data_path)?;

    let sdl_context = sdl2::init().unwrap();
    let video_subsystem = sdl_context.video().unwrap();

    let mut window = video_subsystem
        .window("rust-sdl2 demo", 1000, 1000)
        .position_centered()
        .build()
        .unwrap();

    if config.fullscreen {
        window
            .set_fullscreen(sdl2::video::FullscreenType::True)
            .map_err(|msg| anyhow!(msg))?;
    }
    if config.hide_cursor {
        // Hide the cursor
        sdl_context.mouse().show_cursor(false);
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
        'toplevel: loop {
            let mut stream =
                match block_feed::ChunkStream::new(&config.rpc_hostname, start_block_num).await {
                    Ok(stream) => stream,
                    Err(err) => {
                        warn!("connection error: {}. Retrying...", err);
                        async_std::task::sleep(std::time::Duration::from_secs(5)).await;
                        continue;
                    }
                };
            loop {
                let chunk = stream.next().fuse();
                let timeout = async_std::task::sleep(std::time::Duration::from_secs(5)).fuse();
                pin_mut!(chunk, timeout);
                select! {
                    chunk = chunk => {
                        if chunk.block_num % 1000 == 0 {
                            info!("current block: {}", chunk.block_num);
                        }
                        if !chunk.cmds.is_empty() {
                            info!("{} has {} cmds", chunk.block_num, chunk.cmds.len());
                        }
                        if let Err(err) = persister.apply(&chunk).await {
                            warn!("Failed to persist {}", err);
                        }
                        if let Err(_) = tx.send(chunk) {
                            // The other end hung-up. We treat it as a shutdown signal.
                            break 'toplevel;
                        }
                    }
                    timeout = timeout => {
                        warn!("timeout getting chunks. Reconnecting...");
                        continue 'toplevel;
                    }
                }
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
