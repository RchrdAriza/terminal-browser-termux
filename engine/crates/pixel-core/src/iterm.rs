use crate::wrapper::Wrapper;
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;

pub fn iterm_transmit(width: u32, height: u32, rgba: &[u8], wrapper: Wrapper) -> Vec<u8> {
    assert_eq!(rgba.len(), (width * height * 4) as usize);
    let png = encode_png(width, height, rgba);
    let b64 = BASE64.encode(&png);
    let seq = format!(
        "\x1b]1337;File=inline=1;width={width}px;height={height}px;preserveAspectRatio=0:{b64}\x07"
    );
    wrapper.wrap(seq.as_bytes())
}

fn encode_png(width: u32, height: u32, rgba: &[u8]) -> Vec<u8> {
    if let Some(img) = image::RgbaImage::from_raw(width, height, rgba.to_vec()) {
        let mut cursor = std::io::Cursor::new(Vec::new());
        if img.write_to(&mut cursor, image::ImageFormat::Png).is_ok() {
            return cursor.into_inner();
        }
    }
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wrapper::Wrapper;

    #[test]
    fn iterm_osc_format() {
        let out = iterm_transmit(2, 2, &[255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 255, 255], Wrapper::None);
        let s = String::from_utf8_lossy(&out);
        assert!(s.starts_with("\x1b]1337;File=inline=1;"));
        assert!(s.contains("width=2px;height=2px"));
        assert!(s.ends_with("\x07"));
        let b64 = s.split(':').last().unwrap().trim_end_matches('\x07');
        let png = BASE64.decode(b64).unwrap();
        assert_eq!(&png[1..4], b"PNG");
    }

    #[test]
    fn iterm_wraps_for_tmux() {
        let out = iterm_transmit(1, 1, &[0, 0, 0, 255], Wrapper::Tmux);
        let s = String::from_utf8_lossy(&out);
        assert!(s.starts_with("\x1bPtmux;"));
        assert!(s.ends_with("\x1b\\"));
    }
}
