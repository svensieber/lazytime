use eframe::egui;

pub fn color32_from_hex(hex: &str) -> Option<egui::Color32> {
    let raw = hex.trim().trim_start_matches('#');
    if raw.len() != 6 {
        return None;
    }
    let r = u8::from_str_radix(&raw[0..2], 16).ok()?;
    let g = u8::from_str_radix(&raw[2..4], 16).ok()?;
    let b = u8::from_str_radix(&raw[4..6], 16).ok()?;
    Some(egui::Color32::from_rgb(r, g, b))
}

pub fn hex_from_color32(col: egui::Color32) -> String {
    format!("#{:02X}{:02X}{:02X}", col.r(), col.g(), col.b())
}

pub fn generate_color_for_name(name: &str) -> String {
    let mut hash: u32 = 0x811C9DC5;
    for b in name.as_bytes() {
        hash ^= u32::from(*b);
        hash = hash.wrapping_mul(0x01000193);
    }
    let hue = (hash % 360) as f32;
    let (r, g, b) = hsl_to_rgb(hue, 0.60, 0.50);
    format!("#{:02X}{:02X}{:02X}", r, g, b)
}

fn hsl_to_rgb(h: f32, s: f32, l: f32) -> (u8, u8, u8) {
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let x = c * (1.0 - (((h / 60.0) % 2.0) - 1.0).abs());
    let m = l - c / 2.0;

    let (r1, g1, b1) = match h {
        h if (0.0..60.0).contains(&h) => (c, x, 0.0),
        h if (60.0..120.0).contains(&h) => (x, c, 0.0),
        h if (120.0..180.0).contains(&h) => (0.0, c, x),
        h if (180.0..240.0).contains(&h) => (0.0, x, c),
        h if (240.0..300.0).contains(&h) => (x, 0.0, c),
        _ => (c, 0.0, x),
    };

    let to_u8 = |v: f32| ((v + m).clamp(0.0, 1.0) * 255.0).round() as u8;
    (to_u8(r1), to_u8(g1), to_u8(b1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_roundtrip_rgb() {
        let c = color32_from_hex("#12ABEF").expect("valid hex");
        assert_eq!(hex_from_color32(c), "#12ABEF");
    }

    #[test]
    fn invalid_hex_is_none() {
        assert!(color32_from_hex("#12345").is_none());
        assert!(color32_from_hex("foo").is_none());
    }

    #[test]
    fn generated_color_is_stable() {
        assert_eq!(generate_color_for_name("Default"), "#5733CC");
        assert_eq!(generate_color_for_name("Project A"), "#33BFCC");
    }
}
