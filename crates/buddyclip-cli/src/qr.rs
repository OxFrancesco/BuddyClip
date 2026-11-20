use anyhow::Result;
use qrcode::{Color, QrCode};

/// Renders a high-contrast Unicode half-block QR code suitable for scanning directly
/// from any terminal color scheme using an Android phone camera.
pub fn render_terminal_qr(data: &str) -> Result<String> {
    let code = QrCode::new(data.as_bytes())?;
    let width = code.width();
    let quiet = 2usize;
    let total = width + quiet * 2;

    let is_dark = |x: usize, y: usize| -> bool {
        if x < quiet || y < quiet || x >= width + quiet || y >= width + quiet {
            return false;
        }
        code[(x - quiet, y - quiet)] == Color::Dark
    };

    let mut out = String::new();
    // Use explicit black foreground on white background ANSI codes so scanners see
    // crisp dark-on-light modules even in dark terminal themes.
    let mut y = 0;
    while y < total {
        out.push_str("\x1b[30;47m");
        for x in 0..total {
            let top = is_dark(x, y);
            let bottom = if y + 1 < total {
                is_dark(x, y + 1)
            } else {
                false
            };
            let ch = match (top, bottom) {
                (true, true) => '█',
                (true, false) => '▀',
                (false, true) => '▄',
                (false, false) => ' ',
            };
            out.push(ch);
        }
        out.push_str("\x1b[0m\n");
        y += 2;
    }

    Ok(out)
}
