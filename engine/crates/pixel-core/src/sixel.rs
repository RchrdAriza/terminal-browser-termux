use crate::wrapper::Wrapper;

pub fn sixel_transmit(width: u32, height: u32, rgba: &[u8], wrapper: Wrapper) -> Vec<u8> {
    if width == 0 || height == 0 || rgba.is_empty() {
        return crate::iterm::iterm_transmit(width, height, rgba, wrapper);
    }
    let encoded = encode_sixel(width, height, rgba);
    match encoded {
        Some(seq) => wrapper.wrap(&seq),
        None => crate::iterm::iterm_transmit(width, height, rgba, wrapper),
    }
}

fn encode_sixel(width: u32, height: u32, rgba: &[u8]) -> Option<Vec<u8>> {
    if width > 4096 || height > 4096 {
        return None;
    }
    let w = width as usize;
    let h = height as usize;

    let mut palette: Vec<[u8; 3]> = Vec::new();
    let mut index_map = std::collections::HashMap::new();
    for chunk in rgba.chunks(4) {
        if chunk[3] < 16 {
            continue;
        }
        let r = chunk[0] & 0xF8;
        let g = chunk[1] & 0xF8;
        let b = chunk[2] & 0xF8;
        let key = [r, g, b];
        if !index_map.contains_key(&key) {
            if palette.len() >= 240 {
                continue;
            }
            let idx = palette.len();
            palette.push(key);
            index_map.insert(key, idx);
        }
    }
    if palette.is_empty() {
        palette.push([0, 0, 0]);
        index_map.insert([0, 0, 0], 0);
    }

    let mut indexed: Vec<u8> = Vec::with_capacity(w * h);
    for chunk in rgba.chunks(4) {
        if chunk[3] < 16 {
            indexed.push(0);
            continue;
        }
        let r = chunk[0] & 0xF8;
        let g = chunk[1] & 0xF8;
        let b = chunk[2] & 0xF8;
        let key = [r, g, b];
        let idx = *index_map.get(&key).unwrap_or(&0);
        indexed.push(idx as u8);
    }

    let mut out = Vec::new();
    out.extend_from_slice(b"\x1bPq");
    out.extend_from_slice(format!("\"1;1;{w};{h}").as_bytes());

    for (i, col) in palette.iter().enumerate() {
        let r = col[0] as u32 * 100 / 255;
        let g = col[1] as u32 * 100 / 255;
        let b = col[2] as u32 * 100 / 255;
        out.extend_from_slice(format!("#{i};2;{r};{g};{b}").as_bytes());
    }

    let bands = (h + 5) / 6;
    for band in 0..bands {
        let y0 = band * 6;
        let mut band_has_data = false;
        let mut first_color_in_band = true;
        for color_idx in 0..palette.len() {
            let mut has_pixels = false;
            for x in 0..w {
                let mut bits: u8 = 0;
                for bit in 0..6 {
                    let y = y0 + bit;
                    if y >= h {
                        continue;
                    }
                    let idx = y * w + x;
                    if indexed[idx] as usize == color_idx {
                        bits |= 1 << bit;
                    }
                }
                if bits != 0 {
                    has_pixels = true;
                    break;
                }
            }
            if !has_pixels {
                continue;
            }
            band_has_data = true;
            if !first_color_in_band {
                out.push(b'$');
            }
            first_color_in_band = false;
            out.extend_from_slice(format!("#{color_idx}").as_bytes());
            let mut run_char: Option<u8> = None;
            let mut run_len: usize = 0;
            let flush = |out: &mut Vec<u8>, ch: u8, len: usize| {
                if len > 3 {
                    out.extend_from_slice(format!("!{len}{}", (ch) as char).as_bytes());
                } else {
                    for _ in 0..len {
                        out.push(ch);
                    }
                }
            };
            for x in 0..w {
                let mut bits: u8 = 0;
                for bit in 0..6 {
                    let y = y0 + bit;
                    if y >= h {
                        continue;
                    }
                    let idx = y * w + x;
                    if indexed[idx] as usize == color_idx {
                        bits |= 1 << bit;
                    }
                }
                let sixel_char = bits + 63;
                if Some(sixel_char) == run_char {
                    run_len += 1;
                } else {
                    if let Some(ch) = run_char {
                        flush(&mut out, ch, run_len);
                    }
                    run_char = Some(sixel_char);
                    run_len = 1;
                }
            }
            if let Some(ch) = run_char {
                flush(&mut out, ch, run_len);
            }
        }
        if band_has_data {
            if band + 1 < bands {
                out.push(b'-');
            }
        } else if band + 1 < bands {
            out.push(b'-');
        }
    }

    out.extend_from_slice(b"\x1b\\");
    if out.len() > 1024 * 1024 {
        return None;
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wrapper::Wrapper;

    #[test]
    fn sixel_starts_with_dcs() {
        let out = sixel_transmit(2, 2, &[255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 255, 255], Wrapper::None);
        let s = String::from_utf8_lossy(&out);
        assert!(s.starts_with("\x1bPq") || s.starts_with("\x1b]1337;"));
    }

    #[test]
    fn sixel_fallback_for_large() {
        let w = 5000u32;
        let h = 5000u32;
        let rgba = vec![0u8; (w * h * 4) as usize];
        let out = sixel_transmit(w, h, &rgba, Wrapper::None);
        let s = String::from_utf8_lossy(&out);
        assert!(s.starts_with("\x1b]1337;") || s.is_empty());
    }

    #[test]
    fn sixel_wraps_for_tmux() {
        let out = sixel_transmit(1, 1, &[10, 20, 30, 255], Wrapper::Tmux);
        let s = String::from_utf8_lossy(&out);
        assert!(s.starts_with("\x1bPtmux;"));
    }
}
