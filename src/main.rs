#![recursion_limit = "1024"]

use anyhow::anyhow;
use log::{debug, info, warn};
use sdl2::event::Event;
use sdl2::keyboard::Keycode;
use sdl2::pixels::{Color, PixelFormatEnum};
use sdl2::rect::Rect;
use sdl2::surface::Surface;
use std::sync::mpsc;
use std::time::Duration;

mod block_feed;
mod chain_data;
mod command;
mod persist;

// In order to make it work on mac the following env is required
//
//    export LIBRARY_PATH="$LIBRARY_PATH:/usr/local/lib"
//

const PERSISTED_DATA_FILENAME: &str = "persisted_data";

fn main() -> anyhow::Result<()> {
    env_logger::init();

    let (mut persisted_data, mut persister) = persist::init(PERSISTED_DATA_FILENAME)?;

    let sdl_context = sdl2::init().unwrap();
    let video_subsystem = sdl_context.video().unwrap();

    let mut window = video_subsystem
        .window("rust-sdl2 demo", 1000, 1000)
        .position_centered()
        .build()
        .unwrap();
    // window
    //     .set_fullscreen(sdl2::video::FullscreenType::True)
    //     .map_err(|msg| anyhow!(msg))?;

    let mut canvas = window.into_canvas().build().unwrap();

    canvas.set_draw_color(Color::RGB(0, 0, 0));
    canvas.clear();
    canvas.present();
    let mut event_pump = sdl_context.event_pump().unwrap();

    // Create the texture to draw the contents of the remarkable image.
    //
    // Initialize it with the persisted data.
    let initial_surface = Surface::from_data(
        &mut persisted_data.image_data[..],
        command::CANVAS_WIDTH as u32,
        command::CANVAS_HEIGHT as u32,
        3 * command::CANVAS_WIDTH as u32,
        PixelFormatEnum::RGB888,
    )
    .map_err(|msg| anyhow!(msg))?;
    let creator = canvas.texture_creator();
    let mut texture = creator.create_texture_from_surface(initial_surface)?;

    // Start the block feed.
    let start_block_num = persisted_data.block_num;
    let (tx, rx) = mpsc::channel();
    let _handle = async_std::task::spawn(async move {
        const RPC: &str = "ws://localhost:1234";
        // const RPC: &str = "wss://kusama-rpc.polkadot.io/";

        let mut stream = block_feed::ChunkStream::new(RPC, start_block_num)
            .await
            .unwrap();
        loop {
            let chunk = stream.next().await;
            if chunk.block_num % 1000 == 0 {
                info!("current block: {}", chunk.block_num);
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
                    if chunk.cmds.is_empty() { continue; }

                    canvas.with_texture_canvas(&mut texture, |texture_canvas| {
                        for cmd in &chunk.cmds {
                            texture_canvas.set_draw_color(Color::from(cmd.rgb));
                            texture_canvas
                                .draw_point((cmd.x as i32, cmd.y as i32))
                                .unwrap();
                        }
                    })?;
                }
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => break 'running,
            }
        }

        canvas.set_draw_color(Color::RGB(0, 0, 0));
        canvas.clear();
        let dst = Some(Rect::new(0, 0, 1000, 1000));
        canvas
            .copy_ex(&texture, None, dst, 0.0, None, false, false)
            .map_err(|msg| anyhow!(msg))?;
        canvas.present();

        ::std::thread::sleep(Duration::new(0, 1_000_000_000u32 / 60));
    }

    Ok(())
}
