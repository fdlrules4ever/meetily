//! Shows outside the app window that a recording is running: a red dot on the tray icon
//! (orange while paused) with the recording time in its tooltip, a red dot on the taskbar
//! button on Windows and a badge on the Dock icon on macOS.
//!
//! Closing the window hides it to the tray, which also removes its taskbar button, so the tray
//! icon is the one that is always visible. Windows drops the overlay when the button is
//! recreated, so [`refresh_window`] re-applies it when the window gets focus again.

use std::sync::Mutex;

use log::warn;
use tauri::{image::Image, AppHandle, Manager, Runtime, WebviewWindow};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Indicator {
    Off,
    Recording,
    Paused,
}

const TRAY_ID: &str = "main-tray";
const IDLE_TOOLTIP: &str = "Meetily";
const RED: [u8; 3] = [0xE5, 0x39, 0x35];
const ORANGE: [u8; 3] = [0xFB, 0x8C, 0x00];
const WHITE: [u8; 3] = [0xFF, 0xFF, 0xFF];

static CURRENT: Mutex<Indicator> = Mutex::new(Indicator::Off);
static TOOLTIP: Mutex<String> = Mutex::new(String::new());

fn color(indicator: Indicator) -> Option<[u8; 3]> {
    match indicator {
        Indicator::Off => None,
        Indicator::Recording => Some(RED),
        Indicator::Paused => Some(ORANGE),
    }
}

/// Sets the tray icon, tooltip, taskbar overlay and Dock badge for `indicator`.
pub fn apply<R: Runtime>(app: &AppHandle<R>, indicator: Indicator) {
    *CURRENT.lock().unwrap_or_else(|e| e.into_inner()) = indicator;

    if let (Some(tray), Some(base)) = (app.tray_by_id(TRAY_ID), app.default_window_icon()) {
        let icon = match color(indicator) {
            Some(c) => with_dot(base, c),
            None => base.clone().to_owned(),
        };
        if let Err(e) = tray.set_icon(Some(icon)) {
            warn!("Could not update the tray icon: {}", e);
        }
    }
    set_tooltip(
        app,
        match indicator {
            Indicator::Off => IDLE_TOOLTIP.to_string(),
            Indicator::Recording => "Meetily – recording".to_string(),
            Indicator::Paused => "Meetily – recording paused".to_string(),
        },
    );

    if let Some(window) = app.get_webview_window("main") {
        apply_to_window(&window, indicator);
    }
}

/// Re-applies the taskbar overlay / Dock badge, for when the window is shown again.
pub fn refresh_window<R: Runtime>(window: &WebviewWindow<R>) {
    let indicator = *CURRENT.lock().unwrap_or_else(|e| e.into_inner());
    if indicator != Indicator::Off {
        apply_to_window(window, indicator);
    }
}

/// Keeps the tray tooltip's recording time current (called every few seconds).
pub fn show_duration<R: Runtime>(app: &AppHandle<R>, paused: bool, active_secs: u64) {
    let time = describe_duration(active_secs);
    set_tooltip(
        app,
        if paused {
            format!("Meetily – recording paused ({})", time)
        } else {
            format!("Meetily – recording for {}", time)
        },
    );
}

fn set_tooltip<R: Runtime>(app: &AppHandle<R>, text: String) {
    let mut current = TOOLTIP.lock().unwrap_or_else(|e| e.into_inner());
    if *current == text {
        return;
    }
    if let Some(tray) = app.tray_by_id(TRAY_ID) {
        match tray.set_tooltip(Some(&text)) {
            Ok(()) => *current = text,
            Err(e) => warn!("Could not update the tray tooltip: {}", e),
        }
    }
}

#[cfg(target_os = "windows")]
fn apply_to_window<R: Runtime>(window: &WebviewWindow<R>, indicator: Indicator) {
    if let Err(e) = window.set_overlay_icon(color(indicator).map(|c| dot_icon(32, c))) {
        warn!("Could not update the taskbar overlay icon: {}", e);
    }
}

