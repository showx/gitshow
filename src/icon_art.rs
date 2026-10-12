/// Square app icon: a blue tile with a white git branch.
/// Distances are in pixels so the mark stays readable at 16px.

pub fn render(size: u32) -> Vec<u8> {
    if size < 64 {
        let scale = 4;
        let big = render_exact(size * scale);
        downsample(&big, size * scale, size)
    } else {
        render_exact(size)
    }
}

fn render_exact(size: u32) -> Vec<u8> {
    let mut pixels = vec![0u8; (size as usize) * (size as usize) * 4];
    let s = size as f32;
    for y in 0..size {
        for x in 0..size {
            let color = sample(x as f32 + 0.5, y as f32 + 0.5, s);
            let index = ((y * size + x) * 4) as usize;
            pixels[index..index + 4].copy_from_slice(&color);
        }
    }
    pixels
}

fn sample(x: f32, y: f32, s: f32) -> [u8; 4] {
    let mut pixel = [0u8; 4];
    let margin = s * 0.02;
    let radius = s * 0.22;
    let tile = sd_round_box(
        x,
        y,
        s * 0.5,
        s * 0.5,
        s * 0.5 - margin,
        s * 0.5 - margin,
        radius,
    );
    let tile_cover = cover(-tile);
    if tile_cover > 0.0 {
        let t = (y / s).clamp(0.0, 1.0);
        let rgb = lerp3([58, 142, 255], [20, 88, 206], t);
        blend(&mut pixel, [rgb[0], rgb[1], rgb[2], (tile_cover * 255.0) as u8]);
    }

    let node = (s * 0.078).max(s * 0.07);
    let stroke = (s * 0.052).max(s * 0.046);
    let trunk_x = s * 0.36;
    let branch_x = s * 0.66;
    let top_y = s * 0.30;
    let joint_y = s * 0.50;
    let bot_y = s * 0.72;

    let mark = sd_segment(x, y, trunk_x, bot_y, trunk_x, top_y, stroke)
        .min(sd_segment(x, y, trunk_x, joint_y, branch_x, top_y, stroke))
        .min(sd_circle(x, y, trunk_x, top_y, node))
        .min(sd_circle(x, y, trunk_x, bot_y, node))
        .min(sd_circle(x, y, branch_x, top_y, node));
    let mark_cover = cover(-mark) * tile_cover;
    if mark_cover > 0.0 {
        blend(
            &mut pixel,
            [255, 255, 255, (mark_cover * 255.0) as u8],
        );
    }
    pixel
}

fn downsample(src: &[u8], src_size: u32, dst_size: u32) -> Vec<u8> {
    let scale = (src_size / dst_size) as usize;
    let area = (scale * scale) as f32;
    let mut out = vec![0u8; (dst_size as usize) * (dst_size as usize) * 4];
    for y in 0..dst_size as usize {
        for x in 0..dst_size as usize {
            let mut acc = [0f32; 4];
            for oy in 0..scale {
                for ox in 0..scale {
                    let sx = x * scale + ox;
                    let sy = y * scale + oy;
                    let i = (sy * src_size as usize + sx) * 4;
                    let a = src[i + 3] as f32 / 255.0;
                    acc[0] += src[i] as f32 / 255.0 * a;
                    acc[1] += src[i + 1] as f32 / 255.0 * a;
                    acc[2] += src[i + 2] as f32 / 255.0 * a;
                    acc[3] += a;
                }
            }
            let alpha = acc[3] / area;
            let index = (y * dst_size as usize + x) * 4;
            if alpha > 0.0 {
                out[index] = ((acc[0] / area) / alpha * 255.0).round().clamp(0.0, 255.0) as u8;
                out[index + 1] =
                    ((acc[1] / area) / alpha * 255.0).round().clamp(0.0, 255.0) as u8;
                out[index + 2] =
                    ((acc[2] / area) / alpha * 255.0).round().clamp(0.0, 255.0) as u8;
                out[index + 3] = (alpha * 255.0).round().clamp(0.0, 255.0) as u8;
            }
        }
    }
    out
}

fn blend(dst: &mut [u8], src: [u8; 4]) {
    let sa = src[3] as f32 / 255.0;
    if sa <= 0.0 {
        return;
    }
    let da = dst[3] as f32 / 255.0;
    let out_a = sa + da * (1.0 - sa);
    for channel in 0..3 {
        let s = src[channel] as f32 / 255.0;
        let d = dst[channel] as f32 / 255.0;
        let mixed = (s * sa + d * da * (1.0 - sa)) / out_a;
        dst[channel] = (mixed * 255.0).round().clamp(0.0, 255.0) as u8;
    }
    dst[3] = (out_a * 255.0).round().clamp(0.0, 255.0) as u8;
}

fn cover(sd: f32) -> f32 {
    (0.5 + sd).clamp(0.0, 1.0)
}

fn sd_circle(x: f32, y: f32, cx: f32, cy: f32, r: f32) -> f32 {
    let dx = x - cx;
    let dy = y - cy;
    dx.hypot(dy) - r
}

fn sd_segment(x: f32, y: f32, ax: f32, ay: f32, bx: f32, by: f32, r: f32) -> f32 {
    let abx = bx - ax;
    let aby = by - ay;
    let apx = x - ax;
    let apy = y - ay;
    let denom = abx * abx + aby * aby;
    let h = if denom == 0.0 {
        0.0
    } else {
        ((apx * abx + apy * aby) / denom).clamp(0.0, 1.0)
    };
    let dx = apx - abx * h;
    let dy = apy - aby * h;
    dx.hypot(dy) - r
}

fn sd_round_box(x: f32, y: f32, cx: f32, cy: f32, half_w: f32, half_h: f32, r: f32) -> f32 {
    let dx = (x - cx).abs() - (half_w - r);
    let dy = (y - cy).abs() - (half_h - r);
    let outside = dx.max(0.0).hypot(dy.max(0.0));
    outside + dx.max(dy).min(0.0) - r
}

fn lerp3(a: [u8; 3], b: [u8; 3], t: f32) -> [u8; 3] {
    let mix = |p: u8, q: u8| (p as f32 + (q as f32 - p as f32) * t).round() as u8;
    [mix(a[0], b[0]), mix(a[1], b[1]), mix(a[2], b[2])]
}
