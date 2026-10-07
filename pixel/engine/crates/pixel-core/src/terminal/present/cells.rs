use std::io;

use super::super::{SessionEnv, Terminal};
use crate::canvas::{Canvas, Frame};
use crate::cell_graphics::{CellProtocol, over_black};
use crate::surfaces::Rect;

const GRAPHICS_PROBE_ID: u32 = 297;
const GRAPHICS_PROBE_TIMEOUT_MS: u64 = 1000;

// Termux only frees replaced images every 30 seconds or when scrollback is cleared,
// so after this many pixels we clear it and send a whole frame.
const CLEAR_AFTER_PIXELS: u64 = 24_000_000;
const MAX_RECTS: usize = 24;
const TOP_ROWS_WITHOUT_REGION: u32 = 2;

#[derive(Debug, Default)]
pub(crate) struct Cells {
    shown: Vec<u8>,
    size: Option<(u32, u32)>,
    pixels_since_clear: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::terminal) struct GraphicsReply {
    kitty: bool,
    sixel: bool,
}

pub(in crate::terminal) fn parse_graphics_reply(buf: &[u8]) -> Option<GraphicsReply> {
    let needle = format!("Gi={GRAPHICS_PROBE_ID};OK");
    let kitty = buf.windows(needle.len()).any(|w| w == needle.as_bytes());
    let mut at = 0;
    while let Some(found) = buf[at..].windows(3).position(|w| w == b"\x1b[?") {
        let params = at + found + 3;
        let end = params + buf[params..].iter().take_while(|b| b.is_ascii_digit() || **b == b';').count();
        if buf.get(end) == Some(&b'c') {
            let sixel = buf[params..end].split(|&b| b == b';').any(|p| p == b"4");
            return Some(GraphicsReply { kitty, sixel });
        }
        at = params;
    }
    None
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(in crate::terminal) struct Hints {
    termux: bool,
    iterm2: bool,
}

impl Hints {
    fn of(env: &SessionEnv) -> Self {
        Hints {
            termux: env.var("TERMUX_VERSION").is_some_and(|v| !v.is_empty()),
            iterm2: env.var("LC_TERMINAL").as_deref() == Some("iTerm2")
                || env.var("TERM_PROGRAM").as_deref() == Some("iTerm.app"),
        }
    }
}

/// None keeps the kitty graphics protocol.
pub(in crate::terminal) fn choose_cell_protocol(reply: Option<GraphicsReply>, hints: Hints) -> Option<CellProtocol> {
    match reply {
        Some(GraphicsReply { kitty: true, .. }) => None,
        _ if hints.iterm2 => Some(CellProtocol::Iterm2),
        // Termux draws sixel and iTerm2 images alike, and decodes the PNGs of iTerm2 far faster.
        Some(GraphicsReply { sixel: true, .. }) if hints.termux => Some(CellProtocol::Iterm2),
        Some(GraphicsReply { sixel: true, .. }) => Some(CellProtocol::Sixel),
        _ => None,
    }
}

fn forced_protocol(value: &str) -> Option<Option<CellProtocol>> {
    match value.trim() {
        "kitty" => Some(None),
        "sixel" => Some(Some(CellProtocol::Sixel)),
        "iterm2" | "iterm" => Some(Some(CellProtocol::Iterm2)),
        _ => None,
    }
}

fn snap(rect: Rect, cell: (u32, u32), size: (u32, u32)) -> Rect {
    let x = rect.x / cell.0 * cell.0;
    let y = rect.y / cell.1 * cell.1;
    let right = (rect.x + rect.w).div_ceil(cell.0) * cell.0;
    let bottom = (rect.y + rect.h).div_ceil(cell.1) * cell.1;
    Rect { x, y, w: right - x, h: bottom - y }.clamped(size.0, size.1)
}

fn touch(a: Rect, b: Rect) -> bool {
    a.x <= b.x + b.w && b.x <= a.x + a.w && a.y <= b.y + b.h && b.y <= a.y + a.h
}

fn merge(mut rects: Vec<Rect>) -> Vec<Rect> {
    'again: loop {
        for i in 0..rects.len() {
            for j in i + 1..rects.len() {
                if touch(rects[i], rects[j]) {
                    let other = rects.swap_remove(j);
                    rects[i] = rects[i].union(other);
                    continue 'again;
                }
            }
        }
        break;
    }
    if rects.len() > MAX_RECTS {
        return vec![rects.into_iter().fold(Rect::default(), Rect::union)];
    }
    rects
}

/// Terminals move the cursor below an image once it is drawn, which scrolls the screen when the image
/// reaches the last row, unless the cursor sits below the scrolling region. No region fits above the
/// top rows, so images starting there are cut short of the bottom.
fn scroll_safe_pieces(rect: Rect, cell_height: u32) -> Vec<Rect> {
    let split = TOP_ROWS_WITHOUT_REGION * cell_height;
    if rect.y >= split || rect.y + rect.h <= split {
        return vec![rect];
    }
    vec![
        Rect { h: split - rect.y, ..rect },
        Rect { y: split, h: rect.y + rect.h - split, ..rect },
    ]
}

