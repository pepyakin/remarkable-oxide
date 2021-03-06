#![recursion_limit = "1024"]

use anyhow::anyhow;
use remarkable_oxide_service::{CommStatus, StatusReport};
use sdl2::event::Event;
use sdl2::keyboard::Keycode;
use sdl2::pixels::{Color, PixelFormatEnum};
use sdl2::rect::Rect;
use std::time::Duration;

mod config;

const CANVAS_WIDTH: usize = 1000;
const CANVAS_HEIGHT: usize = 1000;

// Stride is defined as a number of bytes per row.
const STRIDE: usize = 3 * CANVAS_WIDTH;

fn init_logger() {
    let mut builder = env_logger::Builder::new();
    // jsonrpsee is very vocal on connection errors.
    builder.filter(Some("jsonrpsee::client"), log::LevelFilter::Off);
    if let Ok(lvl) = std::env::var("RUST_LOG") {
        builder.parse_filters(&lvl);
    }
    if builder.try_init().is_err() {
        eprintln!("Failed to register logger. Logging is turned off.");
    }
}

fn main() -> anyhow::Result<()> {
    init_logger();

    let config = config::obtain();
    let mut service = remarkable_oxide_service::start(config.clone())?;

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
        CANVAS_WIDTH as u32,
        CANVAS_HEIGHT as u32,
    )?;

    let image_data = service.image_data()?;
    texture.update(None, &image_data, STRIDE)?;

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

        while let Some(cmd) = service.poll() {
            texture.update(
                Rect::new(cmd.x as i32, cmd.y as i32, 1, 1),
                &[cmd.rgb.0, cmd.rgb.1, cmd.rgb.2],
                STRIDE,
            )?;
        }

        canvas.set_draw_color(Color::RGB(0, 0, 0));
        canvas.clear();
        canvas
            .copy(&texture, None, None)
            .map_err(|msg| anyhow!(msg))?;
        canvas.present();

        let status = render_status_string(service.status_report());
        canvas
            .window_mut()
            .set_title(&status)
            .map_err(|msg| anyhow!(msg))?;

        // Just sleep 16ms here to achieve 60fps.
        //
        // That's really simple and trivial but it is enough for what we are trying to achieve here.
        ::std::thread::sleep(Duration::new(0, 1_000_000_000u32 / 60));
    }

    Ok(())
}

fn render_status_string(status_report: StatusReport) -> String {
    if status_report.connection_status == CommStatus::Connecting {
        "connecting".to_string()
    } else if status_report.current_block < status_report.finalized_block.saturating_sub(5) {
        format!(
            "syncing {}/{}",
            status_report.current_block, status_report.finalized_block
        )
    } else {
        "idle".to_string()
    }
}
