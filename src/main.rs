#![recursion_limit = "1024"]

use anyhow::anyhow;
use sdl2::event::Event;
use sdl2::keyboard::Keycode;
use sdl2::pixels::{Color, PixelFormatEnum};
use sdl2::rect::{Point, Rect};
use std::sync::mpsc;
use std::time::Duration;

mod block_feed;
mod chain_data;

// In order to make it work on mac the following env is required
//
//    export LIBRARY_PATH="$LIBRARY_PATH:/usr/local/lib"
//

fn main() -> anyhow::Result<()> {
    env_logger::init();

    let sdl_context = sdl2::init().unwrap();
    let video_subsystem = sdl_context.video().unwrap();

    let mut window = video_subsystem
        .window("rust-sdl2 demo", 1000, 1000)
        .position_centered()
        .build()
        .unwrap();
    window
        .set_fullscreen(sdl2::video::FullscreenType::True)
        .map_err(|msg| anyhow!(msg))?;

    let mut canvas = window.into_canvas().build().unwrap();

    canvas.set_draw_color(Color::RGB(0, 0, 0));
    canvas.clear();
    canvas.present();
    let mut event_pump = sdl_context.event_pump().unwrap();

    let (tx, rx) = mpsc::channel();
    let handle = async_std::task::spawn(async move {
        let mut stream = block_feed::CommandStream::new().await.unwrap();
        loop {
            let commands = stream.next().await;
            if let Err(_) = tx.send(commands) {
                // The other end hung-up. We treat it as a shutdown signal.
                break;
            }
        }
    });

    let creator = canvas.texture_creator();
    let mut texture = creator.create_texture_target(PixelFormatEnum::RGBA8888, 1000, 1000)?;

    canvas.with_texture_canvas(&mut texture, |texture_canvas| {
        texture_canvas.set_draw_color(Color::RGB(0, 0, 0));
        texture_canvas.clear();
    })?;

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
                Ok(commands) => {
                    canvas.with_texture_canvas(&mut texture, |texture_canvas| {
                        for command in commands {
                            texture_canvas.set_draw_color(Color::from(command.rgb));
                            texture_canvas
                                .draw_point((command.x as i32, command.y as i32))
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