fn place(out: &mut Vec<u8>, protocol: CellProtocol, rect: Rect, rgb: &[u8], cell: (u32, u32)) {
    let (col, row) = (rect.x / cell.0, rect.y / cell.1);
    if row >= TOP_ROWS_WITHOUT_REGION {
        out.extend_from_slice(format!("\x1b[1;{row}r").as_bytes());
    } else {
        out.extend_from_slice(b"\x1b[r");
    }
    out.extend_from_slice(format!("\x1b[{};{}H", row + 1, col + 1).as_bytes());
    out.extend_from_slice(&protocol.encode(rgb, rect.w, rect.h));
}

impl Cells {
    fn region(&self, width: u32, rect: Rect) -> Vec<u8> {
        let mut out = Vec::with_capacity(rect.area() as usize * 3);
        for y in rect.y..rect.y + rect.h {
            let start = (y as usize * width as usize + rect.x as usize) * 3;
            out.extend_from_slice(&self.shown[start..start + rect.w as usize * 3]);
        }
        out
    }

    fn take_changes(&mut self, canvas: &Canvas, premultiplied: bool, rect: Rect, cell: (u32, u32)) -> Option<Rect> {
        let width = canvas.width as usize;
        let mut changed: Option<Rect> = None;
        for y in rect.y..rect.y + rect.h {
            let src = (y as usize * width + rect.x as usize) * 4;
            let rgb = over_black(&canvas.pixels[src..src + rect.w as usize * 4], premultiplied);
            let dst = (y as usize * width + rect.x as usize) * 3;
            let shown = &mut self.shown[dst..dst + rgb.len()];
            let differs = |(a, b): (&[u8], &[u8])| a != b;
            let Some(first) = rgb.chunks_exact(3).zip(shown.chunks_exact(3)).position(differs) else {
                continue;
            };
            let last = rgb.chunks_exact(3).zip(shown.chunks_exact(3)).rposition(differs).unwrap_or(first);
            let row = Rect { x: rect.x + first as u32, y, w: (last - first) as u32 + 1, h: 1 };
            changed = Some(changed.map_or(row, |c| c.union(row)));
            shown.copy_from_slice(&rgb);
        }
        changed.map(|c| snap(c, cell, (canvas.width, canvas.height)))
    }
}

impl Terminal {
    pub(in crate::terminal) fn probe_cell_protocol(&mut self, env: &SessionEnv) -> io::Result<Option<CellProtocol>> {
        if let Some(forced) = env.var("TERMINAL_BROWSER_GRAPHICS").as_deref().and_then(forced_protocol) {
            crate::logging::info("terminal", format!("graphics forced to {forced:?}"));
            return Ok(forced);
        }
        if self.wrapper.relayed() {
            return Ok(None);
        }
        let query = format!("\x1b_Gi={GRAPHICS_PROBE_ID},a=q,t=d,f=24,s=1,v=1;AAAA\x1b\\\x1b[c");
        self.io.out().write_all(query.as_bytes())?;
        self.io.out().flush()?;
        let reply = self.read_report(GRAPHICS_PROBE_TIMEOUT_MS, parse_graphics_reply)?;
        let chosen = choose_cell_protocol(reply, Hints::of(env));
        crate::logging::info("terminal", format!("graphics reply {reply:?}, drawing with {}", chosen.map_or("kitty".to_string(), |p| format!("{p:?}"))));
        Ok(chosen)
    }