#[cfg(target_os = "macos")]
fn apply_to_window<R: Runtime>(window: &WebviewWindow<R>, indicator: Indicator) {
    let label = match indicator {
        Indicator::Off => None,
        Indicator::Recording => Some("●".to_string()),
        Indicator::Paused => Some("II".to_string()),
    };
    if let Err(e) = window.set_badge_label(label) {
        warn!("Could not update the Dock badge: {}", e);
    }
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
fn apply_to_window<R: Runtime>(_window: &WebviewWindow<R>, _indicator: Indicator) {}

/// "under a minute", "42 min", "1 h 5 min".
fn describe_duration(secs: u64) -> String {
    let minutes = secs / 60;
    match minutes {
        0 => "under a minute".to_string(),
        m if m < 60 => format!("{} min", m),
        m => format!("{} h {} min", m / 60, m % 60),
    }
}

/// Paints `color` with coverage `alpha` over a straight-alpha RGBA pixel.
fn blend(px: &mut [u8], color: [u8; 3], alpha: f32) {
    if alpha <= 0.0 {
        return;
    }
    let dst_a = px[3] as f32 / 255.0;
    let out_a = alpha + dst_a * (1.0 - alpha);
    for i in 0..3 {
        let src = color[i] as f32 / 255.0;
        let dst = px[i] as f32 / 255.0;
        let out = (src * alpha + dst * dst_a * (1.0 - alpha)) / out_a;
        px[i] = (out * 255.0).round() as u8;
    }
    px[3] = (out_a * 255.0).round() as u8;
}

/// Draws an anti-aliased dot with a white ring around it, centred at (`cx`, `cy`).
fn draw_dot(rgba: &mut [u8], width: u32, height: u32, cx: f32, cy: f32, radius: f32, color: [u8; 3]) {
    let ring = (radius * 0.22).max(1.0);
    for y in 0..height {
        for x in 0..width {
            let dx = x as f32 + 0.5 - cx;
            let dy = y as f32 + 0.5 - cy;
            let d = (dx * dx + dy * dy).sqrt();
            if d > radius + 1.0 {
                continue;
            }
            let i = ((y * width + x) * 4) as usize;
            let px = &mut rgba[i..i + 4];
            blend(px, WHITE, (radius + 0.5 - d).clamp(0.0, 1.0));
            blend(px, color, (radius - ring + 0.5 - d).clamp(0.0, 1.0));
        }
    }
}

/// A square image that is only the dot (the Windows taskbar overlay).
fn dot_icon(size: u32, color: [u8; 3]) -> Image<'static> {
    let mut rgba = vec![0u8; (size * size * 4) as usize];
    let centre = size as f32 / 2.0;
    draw_dot(&mut rgba, size, size, centre, centre, centre - 0.5, color);
    Image::new_owned(rgba, size, size)
}

/// The app icon with the dot in its bottom-right corner (the tray icon).
fn with_dot(base: &Image<'_>, color: [u8; 3]) -> Image<'static> {
    let (width, height) = (base.width(), base.height());
    let mut rgba = base.rgba().to_vec();
    if width == 0 || height == 0 || rgba.len() != (width * height * 4) as usize {
        return base.clone().to_owned();
    }
    let size = width.min(height) as f32;
    let radius = size * 0.28;
    let margin = size * 0.01;
    draw_dot(
        &mut rgba,
        width,
        height,
        width as f32 - radius - margin,
        height as f32 - radius - margin,
        radius,
        color,
    );
    Image::new_owned(rgba, width, height)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pixel(image: &Image<'_>, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * image.width() + x) * 4) as usize;
        image.rgba()[i..i + 4].try_into().unwrap()
    }

    #[test]
    fn dot_icon_is_coloured_in_the_middle_white_at_the_rim_and_clear_outside() {
        let icon = dot_icon(32, RED);
        assert_eq!(pixel(&icon, 16, 16), [RED[0], RED[1], RED[2], 255]);
        let rim = pixel(&icon, 16, 1);
        assert!(rim[0] > 240 && rim[1] > 240 && rim[2] > 240 && rim[3] > 200, "{:?}", rim);
        assert_eq!(pixel(&icon, 0, 0)[3], 0);
    }

    #[test]
    fn dot_goes_in_the_bottom_right_corner_and_leaves_the_rest_of_the_icon_alone() {
        let base_rgba: Vec<u8> = [10u8, 20, 30, 255].repeat(64 * 64);
        let base = Image::new_owned(base_rgba, 64, 64);
        let marked = with_dot(&base, ORANGE);
        assert_eq!(pixel(&marked, 5, 5), [10, 20, 30, 255]);
        assert_eq!(pixel(&marked, 45, 45), [ORANGE[0], ORANGE[1], ORANGE[2], 255]);
    }

    #[test]
    fn describes_recording_time() {
        assert_eq!(describe_duration(30), "under a minute");
        assert_eq!(describe_duration(42 * 60 + 10), "42 min");
        assert_eq!(describe_duration(65 * 60), "1 h 5 min");
    }
}
