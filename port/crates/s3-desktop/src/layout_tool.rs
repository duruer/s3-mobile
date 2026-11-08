//! Dev tools for the full-screen menu layouts (docs/porting/menu-layout.md), no game needed:
//!   s3run --layout-preview <frame.png> <out.png> <WxH> [layout-name]   render a frame through a layout
//!   s3run --layout-probe <frame.png> x,y [x,y ...]                      print 4x4 block means (probe values)
//! Frames are 800x600 screenshots taken with `--inject "t:shot f.png"` (local only, never commit them).
use std::path::Path;

use s3_mods::display::{DisplayMode, ScaleFilter, Viewport};
use s3_mods::menu_layout;

fn read_rgb565(path: &Path) -> Result<(Vec<u16>, u32, u32), String> {
    let dec = png::Decoder::new(std::fs::File::open(path).map_err(|e| e.to_string())?);
    let mut r = dec.read_info().map_err(|e| e.to_string())?;
    let mut buf = vec![0; r.output_buffer_size()];
    let info = r.next_frame(&mut buf).map_err(|e| e.to_string())?;
    let ch = match info.color_type {
        png::ColorType::Rgb => 3,
        png::ColorType::Rgba => 4,
        c => return Err(format!("unsupported png colour type {c:?}")),
    };
    let px = buf[..info.buffer_size()]
        .chunks(ch)
        .map(|p| ((p[0] as u16 >> 3) << 11) | ((p[1] as u16 >> 2) << 5) | (p[2] as u16 >> 3))
        .collect();
    Ok((px, info.width, info.height))
}

/// Handle the tool arguments; `None` if `args` is not a tool invocation.
pub fn run(args: &[String]) -> Option<Result<(), String>> {
    match args.first().map(|s| s.as_str()) {
        Some("--layout-preview") => Some(preview(&args[1..])),
        Some("--layout-probe") => Some(probe(&args[1..])),
        _ => None,
    }
}

fn preview(a: &[String]) -> Result<(), String> {
    let [frame, out, size, rest @ ..] = a else { return Err("usage: --layout-preview frame.png out.png WxH [name]".into()) };
    let (ww, wh) = size.split_once('x').and_then(|(w, h)| Some((w.parse::<u32>().ok()?, h.parse::<u32>().ok()?))).ok_or("bad WxH")?;
    let (px, w, h) = read_rgb565(Path::new(frame))?;
    let vp = match rest.first() {
        Some(name) => {
            let l = menu_layout::by_name(name).ok_or(format!("no layout '{name}'"))?;
            Viewport::with_layout(DisplayMode::NativeWide, l, ww, wh)
        }
        None => Viewport::for_frame(DisplayMode::NativeWide, &px, w, h, ww, wh),
    };
    eprintln!("layout: {}", vp.layout.map(|l| l.name).unwrap_or("none (Fit)"));
    let mut buf = vec![0u32; (ww * wh) as usize];
    s3_mods::display::scale_to_xrgb(&vp, ScaleFilter::Nearest, &px, &mut buf);
    let f = std::fs::File::create(out).map_err(|e| e.to_string())?;
    let mut enc = png::Encoder::new(std::io::BufWriter::new(f), ww, wh);
    enc.set_color(png::ColorType::Rgb);
    enc.set_depth(png::BitDepth::Eight);
    let rgb: Vec<u8> = buf.iter().flat_map(|p| [(p >> 16) as u8, (p >> 8) as u8, *p as u8]).collect();
    enc.write_header().map_err(|e| e.to_string())?.write_image_data(&rgb).map_err(|e| e.to_string())
}

fn probe(a: &[String]) -> Result<(), String> {
    let [frame, pts @ ..] = a else { return Err("usage: --layout-probe frame.png x,y ...".into()) };
    let (px, w, _h) = read_rgb565(Path::new(frame))?;
    for p in pts {
        let (x, y) = p.split_once(',').and_then(|(x, y)| Some((x.parse::<usize>().ok()?, y.parse::<usize>().ok()?))).ok_or("bad x,y")?;
        let m = menu_layout::block_mean(&px, w as usize, x, y);
        println!("        pr({x}, {y}, {}, {}, {}),", m[0], m[1], m[2]);
    }
    Ok(())
}