    pub(in crate::terminal) fn draw_cells(&mut self, protocol: CellProtocol, frame: Frame<'_>, out: &mut Vec<u8>) -> io::Result<usize> {
        self.cell_size()?;
        let cell = self.cell();
        let canvas = frame.canvas;
        let size = (canvas.width, canvas.height);
        let start = out.len();
        let resized = self.cells.size != Some(size);
        let rects = if resized || self.cells.pixels_since_clear >= CLEAR_AFTER_PIXELS {
            out.extend_from_slice(if resized { b"\x1b[r\x1b[2J\x1b[3J".as_slice() } else { b"\x1b[3J".as_slice() });
            self.cells.shown = over_black(&canvas.pixels, frame.premultiplied);
            self.cells.size = Some(size);
            self.cells.pixels_since_clear = 0;
            vec![Rect::sized(size.0, size.1)]
        } else {
            let damage = frame
                .changed
                .iter()
                .chain(frame.repainted)
                .map(|r| snap(*r, cell, size))
                .filter(|r| !r.is_empty())
                .collect();
            merge(damage)
                .into_iter()
                .filter_map(|r| self.cells.take_changes(canvas, frame.premultiplied, r, cell))
                .collect()
        };
        let mut pixels = 0;
        for rect in &rects {
            for piece in scroll_safe_pieces(*rect, cell.1) {
                let rgb = self.cells.region(size.0, piece);
                place(out, protocol, piece, &rgb, cell);
                pixels += piece.area();
            }
        }
        if !rects.is_empty() {
            out.extend_from_slice(b"\x1b[r");
        }
        self.cells.pixels_since_clear += pixels;
        crate::profiler::count("present.pixels", || pixels);
        Ok(out.len() - start)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const KITTY_OK: &[u8] = b"\x1b_Gi=297;OK\x1b\\";

    #[test]
    fn the_graphics_reply_waits_for_the_device_attributes() {
        assert_eq!(parse_graphics_reply(KITTY_OK), None);
        assert_eq!(
            parse_graphics_reply(b"\x1b_Gi=297;OK\x1b\\\x1b[?62;22c"),
            Some(GraphicsReply { kitty: true, sixel: false })
        );
        assert_eq!(
            parse_graphics_reply(b"\x1b[?2026;2$y\x1b[?64;1;2;4;6;9;15;18;21;22c"),
            Some(GraphicsReply { kitty: false, sixel: true })
        );
        assert_eq!(parse_graphics_reply(b"\x1b[?64;14c"), Some(GraphicsReply { kitty: false, sixel: false }));
        assert_eq!(parse_graphics_reply(b"\x1b[?64;4"), None, "reply mid-arrival");
    }

    #[test]
    fn kitty_wins_then_iterm2_then_sixel() {
        let reply = |kitty, sixel| Some(GraphicsReply { kitty, sixel });
        let plain = Hints::default();
        let termux = Hints { termux: true, iterm2: false };
        let iterm2 = Hints { termux: false, iterm2: true };
        assert_eq!(choose_cell_protocol(reply(true, true), termux), None);
        assert_eq!(choose_cell_protocol(reply(false, true), plain), Some(CellProtocol::Sixel));
        assert_eq!(choose_cell_protocol(reply(false, true), termux), Some(CellProtocol::Iterm2));
        assert_eq!(choose_cell_protocol(reply(false, false), iterm2), Some(CellProtocol::Iterm2));
        assert_eq!(choose_cell_protocol(None, iterm2), Some(CellProtocol::Iterm2));
        assert_eq!(choose_cell_protocol(reply(false, false), termux), None);
        assert_eq!(choose_cell_protocol(None, plain), None);
        assert_eq!(forced_protocol(" sixel "), Some(Some(CellProtocol::Sixel)));
        assert_eq!(forced_protocol("kitty"), Some(None));
        assert_eq!(forced_protocol("auto"), None);
    }

    #[test]
    fn damage_snaps_out_to_whole_cells_and_merges_when_touching() {
        let cell = (10, 20);
        assert_eq!(snap(Rect { x: 15, y: 25, w: 10, h: 1 }, cell, (100, 100)), Rect { x: 10, y: 20, w: 20, h: 20 });
        assert_eq!(snap(Rect { x: 95, y: 95, w: 10, h: 10 }, cell, (100, 100)), Rect { x: 90, y: 80, w: 10, h: 20 });
        let merged = merge(vec![
            Rect { x: 0, y: 0, w: 10, h: 20 },
            Rect { x: 50, y: 50, w: 10, h: 20 },
            Rect { x: 10, y: 0, w: 10, h: 20 },
        ]);
        assert_eq!(merged.len(), 2);
        assert!(merged.contains(&Rect { x: 0, y: 0, w: 20, h: 20 }));
    }

    #[test]
    fn images_from_the_top_rows_stop_before_the_region_starts() {
        assert_eq!(scroll_safe_pieces(Rect { x: 0, y: 40, w: 10, h: 400 }, 20).len(), 1);
        assert_eq!(scroll_safe_pieces(Rect { x: 0, y: 0, w: 10, h: 40 }, 20).len(), 1);
        assert_eq!(
            scroll_safe_pieces(Rect { x: 0, y: 20, w: 10, h: 100 }, 20),
            vec![Rect { x: 0, y: 20, w: 10, h: 20 }, Rect { x: 0, y: 40, w: 10, h: 80 }]
        );
        let mut out = Vec::new();
        place(&mut out, CellProtocol::Sixel, Rect { x: 30, y: 60, w: 10, h: 20 }, &[0; 600], (10, 20));
        assert!(out.starts_with(b"\x1b[1;3r\x1b[4;4H\x1bP"));
    }

    #[test]
    fn only_cells_that_changed_since_the_last_frame_are_resent() {
        let mut canvas = Canvas::new(40, 40);
        let mut cells = Cells { shown: over_black(&canvas.pixels, false), size: Some((40, 40)), pixels_since_clear: 0 };
        let whole = Rect::sized(40, 40);
        assert_eq!(cells.take_changes(&canvas, false, whole, (10, 10)), None);
        canvas.fill_rect(12, 25, 3, 2, [255, 0, 0, 255]);
        assert_eq!(cells.take_changes(&canvas, false, whole, (10, 10)), Some(Rect { x: 10, y: 20, w: 10, h: 10 }));
        assert_eq!(cells.take_changes(&canvas, false, whole, (10, 10)), None);
        assert_eq!(&cells.region(40, Rect { x: 12, y: 25, w: 1, h: 1 }), &[255, 0, 0]);
    }
}
